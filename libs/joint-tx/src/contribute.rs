//! What one party proves about objects it holds so that another party
//! can assemble a transaction over them.
//!
//! Each type here is produced by an object's holder with only a
//! [`BuildContext`], never a `TxBuilder`: contributing to someone
//! else's transaction does not mean building one. The assembler
//! consumes them through [`txlib::TxBuilder::mutate_joint`] and
//! [`txlib::TxBuilder::rekey_receive`], as the plain payloads the
//! accessors below hand over.

use std::sync::LazyLock;

use pod2::{
    frontend::MainPod,
    middleware::{
        CustomPredicateRef, EMPTY_VALUE, Hash, Statement, Value, ValueRef, containers::Dictionary,
    },
};
use pod2utils::{macros::BuildContext, op};
use serde::{Deserialize, Serialize};

use txlib::{
    ConsumedSide, ContributedOpenings, ContributedSpend, ObjSide, STABLE_IDENTIFIER_FIELD,
    erased_key_state, obj_with_key, object_stable_identifier, object_type, prove_endorse_spend,
    prove_tx_mutate,
};

// ============================================================================
// Contributions to a jointly-assembled transaction
// ============================================================================
//
// A transaction can be assembled by a party that does not hold every
// object it touches. Anything a transaction needs about an object state
// falls into three groups:
//
//   1. Its commitment, which the chain hashes and the live/nullifier
//      set updates need. Public, no secret involved.
//   2. Openings of specific fields (`type`, `stable_identifier`), which
//      TxMutate needs. Provable only by a party holding the dict, and
//      revealing nothing but the field values.
//   3. Its nullifier and spend endorsement, provable only by a party
//      holding the object's `key`.
//
// The types below carry (2) and (3) across a pod boundary, so the
// assembler never needs a counterparty's key. The statements must be
// public in the contributing party's pod for the assembler to use them
// as premises, so the `prove` constructors reveal them.

/// Field openings for one object state, contributed by the party that
/// holds it to an assembler that does not.
///
/// Reveals the object's commitment, type, and stable identifier, and
/// nothing else: `DictContains` exposes only the field it names, never
/// `key`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectOpenings {
    pub commitment: Hash,
    pub type_value: Value,
    pub stable_identifier: Value,
    /// `DictContains(obj, "type", type_value)`
    pub st_type: Statement,
    /// `DictContains(obj, "stable_identifier", stable_identifier)`
    pub st_stable_identifier: Statement,
}

impl ObjectOpenings {
    /// Prove the openings an assembler needs for `obj`, revealing them
    /// so they survive into this party's pod.
    pub fn prove(ctx: &mut BuildContext, obj: &Dictionary) -> Self {
        let type_value = object_type(obj);
        let stable_identifier = object_stable_identifier(obj);
        let st_type = ctx
            .builder
            .pub_op(op!(DictContains(obj, "type", type_value.clone())))
            .unwrap();
        let st_stable_identifier = ctx
            .builder
            .pub_op(op!(DictContains(
                obj,
                STABLE_IDENTIFIER_FIELD,
                stable_identifier.clone()
            )))
            .unwrap();
        Self {
            commitment: obj.commitment(),
            type_value,
            stable_identifier,
            st_type,
            st_stable_identifier,
        }
    }

    /// The mutation side these openings supply, for handing to
    /// [`txlib::TxBuilder::mutate_joint`].
    pub fn side(&self) -> ObjSide {
        ObjSide::Contributed(Box::new(self.payload()))
    }

    fn payload(&self) -> ContributedOpenings {
        ContributedOpenings {
            commitment: self.commitment,
            type_value: self.type_value.clone(),
            stable_identifier: self.stable_identifier.clone(),
            st_type: self.st_type.clone(),
            st_stable_identifier: self.st_stable_identifier.clone(),
        }
    }
}

/// An owner's authorization to spend one object state inside one
/// specific transaction. See `EndorseSpend` in txlib.podlang for why
/// only the owner can produce it and why it binds one transaction.
///
/// The owner needs nothing but the context *commitment* to produce it:
/// not the transaction's live or nullifier sets, only the negotiated
/// value they hash to.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendAuthorization {
    /// Nullifier of the consumed state. The assembler needs the value
    /// to fold into the transaction's nullifier set, and cannot derive
    /// it itself.
    pub nullifier: Hash,
    /// `EndorseSpend(context, nullifier, old)`
    pub st_endorsement: Statement,
}

impl SpendAuthorization {
    /// Endorse spending `old` in the transaction whose context commits
    /// to `context`, revealing the endorsement so it survives into this
    /// party's pod.
    ///
    /// Build `context` with [`txlib::context_commitment`] from the negotiated
    /// state root and tx_final.
    pub fn prove(ctx: &mut BuildContext, context: Hash, old: &Dictionary) -> Self {
        let (nullifier, st_endorsement) = prove_endorse_spend(ctx, true, Value::from(context), old);
        Self {
            nullifier,
            st_endorsement,
        }
    }
}

/// Everything the current owner of an object contributes to a transfer
/// assembled by the receiving party.
///
/// These three artifacts always travel together, are always about the
/// same object and the same transaction context, and none of them
/// reveals the owner's key. Producing them is the whole of the sender's
/// side of a transfer; [`txlib::TxBuilder::rekey_receive`] consumes them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferOffer {
    pub openings: ObjectOpenings,
    /// `DictUpdate(old, "key", {}, mid)`: the key-erasing half of the
    /// transfer, which only the current owner can prove because
    /// updating `old` requires opening it. Context-free, so it survives
    /// re-grounding and renegotiation; only the endorsement expires.
    pub st_key_erasure: Statement,
    pub auth: SpendAuthorization,
}

impl TransferOffer {
    /// The consumed side this offer authorizes, for handing to
    /// [`txlib::TxBuilder::mutate_joint`].
    pub fn consumed_side(&self) -> ConsumedSide {
        ConsumedSide::Contributed(Box::new(ContributedSpend {
            openings: self.openings.payload(),
            nullifier: self.auth.nullifier,
            endorsement: self.auth.st_endorsement.clone(),
        }))
    }

    /// Prove the sender's whole side of transferring `old` inside the
    /// transaction committing to `context`, revealing each statement so
    /// it survives into this party's pod.
    pub fn prove(ctx: &mut BuildContext, context: Hash, old: &Dictionary) -> Self {
        let mid = erased_key_state(old);
        Self {
            openings: ObjectOpenings::prove(ctx, old),
            st_key_erasure: ctx
                .builder
                .pub_op(op!(DictUpdate(old, "key", EMPTY_VALUE, mid)))
                .unwrap(),
            auth: SpendAuthorization::prove(ctx, context, old),
        }
    }
}

/// The receiving party's side of a transfer, when the *sender* is the
/// one assembling the transaction.
///
/// `Rekey` keeps the receiver's new key in a private wildcard, so an
/// assembler applying that predicate would have to bind the wildcard,
/// which means knowing the key. Only the receiver can prove `Rekey`.
/// When the sender assembles, the receiver therefore proves the whole
/// transfer action here and the sender records the event against it.
///
/// This is the mirror of [`TransferOffer`], and the two directions are
/// not interchangeable: a transfer is always proven by its receiver and
/// endorsed by its sender, whichever of them holds the builder.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferAcceptance {
    /// Openings of the received state, which the assembler needs for its
    /// own `TxMutate` clauses and cannot prove itself.
    pub openings: ObjectOpenings,
    /// `Rekey(new, chain_start, chain_end, type)`, private to the
    /// receiver's pod. The receiver wraps it in the class guard's
    /// transfer branch (class-specific, so caller code) and reveals
    /// *that*; the guard statement is what crosses to the assembler.
    pub st_rekey: Statement,
}

impl TransferAcceptance {
    /// Accept `offer` by taking control of the offered object under
    /// `new_key`, proving the transfer action at the agreed chain
    /// position.
    ///
    /// `mid` is the erased-key state, reconstructed from the sender's
    /// disclosed non-key fields via [`txlib::erased_key_state`];
    /// checking it against the commitment in the offer's key-erasing
    /// statement is what verifies that disclosure. The chain positions
    /// are derivable from the agreed event sequence, so they are inputs
    /// rather than something this party chooses.
    ///
    /// Returns the acceptance and the new state, which stays private to
    /// this party apart from its commitment and the openings above.
    pub fn prove(
        ctx: &mut BuildContext,
        offer: &TransferOffer,
        mid: &Dictionary,
        new_key: Value,
        prev_chain: Hash,
        chain: Hash,
    ) -> (Self, Dictionary) {
        let new = obj_with_key(mid, new_key.clone());
        let openings = ObjectOpenings::prove(ctx, &new);
        let st_set = ctx
            .builder
            .priv_op(op!(DictUpdate(mid, "key", new_key, new)))
            .unwrap();
        let st_tx_mutate = prove_tx_mutate(
            ctx,
            prev_chain,
            chain,
            &offer.openings.side(),
            &ObjSide::Held(new.clone()),
        );
        let st_rekey = ctx
            .apply_custom_pred_simple(
                false,
                "Rekey",
                vec![offer.st_key_erasure.clone(), st_set, st_tx_mutate],
            )
            .unwrap();
        (Self { openings, st_rekey }, new)
    }

    /// The mutation side this acceptance provides, for handing to
    /// [`txlib::TxBuilder::rekey_send`].
    pub fn obj_side(&self) -> ObjSide {
        self.openings.side()
    }
}

// ============================================================================
// Receiving-side validation
// ============================================================================
//
// A contribution that crosses a process boundary arrives as data plus
// the pod that proves it. Validation rebuilds each expected statement
// from the plain fields and the negotiated plan data, then checks that
// the received statement equals it and is among the pod's public
// statements, so a mismatched bundle fails at receipt with a readable
// error instead of as an opaque solver failure at assembly time.
// Soundness never depends on these checks: the proof does.

/// This build's `EndorseSpend` predicate, for pinning a received
/// endorsement to the exact batch: a peer on a different txlib build
/// fails here with both batch ids named, instead of at the solver.
static ENDORSE_SPEND: LazyLock<CustomPredicateRef> = LazyLock::new(|| {
    txlib::predicates::module()
        .predicate_ref_by_name("EndorseSpend")
        .expect("txlib module declares EndorseSpend")
});

impl ObjectOpenings {
    /// Validate received openings against the pod that must prove them
    /// and the commitment the plan says they are about.
    pub(crate) fn validate(&self, pod: &MainPod, object: Hash) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.commitment == object,
            "openings are for {}, expected {object}",
            self.commitment
        );
        expect_public(pod, &self.st_type, "type opening")?;
        check_opening(
            &self.st_type,
            self.commitment,
            "type",
            &self.type_value,
            "type opening",
        )?;
        expect_public(pod, &self.st_stable_identifier, "stable identifier opening")?;
        check_opening(
            &self.st_stable_identifier,
            self.commitment,
            STABLE_IDENTIFIER_FIELD,
            &self.stable_identifier,
            "stable identifier opening",
        )?;
        Ok(())
    }
}

impl SpendAuthorization {
    /// Validate a received authorization against the pod, the
    /// negotiated context, and the consumed state's commitment.
    pub(crate) fn validate(&self, pod: &MainPod, context: Hash, old: Hash) -> anyhow::Result<()> {
        expect_public(pod, &self.st_endorsement, "spend endorsement")?;
        let Statement::Custom(predicate, _) = &self.st_endorsement else {
            anyhow::bail!("spend endorsement is not a custom-predicate statement");
        };
        anyhow::ensure!(
            predicate == &*ENDORSE_SPEND,
            "spend endorsement applies {} from batch {}, expected this build's EndorseSpend (batch {})",
            predicate.predicate().name,
            predicate.batch.id(),
            ENDORSE_SPEND.batch.id()
        );
        let expected = Statement::Custom(
            ENDORSE_SPEND.clone(),
            vec![
                ValueRef::Literal(Value::from(context)),
                ValueRef::Literal(Value::from(self.nullifier)),
                ValueRef::Literal(Value::from(old)),
            ],
        );
        anyhow::ensure!(
            self.st_endorsement == expected,
            "spend endorsement is {}, expected {expected}",
            self.st_endorsement
        );
        Ok(())
    }
}

impl TransferOffer {
    /// Validate a received offer: the openings, the key-erasing
    /// update, and the spend authorization, against the pod, the
    /// negotiated context, and the old state's commitment. The
    /// erased-key commitment is deliberately unchecked: the party that
    /// reconstructs `mid` from the disclosed fields checks it by
    /// commitment match, and `Rekey` cannot be proven otherwise.
    pub fn validate(&self, pod: &MainPod, context: Hash, old: Hash) -> anyhow::Result<()> {
        self.openings.validate(pod, old)?;
        expect_public(pod, &self.st_key_erasure, "key erasure")?;
        // Statement arg order is (old_root, key, value, new_root); the
        // enum's field comments in pod2 say otherwise and are stale.
        let Statement::ContainerUpdate(old_root, key, erased, _mid) = &self.st_key_erasure else {
            anyhow::bail!("key erasure is not a ContainerUpdate statement");
        };
        ensure_literal(old_root, &Value::from(old), "key erasure's subject")?;
        ensure_literal(key, &Value::from("key"), "key erasure's field")?;
        ensure_literal(
            erased,
            &Value::from(EMPTY_VALUE),
            "key erasure's written value",
        )?;
        self.auth.validate(pod, context, old)?;
        Ok(())
    }
}

impl TransferAcceptance {
    /// Validate a received acceptance: the openings of the new state,
    /// against the commitment the plan projects for it. `st_rekey` is
    /// private to the receiver's pod and deliberately unchecked here;
    /// the class guard wrapping it is the acceptance's public face,
    /// checked by [`TransferAcceptance::validate_guard`].
    pub fn validate(&self, pod: &MainPod, new: Hash) -> anyhow::Result<()> {
        self.openings.validate(pod, new)
    }

    /// Validate the class guard that crosses alongside an acceptance:
    /// it must be among the pod's public statements and be the
    /// transferred class's own guard, whose predicate hash is the
    /// object's `type`.
    pub fn validate_guard(&self, pod: &MainPod, guard: &Statement) -> anyhow::Result<()> {
        expect_public(pod, guard, "class guard")?;
        let guard_type = Value::from(guard.predicate().hash());
        anyhow::ensure!(
            guard_type == self.openings.type_value,
            "class guard hashes to {guard_type}, the transferred object's type is {}",
            self.openings.type_value
        );
        Ok(())
    }
}

fn expect_public(pod: &MainPod, statement: &Statement, what: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        pod.public_statements.contains(statement),
        "{what} is not among the pod's public statements"
    );
    Ok(())
}

fn ensure_literal(arg: &ValueRef, expected: &Value, what: &str) -> anyhow::Result<()> {
    let ValueRef::Literal(value) = arg else {
        anyhow::bail!("{what} is anchored, expected a literal");
    };
    anyhow::ensure!(value == expected, "{what} is {value}, expected {expected}");
    Ok(())
}

fn check_opening(
    actual: &Statement,
    dict: Hash,
    field: &str,
    value: &Value,
    what: &str,
) -> anyhow::Result<()> {
    let expected = Statement::Contains(
        ValueRef::Literal(Value::from(dict)),
        ValueRef::Literal(Value::from(field)),
        ValueRef::Literal(value.clone()),
    );
    anyhow::ensure!(
        actual == &expected,
        "{what} is {actual}, expected {expected}"
    );
    Ok(())
}
