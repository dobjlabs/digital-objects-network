//! High-level utilities for building timelib-aware transactions.
//!
//! These functions wrap the low-level POD2 statement plumbing for timelib
//! predicates (LockObject, UnlockObject, NotExpired, SetExpiry). Callers
//! should construct a [`BuildContext`] with **both** the txlib and timelib
//! modules loaded, then pass it to every function here.
//!
//! ```ignore
//! let mods = [Arc::clone(&txlib_module), Arc::clone(&time_module)];
//! let mut ctx = BuildContext::new(&mut builder, &mods);
//! let mut tx_builder = TxBuilder::new(&mut ctx, &[], gsr.clone());
//! let locked = lock_object(&mut ctx, obj, duration)?;
//! tx_builder.mutate(&mut ctx, locked.clone(), obj);
//! let (st, tx) = tx_builder.finalize(&mut ctx);
//! ```

use anyhow::ensure;
use pod2::{
    frontend::MultiPodError,
    lang::{Module, MultiOperationError},
    middleware::{Key, Statement, Value, containers::Dictionary, hash_values},
};
use pod2utils::{macros::BuildContext, op, st_custom};
use txlib::{StateRoot, Tx};

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
/// Returns `Err` if `state` has no `"timeout_block"` field or if the object
/// has already expired at the grounding GSR's block.
pub fn not_expired(
    ctx: &mut BuildContext,
    grounding_gsr: &StateRoot,
    tx_before: Dictionary,
    state: Dictionary,
) -> anyhow::Result<Statement> {
    let timeout_val = state
        .get(&Key::from("timeout_block"))?
        .ok_or_else(|| anyhow::anyhow!("state missing timeout_block field"))?;
    let timeout_block = timeout_val
        .as_int()
        .ok_or_else(|| anyhow::anyhow!("timeout_block is not an integer"))?;
    let gsr_block = grounding_gsr.block_number;
    ensure!(
        gsr_block <= timeout_block,
        "object expired: grounding block {} > timeout_block {}",
        gsr_block,
        timeout_block,
    );

    let gsr_hash = grounding_gsr.hash();

    let st_timeout_block = ctx
        .builder
        .priv_op(op!(DictContains(
            state,
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

    Ok(st_custom!(
        ctx,
        NotExpired() = (st_timeout_block, st_state_root_hash, st_block_num, st_gt_eq)
    )
    .unwrap())
}

/// Proves `SetExpiry(new_obj, obj, tx_before, expiry_block)`: the grounding
/// GSR's block number is strictly less than `expiry_block`, and `new_obj` is
/// `obj` with `"timeout_block"` inserted.
///
/// Returns `Err` if `expiry_block` is not strictly greater than the grounding
/// GSR's block number.
///
/// Capture `tx_before = tx_builder.tx.dict()` before calling this, then call
/// `tx_builder.mutate(ctx, new_obj, obj)` afterwards to record the mutation.
pub fn set_expiry(
    ctx: &mut BuildContext,
    grounding_gsr: &StateRoot,
    tx_before: Dictionary,
    obj: Dictionary,
    expiry_block: i64,
) -> anyhow::Result<Dictionary> {
    ensure!(
        expiry_block > grounding_gsr.block_number,
        "expiry_block {} must be greater than grounding block {}",
        expiry_block,
        grounding_gsr.block_number,
    );

    let mut obj_with_expiry = obj.clone();
    obj_with_expiry.insert(&Key::from("timeout_block"), &Value::from(expiry_block))?;

    let st_sr_hash = ctx
        .builder
        .priv_op(op!(DictContains(
            tx_before,
            "state_root_hash",
            grounding_gsr.hash()
        )))
        .unwrap();
    let st_gsr = prove_state_root(ctx, grounding_gsr);
    let st_block_num = st_custom!(
        ctx,
        BlockNumberForStateRoot(block_number = grounding_gsr.block_number) = (st_gsr)
    )
    .unwrap();
    let st_gt = ctx
        .builder
        .priv_op(op!(Gt(expiry_block, grounding_gsr.block_number)))
        .unwrap();
    let st_dict_insert = ctx
        .builder
        .priv_op(op!(DictInsert(
            obj_with_expiry,
            obj,
            "timeout_block",
            expiry_block
        )))
        .unwrap();
    st_custom!(
        ctx,
        SetExpiry() = (st_sr_hash, st_block_num, st_gt, st_dict_insert)
    )
    .unwrap();

    Ok(obj_with_expiry)
}

/// Proves `LockObject(new_obj, obj, duration)`: `new_obj` is `obj` with a
/// `"locked"` field added. Returns the locked object.
///
/// Returns `Err` if `duration` is not positive.
///
/// Call `tx_builder.mutate(ctx, locked, obj)` afterwards to record the mutation.
pub fn lock_object(
    ctx: &mut BuildContext,
    obj: Dictionary,
    duration: i64,
) -> anyhow::Result<Dictionary> {
    ensure!(
        duration > 0,
        "lock duration must be positive, got {}",
        duration
    );
    let mut locked = obj.clone();
    locked.insert(&Key::from("locked"), &Value::from(duration))?;
    st_custom!(
        ctx,
        LockObject() = (DictInsert(locked, obj, "locked", duration))
    )
    .unwrap();
    Ok(locked)
}

/// Proves `UnlockObject(new_obj, locked_obj, tx_before, ...)`: at least
/// `locked_obj.locked` blocks have elapsed between `gsr_when_locked` and
/// `grounding_gsr`. `gsr_when_locked` must appear in `grounding_gsr.gsrs`.
/// Returns the unlocked object (the `"locked"` field removed).
///
/// Returns `Err` if `locked_obj` has no `"locked"` field, if `gsr_when_locked`
/// is not in `grounding_gsr.gsrs`, or if insufficient blocks have elapsed.
///
/// Capture `tx_before = tx_builder.tx.dict()` before calling this, then call
/// `tx_builder.mutate(ctx, unlocked, locked_obj)` afterwards.
pub fn unlock_object(
    ctx: &mut BuildContext,
    time_module: &Module,
    grounding_gsr: &StateRoot,
    tx_before: Dictionary,
    locked_obj: Dictionary,
    tx_when_locked: &Tx,
    gsr_when_locked: &StateRoot,
) -> anyhow::Result<Dictionary> {
    let lock_duration = locked_obj
        .get(&Key::from("locked"))?
        .ok_or_else(|| anyhow::anyhow!("locked_obj missing 'locked' field"))?;
    let lock_duration = lock_duration
        .as_int()
        .ok_or_else(|| anyhow::anyhow!("'locked' field is not an integer"))?;

    // Find where gsr_when_locked sits in the current state root's gsrs array.
    let target = Value::from(gsr_when_locked.hash());
    let mut idx = None;
    for entry in grounding_gsr.gsrs.iter() {
        let (i, v) = entry?;
        if v == target {
            idx = Some(i);
            break;
        }
    }
    let idx =
        idx.ok_or_else(|| anyhow::anyhow!("gsr_when_locked not found in grounding_gsr.gsrs"))?;

    let distance = grounding_gsr.block_number - gsr_when_locked.block_number;
    ensure!(
        distance >= lock_duration,
        "cannot unlock: only {} blocks elapsed, need {}",
        distance,
        lock_duration,
    );

    let mut unlocked = locked_obj.clone();
    unlocked.delete(&Key::from("locked"))?;

    let st_gsr_when_locked_root = prove_state_root(ctx, gsr_when_locked);
    let st_grounding_gsr_root = prove_state_root(ctx, grounding_gsr);

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
            grounding_gsr.gsrs,
            idx as i64,
            gsr_when_locked.hash()
        )))
        .unwrap();
    let st_prior_gsr = st_custom!(
        ctx,
        PriorStateRootInStateRoot() = (st_grounding_gsr_root.clone(), st_gsr_has_prior)
    )
    .unwrap();
    let st_current_block = st_custom!(
        ctx,
        BlockNumberForStateRoot(block_number = grounding_gsr.block_number) =
            (st_grounding_gsr_root)
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
                grounding_gsr.block_number,
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
            grounding_gsr.hash()
        )))
        .unwrap();
    let st_locked_in_tx = ctx
        .builder
        .priv_op(op!(SetContains(
            (&tx_when_locked.dict(), "live"),
            locked_obj
        )))
        .unwrap();
    let st_gt_eq = ctx
        .builder
        .priv_op(op!(GtEq(distance, (&locked_obj, "locked"))))
        .unwrap();
    let st_dict_delete = ctx
        .builder
        .priv_op(op!(DictDelete(unlocked, locked_obj, "locked")))
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
        ],
    );

    Ok(unlocked)
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
