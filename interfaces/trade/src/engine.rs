//! Transport-independent initiator and accepter state machines.
//!
//! Each transition consumes its stage and returns the next, preventing
//! message replay and out-of-order protocol steps.

use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, ensure};
use joint_tx::{
    InputState, JointEvent, JointTransaction, TransferAcceptance, TransferOffer, TxPlan,
};
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver},
    frontend::{MainPod, MultiPodBuilder},
    lang::Module,
    middleware::{Hash, MainPodProver, Params, Statement, VDSet, Value, containers::Dictionary},
};
use pod2utils::{macros::BuildContext, map, rand_raw_value};
use txlib::{GroundingWitness, StateHeader, Tx, TxBuilder, compute_nullifier, obj_with_key};

use crate::protocol::{
    ACCEPTER, AcceptMsg, AcceptanceMsg, INITIATOR, LegDisclosure, OfferMsg, PlanAckMsg, PlanDataMsg,
};

/// Return the new-state commitment for transfer action `index`.
fn transfer_new(transaction: &JointTransaction, index: usize) -> Result<Hash> {
    if let Some(JointEvent::Action(leg)) = transaction.events().get(index)
        && let [JointEvent::Transfer { new, .. }] = leg.as_slice()
    {
        return Ok(*new);
    }
    Err(anyhow!(
        "counterparty deal does not have the swap's two-leg shape"
    ))
}

/// Class-guard predicate name and Rekey branch index.
#[derive(Clone, Debug)]
pub struct ClassGuardInfo {
    pub guard_name: String,
    pub rekey_branch: usize,
}

/// Guard metadata keyed by the class hash stored in object `type`.
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

/// Shared proving dependencies.
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

    /// Reject pods produced in a different proving mode.
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

/// Deal independently derived by both engines.
/// Event 0 transfers to the initiator; event 1 transfers to the accepter.
struct AgreedPlan {
    transaction: JointTransaction,
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
        let transaction = JointTransaction::new(
            vec![INITIATOR.to_string(), ACCEPTER.to_string()],
            INITIATOR.to_string(),
            vec![
                InputState {
                    commitment: accepter_object.old,
                    holder: ACCEPTER.to_string(),
                },
                InputState {
                    commitment: initiator_object.old,
                    holder: INITIATOR.to_string(),
                },
            ],
            vec![
                JointEvent::transfer_leg(
                    ACCEPTER,
                    INITIATOR,
                    accepter_object.old,
                    accepter_object.new,
                    accepter_object.nullifier,
                ),
                JointEvent::transfer_leg(
                    INITIATOR,
                    ACCEPTER,
                    initiator_object.old,
                    initiator_object.new,
                    initiator_object.nullifier,
                ),
            ],
        )
        .map_err(|err| anyhow!("deal does not validate: {err}"))?;
        let context = transaction.plan().context(header.hash());
        Ok(Self {
            transaction,
            context,
            accepter_object,
            initiator_object,
        })
    }

    fn plan(&self) -> &TxPlan {
        self.transaction.plan()
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

/// Received state and transaction effect expected by one party.
pub struct SwapExpectation {
    pub received: Dictionary,
    pub tx_final: Hash,
    pub new_commitments: Vec<Hash>,
    pub nullifiers: Vec<Hash>,
}

/// Finalized pod, transaction, and expected effect.
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
    /// Start with the outgoing state and desired class hash.
    pub fn new(deps: SwapDeps, outgoing: Dictionary, want_class: Hash) -> Self {
        Self {
            deps,
            outgoing,
            want_class,
            new_key: Value::from(rand_raw_value()),
        }
    }

    /// Validate the accepter's disclosure and ground both inputs.
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
    /// Projected received state, including its new private key.
    /// Persist this state before exporting an endorsement.
    pub fn projected_received(&self) -> &Dictionary {
        &self.incoming_new
    }

    /// Require the accepter's deal to match, then prove the initiator's offer.
    pub fn on_plan_ack(self, msg: &PlanAckMsg) -> Result<(InitiatorOffered, OfferMsg)> {
        let agreed = AgreedPlan::derive(
            LegPlan {
                old: self.incoming.old_commitment,
                new: self.incoming_new.commitment(),
                nullifier: self.incoming.nullifier,
            },
            LegPlan {
                old: self.outgoing.commitment(),
                new: transfer_new(&msg.transaction, 1)?,
                nullifier: compute_nullifier(&self.outgoing),
            },
            &self.witness.state_header,
        )?;
        ensure!(
            agreed.transaction == msg.transaction,
            "deals diverge: this side derives tx_final {:#}, counterparty {:#}",
            agreed.plan().tx_final(),
            msg.transaction.plan().tx_final()
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
    /// Validate the accepter's session, assemble both legs, and finalize.
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
            self.agreed.plan().chain_start() == tx.chain_start,
            "plan and builder disagree on chain start"
        );

        let header = &self.witness.state_header;

        // Apply the incoming leg.
        let scope = tx.begin_action();
        let (received, st_rekey, handle) = tx.rekey_apply(
            &mut session,
            &msg.offer.spend_facts(),
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

        // Record the outgoing leg against the accepter's class guard.
        let scope = tx.begin_action();
        let handle = tx.rekey_record(&mut session, &self.outgoing, &msg.acceptance.state_facts());
        tx.set_guard(handle, msg.guard);
        tx.end_action(scope);

        let (st_finalized, tx_out, _stats) = tx.finalize(&mut session);
        session
            .builder
            .reveal(&st_finalized)
            .map_err(|err| anyhow!("cannot reveal TxFinalized: {err}"))?;
        ensure!(
            tx_out.dict().commitment() == self.agreed.plan().tx_final(),
            "finalized transaction diverges from the agreed plan"
        );
        let pod = self.deps.prove_session(session.builder)?;

        Ok(SwapOutcome {
            pod,
            tx: tx_out,
            expectation: SwapExpectation {
                received,
                tx_final: self.agreed.plan().tx_final(),
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
    /// Start with the outgoing state and invitation's offered class.
    pub fn new(deps: SwapDeps, give: Dictionary, incoming_class: Hash) -> Self {
        Self {
            deps,
            give,
            incoming_class,
            new_key: Value::from(rand_raw_value()),
        }
    }

    /// Disclose the object this party gives.
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
    /// Complete the plan and return the independently derived deal.
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
            transaction: agreed.transaction.clone(),
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
    /// Projected received state, including its new private key.
    pub fn projected_received(&self) -> &Dictionary {
        &self.incoming_new
    }

    /// Validate the initiator's offer, then prove this side's offer,
    /// acceptance, and class guard in one session.
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
        let (prev_chain, chain) = self.agreed.plan().event_range(1);
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
            tx_final: self.agreed.plan().tx_final(),
            new_commitments: self.agreed.new_commitments(),
            nullifiers: self.agreed.nullifiers(),
        };
        Ok((reply, expectation))
    }
}
