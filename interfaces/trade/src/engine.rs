//! The two role engines, transport-agnostic: each method consumes one
//! inbound message and produces the next outbound one, so any carrier
//! of bytes (an iroh stream, a test harness) can drive a swap.
//!
//! Stages are separate types consumed by each step, so a session
//! cannot replay a message or skip a round; what a stage carries is
//! exactly the plan data that survives to the next step.

use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, ensure};
use joint_tx::{PlannedEvent, TransferAcceptance, TransferOffer, TxPlan};
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver},
    frontend::{MainPod, MultiPodBuilder},
    lang::Module,
    middleware::{Hash, MainPodProver, Params, Statement, VDSet, Value, containers::Dictionary},
};
use pod2utils::{macros::BuildContext, map, rand_raw_value};
use txlib::{GroundingWitness, StateHeader, Tx, TxBuilder, compute_nullifier, obj_with_key};

use crate::protocol::{AcceptMsg, AcceptanceMsg, LegDisclosure, OfferMsg, PlanAckMsg, PlanDataMsg};

/// What proving a class guard takes: the predicate's name and where
/// the Rekey branch sits in its OR. The SDK emits the Rekey branch
/// after every action branch, so the index is the action-branch count.
#[derive(Clone, Debug)]
pub struct ClassGuardInfo {
    pub guard_name: String,
    pub rekey_branch: usize,
}

/// Guard info per class, keyed by the class's guard-predicate hash,
/// which is also the `type` value its objects carry.
#[derive(Clone, Debug, Default)]
pub struct ClassDirectory {
    classes: std::collections::HashMap<Hash, ClassGuardInfo>,
}

impl ClassDirectory {
    pub fn from_sdk_module(module: &sdk::SdkModule) -> Self {
        let mut classes = std::collections::HashMap::new();
        for class in module.classes() {
            let hash = module
                .class_hash(&class.name)
                .expect("loaded module resolves its own classes");
            classes.insert(
                hash,
                ClassGuardInfo {
                    guard_name: format!("Is{}", class.name),
                    rekey_branch: class.actions.len(),
                },
            );
        }
        Self { classes }
    }

    /// Merge another directory in; later entries win on collision.
    pub fn absorb(&mut self, other: ClassDirectory) {
        self.classes.extend(other.classes);
    }

    pub fn get(&self, class_hash: Hash) -> Result<&ClassGuardInfo> {
        self.classes
            .get(&class_hash)
            .ok_or_else(|| anyhow!("no installed class with guard hash {class_hash:#}"))
    }
}

/// Everything a proving session needs besides the swap's own data.
#[derive(Clone)]
pub struct SwapDeps {
    pub modules: Vec<Arc<Module>>,
    pub classes: ClassDirectory,
    pub mock: bool,
}

impl SwapDeps {
    fn vd_set(&self) -> VDSet {
        if self.mock {
            VDSet::new(&[])
        } else {
            DEFAULT_VD_SET.clone()
        }
    }

    fn build_ctx(&self) -> BuildContext {
        BuildContext {
            builder: MultiPodBuilder::new(&Params::default(), &self.vd_set()),
            modules: self.modules.clone(),
        }
    }

    fn prove_session(&self, builder: MultiPodBuilder) -> Result<MainPod> {
        let solution = builder
            .solve()
            .map_err(|err| anyhow!("proving session does not solve: {err}"))?;
        let prover: Box<dyn MainPodProver> = if self.mock {
            Box::new(MockProver {})
        } else {
            Box::new(Prover {})
        };
        let pod = solution
            .prove(prover.as_ref())
            .map_err(|err| anyhow!("proving session failed: {err}"))?
            .output_pod()
            .clone();
        pod.pod.verify().context("own pod fails verification")?;
        Ok(pod)
    }

    /// A mock pod cannot be recursively verified by the real prover
    /// (and a real pod is wasted on a mock run), so a wrong-mode pod
    /// fails here, at receipt, instead of as a panic mid-proving.
    fn check_pod_mode(&self, pod: &MainPod, whose: &str) -> Result<()> {
        let pod_is_mock = pod.pod.is_mock();
        ensure!(
            pod_is_mock == self.mock,
            "{whose} pod is {} but this side runs {}; both sides must use the same mode",
            if pod_is_mock { "mock" } else { "real" },
            if self.mock { "--mock" } else { "real proving" },
        );
        Ok(())
    }

    /// Prove `Is{class}` with `st_rekey` in the guard's Rekey branch.
    fn prove_guard(
        &self,
        ctx: &mut BuildContext,
        class_hash: Hash,
        header: &StateHeader,
        st_rekey: Statement,
    ) -> Result<Statement> {
        let info = self.classes.get(class_hash)?;
        let mut premises = vec![Statement::None; info.rekey_branch + 1];
        premises[info.rekey_branch] = st_rekey;
        ctx.apply_custom_pred(
            false,
            &info.guard_name,
            map!({"state_header" => header.array()}),
            premises,
        )
        .map_err(|err| anyhow!("guard {} does not apply: {err}", info.guard_name))
    }
}

/// The agreed effect, kept in the shape both engines derive it in.
/// Event 0 moves the accepter's object to the initiator; event 1 moves
/// the initiator's object to the accepter.
struct AgreedPlan {
    plan: TxPlan,
    context: Hash,
    accepter_object: LegPlan,
    initiator_object: LegPlan,
}

struct LegPlan {
    old: Hash,
    new: Hash,
    nullifier: Hash,
}

impl AgreedPlan {
    fn derive(
        accepter_object: LegPlan,
        initiator_object: LegPlan,
        header: &StateHeader,
    ) -> Result<Self> {
        let plan = TxPlan::new(
            vec![accepter_object.old, initiator_object.old],
            vec![
                PlannedEvent::Action(vec![PlannedEvent::Mutate {
                    old: accepter_object.old,
                    new: accepter_object.new,
                    nullifier: accepter_object.nullifier,
                }]),
                PlannedEvent::Action(vec![PlannedEvent::Mutate {
                    old: initiator_object.old,
                    new: initiator_object.new,
                    nullifier: initiator_object.nullifier,
                }]),
            ],
        )
        .map_err(|err| anyhow!("plan does not validate: {err}"))?;
        let context = plan.context(header.hash());
        Ok(Self {
            plan,
            context,
            accepter_object,
            initiator_object,
        })
    }

    fn new_commitments(&self) -> Vec<Hash> {
        vec![self.accepter_object.new, self.initiator_object.new]
    }

    fn nullifiers(&self) -> Vec<Hash> {
        vec![
            self.accepter_object.nullifier,
            self.initiator_object.nullifier,
        ]
    }
}

/// What a party is left holding once its proving is done: the state it
/// now controls and everything needed to watch the synchronizer for
/// the transaction landing.
pub struct SwapExpectation {
    pub received: Dictionary,
    pub tx_final: Hash,
    pub new_commitments: Vec<Hash>,
    pub nullifiers: Vec<Hash>,
}

/// The initiator's terminal output: the finalized transaction pod and
/// sets ready for posting, plus the expectation shared with the
/// accepter's side.
pub struct SwapOutcome {
    pub pod: MainPod,
    pub tx: Tx,
    pub expectation: SwapExpectation,
}

// ---------------------------------------------------------------- //
//                            Initiator                             //
// ---------------------------------------------------------------- //

pub struct Initiator {
    deps: SwapDeps,
    outgoing: Dictionary,
    want_class: Hash,
    new_key: Value,
}

impl Initiator {
    /// `outgoing` is the full current state of the object this party
    /// gives; `want_class` is the guard hash of the class it wants.
    pub fn new(deps: SwapDeps, outgoing: Dictionary, want_class: Hash) -> Self {
        Self {
            deps,
            outgoing,
            want_class,
            new_key: Value::from(rand_raw_value()),
        }
    }

    /// Consume the accepter's disclosure. `witness` is the grounding
    /// witness for both inputs, fetched by the caller once the second
    /// commitment is known; its header becomes the plan's header.
    pub fn on_accept(
        self,
        msg: &AcceptMsg,
        witness: Arc<GroundingWitness>,
    ) -> Result<(InitiatorNegotiated, PlanDataMsg)> {
        msg.accepter_object.validate(self.want_class)?;
        for commitment in [
            msg.accepter_object.old_commitment,
            self.outgoing.commitment(),
        ] {
            ensure!(
                witness.created_proofs.contains_key(&commitment),
                "grounding witness has no proof for input {commitment:#}"
            );
        }
        let incoming_new = obj_with_key(&msg.accepter_object.mid, self.new_key.clone());
        let reply = PlanDataMsg {
            initiator_object: LegDisclosure::of(&self.outgoing),
            header: witness.state_header.clone(),
            accepter_object_new: incoming_new.commitment(),
        };
        let next = InitiatorNegotiated {
            deps: self.deps,
            outgoing: self.outgoing,
            want_class: self.want_class,
            new_key: self.new_key,
            incoming: msg.accepter_object.clone(),
            incoming_new,
            witness,
        };
        Ok((next, reply))
    }
}

pub struct InitiatorNegotiated {
    deps: SwapDeps,
    outgoing: Dictionary,
    want_class: Hash,
    new_key: Value,
    incoming: LegDisclosure,
    incoming_new: Dictionary,
    witness: Arc<GroundingWitness>,
}

impl InitiatorNegotiated {
    /// The state this party will control if the deal lands: the
    /// counterparty's object under this party's new key. Fully
    /// determined (key included) from the data round on, so the caller
    /// can file it durably before any endorsement leaves the machine.
    pub fn projected_received(&self) -> &Dictionary {
        &self.incoming_new
    }

    /// Consume the accepter's plan ack, pin the agreed effect, and
    /// prove round 0: the offer of the initiator's own object.
    pub fn on_plan_ack(self, msg: &PlanAckMsg) -> Result<(InitiatorOffered, OfferMsg)> {
        let agreed = AgreedPlan::derive(
            LegPlan {
                old: self.incoming.old_commitment,
                new: self.incoming_new.commitment(),
                nullifier: self.incoming.nullifier,
            },
            LegPlan {
                old: self.outgoing.commitment(),
                new: msg.initiator_object_new,
                nullifier: compute_nullifier(&self.outgoing),
            },
            &self.witness.state_header,
        )?;
        ensure!(
            agreed.plan.tx_final() == msg.tx_final,
            "plans diverge: this side derives tx_final {:#}, counterparty {:#}",
            agreed.plan.tx_final(),
            msg.tx_final
        );

        let mut session = self.deps.build_ctx();
        let offer = TransferOffer::prove(&mut session, agreed.context, &self.outgoing);
        let pod = self.deps.prove_session(session.builder)?;
        let reply = OfferMsg {
            offer: offer.clone(),
            pod,
        };
        let next = InitiatorOffered {
            deps: self.deps,
            outgoing: self.outgoing,
            want_class: self.want_class,
            new_key: self.new_key,
            incoming: self.incoming,
            incoming_new: self.incoming_new,
            witness: self.witness,
            agreed,
        };
        Ok((next, reply))
    }
}

pub struct InitiatorOffered {
    deps: SwapDeps,
    outgoing: Dictionary,
    want_class: Hash,
    new_key: Value,
    incoming: LegDisclosure,
    incoming_new: Dictionary,
    witness: Arc<GroundingWitness>,
    agreed: AgreedPlan,
}

impl InitiatorOffered {
    /// Round 2: validate the accepter's combined session, assemble
    /// both legs, and finalize.
    pub fn on_acceptance(self, msg: AcceptanceMsg) -> Result<SwapOutcome> {
        self.deps.check_pod_mode(&msg.pod, "the accepter's")?;
        msg.pod
            .pod
            .verify()
            .context("accepter's pod fails verification")?;
        msg.offer
            .validate(&msg.pod, self.agreed.context, self.incoming.old_commitment)
            .context("accepter's offer does not validate")?;
        msg.acceptance
            .validate(&msg.pod, self.agreed.initiator_object.new)
            .context("accepter's acceptance does not validate")?;
        msg.acceptance
            .validate_guard(&msg.pod, &msg.guard)
            .context("accepter's class guard does not validate")?;

        let mut session = self.deps.build_ctx();
        session
            .builder
            .add_pod(msg.pod)
            .map_err(|err| anyhow!("cannot import accepter's pod: {err}"))?;
        let mut tx = TxBuilder::new_from_commitments(
            &mut session,
            &[self.incoming.old_commitment, self.outgoing.commitment()],
            self.witness.clone(),
        );
        ensure!(
            self.agreed.plan.chain_start() == tx.chain_start,
            "plan and builder disagree on chain start"
        );

        let header = &self.witness.state_header;

        // Event 0: the leg this party receives.
        let scope = tx.begin_action();
        let (received, st_rekey, handle) = tx.rekey_receive(
            &mut session,
            &msg.offer.consumed_side(),
            msg.offer.st_key_erasure.clone(),
            &self.incoming.mid,
            self.new_key.clone(),
        );
        let guard = self
            .deps
            .prove_guard(&mut session, self.want_class, header, st_rekey)?;
        tx.set_guard(handle, guard);
        tx.end_action(scope);
        ensure!(
            received.commitment() == self.incoming_new.commitment(),
            "received state diverges from the planned projection"
        );

        // Event 1: the leg this party gives, recorded against the
        // accepter's Rekey via its revealed class guard.
        let scope = tx.begin_action();
        let handle = tx.rekey_send(&mut session, &self.outgoing, &msg.acceptance.obj_side());
        tx.set_guard(handle, msg.guard);
        tx.end_action(scope);

        let (st_finalized, tx_out, _stats) = tx.finalize(&mut session);
        session
            .builder
            .reveal(&st_finalized)
            .map_err(|err| anyhow!("cannot reveal TxFinalized: {err}"))?;
        ensure!(
            tx_out.dict().commitment() == self.agreed.plan.tx_final(),
            "finalized transaction diverges from the agreed plan"
        );
        let pod = self.deps.prove_session(session.builder)?;

        Ok(SwapOutcome {
            pod,
            tx: tx_out,
            expectation: SwapExpectation {
                received,
                tx_final: self.agreed.plan.tx_final(),
                new_commitments: self.agreed.new_commitments(),
                nullifiers: self.agreed.nullifiers(),
            },
        })
    }
}

// ---------------------------------------------------------------- //
//                             Accepter                             //
// ---------------------------------------------------------------- //

pub struct Accepter {
    deps: SwapDeps,
    give: Dictionary,
    incoming_class: Hash,
    new_key: Value,
}

impl Accepter {
    /// `give` is the full current state of the object this party
    /// gives; `incoming_class` is the invitation's offered class.
    pub fn new(deps: SwapDeps, give: Dictionary, incoming_class: Hash) -> Self {
        Self {
            deps,
            give,
            incoming_class,
            new_key: Value::from(rand_raw_value()),
        }
    }

    /// Open the data round by disclosing the object this party gives.
    pub fn accept(self) -> (AccepterDisclosed, AcceptMsg) {
        let msg = AcceptMsg {
            accepter_object: LegDisclosure::of(&self.give),
        };
        (
            AccepterDisclosed {
                deps: self.deps,
                give: self.give,
                incoming_class: self.incoming_class,
                new_key: self.new_key,
            },
            msg,
        )
    }
}

pub struct AccepterDisclosed {
    deps: SwapDeps,
    give: Dictionary,
    incoming_class: Hash,
    new_key: Value,
}

impl AccepterDisclosed {
    /// Consume the initiator's plan data and answer with the last plan
    /// datum plus this side's `tx_final`, completing the data round.
    pub fn on_plan_data(self, msg: &PlanDataMsg) -> Result<(AccepterPlanned, PlanAckMsg)> {
        msg.initiator_object.validate(self.incoming_class)?;
        let incoming_new = obj_with_key(&msg.initiator_object.mid, self.new_key.clone());
        let agreed = AgreedPlan::derive(
            LegPlan {
                old: self.give.commitment(),
                new: msg.accepter_object_new,
                nullifier: compute_nullifier(&self.give),
            },
            LegPlan {
                old: msg.initiator_object.old_commitment,
                new: incoming_new.commitment(),
                nullifier: msg.initiator_object.nullifier,
            },
            &msg.header,
        )?;
        let reply = PlanAckMsg {
            initiator_object_new: incoming_new.commitment(),
            tx_final: agreed.plan.tx_final(),
        };
        let next = AccepterPlanned {
            deps: self.deps,
            give: self.give,
            incoming_class: self.incoming_class,
            new_key: self.new_key,
            incoming: msg.initiator_object.clone(),
            incoming_new,
            header: msg.header.clone(),
            agreed,
        };
        Ok((next, reply))
    }
}

pub struct AccepterPlanned {
    deps: SwapDeps,
    give: Dictionary,
    incoming_class: Hash,
    new_key: Value,
    incoming: LegDisclosure,
    incoming_new: Dictionary,
    header: StateHeader,
    agreed: AgreedPlan,
}

impl AccepterPlanned {
    /// The state this party will control if the deal lands; see
    /// [`InitiatorNegotiated::projected_received`].
    pub fn projected_received(&self) -> &Dictionary {
        &self.incoming_new
    }

    /// Round 1: validate the initiator's offer, then prove everything
    /// of this party's in one session: its own offer, its acceptance
    /// of the initiator's object at the plan positions, and the class
    /// guard wrapping that acceptance's Rekey.
    pub fn on_offer(self, msg: OfferMsg) -> Result<(AcceptanceMsg, SwapExpectation)> {
        self.deps.check_pod_mode(&msg.pod, "the initiator's")?;
        msg.pod
            .pod
            .verify()
            .context("initiator's pod fails verification")?;
        msg.offer
            .validate(&msg.pod, self.agreed.context, self.incoming.old_commitment)
            .context("initiator's offer does not validate")?;

        let mut session = self.deps.build_ctx();
        session
            .builder
            .add_pod(msg.pod)
            .map_err(|err| anyhow!("cannot import initiator's pod: {err}"))?;
        let my_offer = TransferOffer::prove(&mut session, self.agreed.context, &self.give);
        let (prev_chain, chain) = self.agreed.plan.event_range(1);
        let (acceptance, received) = TransferAcceptance::prove(
            &mut session,
            &msg.offer,
            &self.incoming.mid,
            self.new_key.clone(),
            prev_chain,
            chain,
        );
        ensure!(
            received.commitment() == self.incoming_new.commitment(),
            "received state diverges from the planned projection"
        );
        let guard = self.deps.prove_guard(
            &mut session,
            self.incoming_class,
            &self.header,
            acceptance.st_rekey.clone(),
        )?;
        session
            .builder
            .reveal(&guard)
            .map_err(|err| anyhow!("cannot reveal the class guard: {err}"))?;
        let pod = self.deps.prove_session(session.builder)?;

        let reply = AcceptanceMsg {
            offer: my_offer,
            acceptance,
            guard,
            pod,
        };
        let expectation = SwapExpectation {
            received,
            tx_final: self.agreed.plan.tx_final(),
            new_commitments: self.agreed.new_commitments(),
            nullifiers: self.agreed.nullifiers(),
        };
        Ok((reply, expectation))
    }
}
