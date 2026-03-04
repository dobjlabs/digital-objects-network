//! High-level utilities for building timelib-aware transactions.
//!
//! These functions wrap the low-level POD2 statement plumbing for timelib
//! predicates (LockObject, UnlockObject, NotExpired, ExecuteOption). Callers
//! should construct a [`BuildContext`] with **both** the txlib and timelib
//! modules loaded, then pass it to every function here.
//!
//! ```ignore
//! let mods = [Arc::clone(&txlib_module), Arc::clone(&time_module)];
//! let mut ctx = BuildContext::new(&mut builder, &mods);
//! let mut tx_builder = TxBuilder::new(&mut ctx, &[], gsr.clone());
//! let locked = lock_object(&mut ctx, &mut tx_builder, obj, duration);
//! let (st, tx) = tx_builder.finalize(&mut ctx);
//! ```

use std::sync::Arc;

use pod2::{
    frontend::MultiPodError,
    lang::{Module, MultiOperationError},
    middleware::{Statement, Value, containers::Dictionary, hash_values},
};
use pod2utils::{macros::BuildContext, op, st_custom};
use txlib::{Object, StateRoot, Tx, TxBuilder};

/// Proves a `StateRoot` predicate for the given state root. Useful when you
/// need to supply a StateRoot statement as input to another predicate.
pub fn prove_state_root(ctx: &mut BuildContext, sr: &StateRoot) -> Statement {
    let tx_nullifiers_hash = hash_values(&[
        Value::from(sr.transactions.clone()),
        Value::from(sr.nullifiers.clone()),
    ]);
    let block_number_gsrs_hash =
        hash_values(&[Value::from(sr.block_number), Value::from(sr.gsrs.clone())]);
    let hash = sr.hash();
    st_custom!(
        ctx,
        StateRoot() = (
            HashOf(tx_nullifiers_hash, sr.transactions, sr.nullifiers),
            HashOf(block_number_gsrs_hash, sr.block_number, sr.gsrs),
            HashOf(hash, tx_nullifiers_hash, block_number_gsrs_hash)
        )
    )
    .unwrap()
}

/// Proves `NotExpired(state, grounding_gsr.block_number, tx_before)`:
/// the grounding GSR's block number is ≤ `state`'s `timeout_block` field.
///
/// `state` must have a `"timeout_block"` entry in `app_layer`. Returns the
/// `NotExpired` statement.
pub fn not_expired(
    ctx: &mut BuildContext,
    grounding_gsr: &StateRoot,
    tx_before: Dictionary,
    state: &Object,
) -> Statement {
    let timeout_val = state
        .app_layer
        .get("timeout_block")
        .cloned()
        .expect("state missing timeout_block field");
    let gsr_hash = grounding_gsr.hash();
    let gsr_block = grounding_gsr.block_number;

    let st_timeout_block = ctx
        .builder
        .priv_op(op!(DictContains(
            state.dict(),
            "timeout_block",
            timeout_val.clone()
        )))
        .unwrap();
    let st_gsr = prove_state_root(ctx, grounding_gsr);
    let st_block_num = st_custom!(
        ctx,
        BlockNumberForStateRoot(block_number = gsr_block) = (st_gsr)
    )
    .unwrap();
    let st_state_root_hash = ctx
        .builder
        .priv_op(op!(DictContains(tx_before, "state_root_hash", gsr_hash)))
        .unwrap();
    let st_gt_eq = ctx
        .builder
        .priv_op(op!(GtEq(timeout_val, gsr_block)))
        .unwrap();

    st_custom!(
        ctx,
        NotExpired() = (st_timeout_block, st_state_root_hash, st_block_num, st_gt_eq)
    )
    .unwrap()
}

/// Locks `obj` by adding a `"locked"` field with value `duration`. Mutates
/// the transaction and proves `LockObject`. Returns the locked object.
pub fn lock_object(
    ctx: &mut BuildContext,
    tx_builder: &mut TxBuilder,
    obj: Object,
    duration: i64,
) -> Object {
    let mut locked = obj.clone();
    locked
        .app_layer
        .insert("locked".to_string(), Value::from(duration));
    let st_mutated = tx_builder.mutate(ctx, locked.clone(), obj.clone());
    st_custom!(
        ctx,
        LockObject() = (
            DictInsert(locked.dict(), obj.dict(), "locked", duration),
            st_mutated
        )
    )
    .unwrap();
    locked
}

/// Unlocks `locked_obj` by proving that at least `locked_obj.locked` blocks
/// have elapsed between `gsr_when_locked` and the transaction's grounding GSR.
/// `gsr_when_locked` must appear in the current state root's `gsrs` array.
/// Returns the unlocked object (the `"locked"` field removed).
pub fn unlock_object(
    ctx: &mut BuildContext,
    time_module: &Module,
    tx_builder: &mut TxBuilder,
    locked_obj: Object,
    tx_when_locked: &Tx,
    gsr_when_locked: &StateRoot,
) -> Object {
    let gsr_current = Arc::clone(&tx_builder.tx.state_root);

    // Find where gsr_when_locked sits in the current state root's gsrs array.
    let target = Value::from(gsr_when_locked.hash());
    let idx = gsr_current
        .gsrs
        .array()
        .iter()
        .position(|v| v == &target)
        .expect("gsr_when_locked not found in current state root's gsrs array")
        as i64;

    let distance = gsr_current.block_number - gsr_when_locked.block_number;

    let mut unlocked = locked_obj.clone();
    unlocked.app_layer.remove("locked");

    let tx_before = tx_builder.tx.dict();
    let st_tx_mutated = tx_builder.mutate(ctx, unlocked.clone(), locked_obj.clone());

    let st_gsr_when_locked_root = prove_state_root(ctx, gsr_when_locked);
    let st_gsr_current_root = prove_state_root(ctx, &gsr_current);

    let st_gsr_has_tx = ctx
        .builder
        .priv_op(op!(SetContains(
            gsr_when_locked.transactions,
            tx_when_locked.dict()
        )))
        .unwrap();
    let st_tx_in_gsr = st_custom!(
        ctx,
        TxInStateRoot() = (st_gsr_when_locked_root.clone(), st_gsr_has_tx)
    )
    .unwrap();

    let st_gsr_has_prior = ctx
        .builder
        .priv_op(op!(ArrayContains(
            gsr_current.gsrs,
            idx,
            gsr_when_locked.hash()
        )))
        .unwrap();
    let st_prior_gsr = st_custom!(
        ctx,
        PriorStateRootInStateRoot() = (st_gsr_current_root.clone(), st_gsr_has_prior)
    )
    .unwrap();
    let st_current_block = st_custom!(
        ctx,
        BlockNumberForStateRoot(block_number = gsr_current.block_number) = (st_gsr_current_root)
    )
    .unwrap();
    let st_when_locked_block = st_custom!(
        ctx,
        BlockNumberForStateRoot(block_number = gsr_when_locked.block_number) =
            (st_gsr_when_locked_root)
    )
    .unwrap();
    let st_distance = st_custom!(
        ctx,
        DistanceBetweenStateRoots(distance = distance) = (
            st_prior_gsr,
            st_current_block,
            st_when_locked_block,
            SumOf(
                gsr_current.block_number,
                gsr_when_locked.block_number,
                distance
            )
        )
    )
    .unwrap();

    let st_tx_before_root = ctx
        .builder
        .priv_op(op!(DictContains(
            tx_before,
            "state_root_hash",
            gsr_current.hash()
        )))
        .unwrap();
    let st_locked_in_tx = ctx
        .builder
        .priv_op(op!(SetContains(
            (&tx_when_locked.dict(), "live"),
            locked_obj.dict()
        )))
        .unwrap();
    let st_gt_eq = ctx
        .builder
        .priv_op(op!(GtEq(distance, (&locked_obj.dict(), "locked"))))
        .unwrap();
    let st_dict_delete = ctx
        .builder
        .priv_op(op!(DictDelete(
            unlocked.dict(),
            locked_obj.dict(),
            "locked"
        )))
        .unwrap();

    apply_predicate(
        ctx,
        time_module,
        "UnlockObject",
        vec![
            st_tx_before_root,
            st_tx_in_gsr,
            st_locked_in_tx,
            st_distance,
            st_gt_eq,
            st_dict_delete,
            st_tx_mutated,
        ],
    );

    unlocked
}

/// Applies a timelib predicate with more than 5 clauses via `apply_predicate_with`.
fn apply_predicate(ctx: &mut BuildContext, module: &Module, name: &str, stmts: Vec<Statement>) {
    struct ApplyErr(MultiPodError);
    impl From<MultiOperationError> for ApplyErr {
        fn from(e: MultiOperationError) -> Self {
            ApplyErr(MultiPodError::Custom(e.to_string()))
        }
    }
    module
        .apply_predicate_with(
            name,
            stmts,
            false,
            |is_public, op| -> Result<Statement, ApplyErr> {
                if is_public {
                    ctx.builder.pub_op(op).map_err(ApplyErr)
                } else {
                    ctx.builder.priv_op(op).map_err(ApplyErr)
                }
            },
        )
        .map_err(|ApplyErr(e)| e)
        .unwrap();
}
