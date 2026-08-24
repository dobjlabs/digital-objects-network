//! Proof bundles for states unavailable to the transaction assembler.
//!
//! A state holder produces these values with a [`BuildContext`], without
//! constructing a transaction. Their accessors provide the inputs expected
//! by [`txlib::TxBuilder::mutate`] and [`txlib::TxBuilder::rekey_apply`].

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
    STABLE_IDENTIFIER_FIELD, SpendFacts, SpendStatements, StateFacts, StateOpenings,
    erased_key_state, obj_with_key, object_stable_identifier, object_type, prove_endorse_spend,
    prove_tx_mutate,
};

// ============================================================================
// Contributions to a jointly-assembled transaction
// ============================================================================
//
// The assembler needs three kinds of state data:
//
//   1. A public commitment for chain and set updates.
//   2. `type` and `stable_identifier` openings for TxMutate.
//   3. A nullifier and spend endorsement derived from the key.
//
// Contributions carry (2) and (3) as public statements in the producer's
// pod. The key never crosses the boundary.

/// Public `type` and `stable_identifier` openings for one state.
/// The statements reveal no other fields.
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
    /// Prove the openings an assembler needs for `obj`.
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

    /// These openings as the facts [`txlib::TxBuilder::mutate`] reads
    /// about the state.
    pub fn state_facts(&self) -> StateFacts {
        StateFacts::Statements(Box::new(self.payload()))
    }

    fn payload(&self) -> StateOpenings {
        StateOpenings {
            commitment: self.commitment,
            type_value: self.type_value.clone(),
            stable_identifier: self.stable_identifier.clone(),
            st_type: self.st_type.clone(),
            st_stable_identifier: self.st_stable_identifier.clone(),
        }
    }
}

/// Authorization to spend one state in one transaction context.
///
/// Producing it requires the state's key and the context commitment.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendAuthorization {
    pub nullifier: Hash,
    /// `EndorseSpend(context, nullifier, old)`
    pub st_endorsement: Statement,
}

impl SpendAuthorization {
    /// Endorse spending `old` in `context`.
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

/// A sender's complete contribution to a receiver-assembled transfer.
///
/// The openings, key-erasure statement, and authorization refer to one
/// state and context without revealing its key.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferOffer {
    pub openings: ObjectOpenings,
    /// `DictUpdate(old, "key", {}, mid)`. This statement is context-free;
    /// only the authorization expires when the context changes.
    pub st_key_erasure: Statement,
    pub auth: SpendAuthorization,
}

impl TransferOffer {
    /// The facts [`txlib::TxBuilder::mutate`] reads about the consumed
    /// state, including this offer's authorization to spend it.
    pub fn spend_facts(&self) -> SpendFacts {
        SpendFacts::Statements(Box::new(SpendStatements {
            openings: self.openings.payload(),
            nullifier: self.auth.nullifier,
            endorsement: self.auth.st_endorsement.clone(),
        }))
    }

    /// Prove the complete offer for `old` in `context`.
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

/// A receiver's contribution to a sender-assembled transfer.
///
/// The receiver proves `Rekey` because its new key is a private wildcard.
/// The sender then records the event using the exported class guard.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferAcceptance {
    /// Openings the assembler needs for `TxMutate`.
    pub openings: ObjectOpenings,
    /// Private `Rekey(new, chain_start, chain_end, type)` statement.
    pub st_rekey: Statement,
}

impl TransferAcceptance {
    /// Accept `offer` under `new_key` at the plan's chain positions.
    ///
    /// `mid` is reconstructed from disclosed non-key fields; its commitment
    /// must match the offer's key-erasure statement. Returns the acceptance
    /// and new state.
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
            &offer.openings.state_facts(),
            &StateFacts::Dict(new.clone()),
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

    /// The facts [`txlib::TxBuilder::rekey_record`] reads about the
    /// received state.
    pub fn state_facts(&self) -> StateFacts {
        self.openings.state_facts()
    }
}

// ============================================================================
// Receiving-side validation
// ============================================================================
//
// Validation reconstructs expected statements from bundle and plan data,
// then checks them against the pod's public statements. This reports wire
// mismatches early; proof soundness does not depend on these checks.

/// This build's `EndorseSpend`, used to reject incompatible txlib batches.
static ENDORSE_SPEND: LazyLock<CustomPredicateRef> = LazyLock::new(|| {
    txlib::predicates::module()
        .predicate_ref_by_name("EndorseSpend")
        .expect("txlib module declares EndorseSpend")
});

impl ObjectOpenings {
    /// Validate openings against their pod and planned commitment.
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
    /// Validate authorization against its pod, context, and state.
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
    /// Validate an offer against its pod, context, and old-state commitment.
    /// The receiver validates the erased-key commitment when reconstructing
    /// `mid`.
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
    /// Validate the new-state openings against the pod and plan commitment.
    /// [`TransferAcceptance::validate_guard`] checks the public guard.
    pub fn validate(&self, pod: &MainPod, new: Hash) -> anyhow::Result<()> {
        self.openings.validate(pod, new)
    }

    /// Require the exported guard to be public and to hash to the state type.
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
