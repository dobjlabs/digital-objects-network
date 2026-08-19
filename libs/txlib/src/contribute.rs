//! What one party proves about objects it holds so that another party
//! can assemble a transaction over them.
//!
//! Each type here is produced by an object's holder with only a
//! [`BuildContext`], never a `TxBuilder`: contributing to someone
//! else's transaction does not mean building one. The assembler
//! consumes them through [`crate::TxBuilder::mutate_joint`] and
//! [`crate::TxBuilder::rekey_receive`].

use pod2::middleware::{EMPTY_VALUE, Hash, Statement, Value, containers::Dictionary};
use pod2utils::{macros::BuildContext, op};

use crate::{
    ConsumedSide, ContributedSpend, ObjSide, STABLE_IDENTIFIER_FIELD, erased_key_state,
    obj_with_key, object_stable_identifier, object_type, prove_endorse_spend, prove_tx_mutate,
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
#[derive(Clone, Debug)]
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
}

/// An owner's authorization to spend one object state inside one
/// specific transaction. See `EndorseSpend` in txlib.podlang for why
/// only the owner can produce it and why it binds one transaction.
///
/// The owner needs nothing but the context *commitment* to produce it:
/// not the transaction's live or nullifier sets, only the negotiated
/// value they hash to.
#[derive(Clone, Debug)]
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
    /// Build `context` with [`crate::context_commitment`] from the negotiated
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
/// side of a transfer; [`crate::TxBuilder::rekey_receive`] consumes them.
#[derive(Clone, Debug)]
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
    /// [`crate::TxBuilder::mutate_joint`].
    pub fn consumed_side(&self) -> ConsumedSide {
        ConsumedSide::Contributed(Box::new(ContributedSpend {
            openings: self.openings.clone(),
            auth: self.auth.clone(),
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
#[derive(Clone, Debug)]
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
    /// disclosed non-key fields via [`crate::erased_key_state`];
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
            &ObjSide::Contributed(Box::new(offer.openings.clone())),
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
    /// [`crate::TxBuilder::mutate_joint`].
    pub fn obj_side(&self) -> ObjSide {
        ObjSide::Contributed(Box::new(self.openings.clone()))
    }
}
