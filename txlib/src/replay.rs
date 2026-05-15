//! Replay circuit construction for `TxBuilder::finalize`.
//!
//! At finalize time the recorded events form a tree (actions containing
//! events and sub-actions). This module walks that tree and builds the
//! POD2 predicate statements that prove each event's hash step, update
//! the live/nullifier sets, and dispatch each event to its object-type
//! guard.
//!
//! Only `build_replay_contents` is `pub(crate)`; every other function
//! here is a private helper it delegates to.

use pod2::{
    frontend::Operation,
    middleware::{
        Hash, Statement, Value,
        containers::{Dictionary, Set},
    },
};
use pod2utils::{dict, macros::BuildContext, map, op, st_custom};

use crate::{
    ChainEvent, OBJECT_NULLIFIER_VERSION, TxStats, build_tx, object_key_hash,
    object_nullifier_from_key_hash, record, tx_with,
};

/// Walk the top-level event list and build a `ReplayActions` statement.
/// Every top-level event must be `ChainEvent::Action` -- the prover
/// API enforces this by construction, and we panic here if not.
/// `events` is guaranteed non-empty (TxBuilder::finalize asserts).
/// Callers: `TxBuilder::finalize`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_replay_actions(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    events: &[ChainEvent],
    chain: Hash,
    live: &Set,
    nullifiers: &Set,
    chain_start: Hash,
    chain_end: Hash,
) -> (Statement, Hash, Set, Set) {
    assert!(
        !events.is_empty(),
        "build_replay_actions: empty event list (empty Tx is forbidden)"
    );

    if events.len() == 1 {
        // Single action: no step wrapping.
        let (st_action, c, l, n) = build_top_level_action(
            ctx,
            stats,
            &events[0],
            chain,
            live,
            nullifiers,
            chain_start,
            chain_end,
        );
        let st = st_custom!(ctx, ReplayActions() = (st_action, Statement::None)).unwrap();
        record(stats, "ReplayActions");
        return (st, c, l, n);
    }

    // Step: first action + recursive tail.
    let (first, rest) = events.split_first().unwrap();
    let (st_action, c, l, n) = build_top_level_action(
        ctx,
        stats,
        first,
        chain,
        live,
        nullifiers,
        chain_start,
        chain_end,
    );
    let (st_rest, c2, l2, n2) =
        build_replay_actions(ctx, stats, rest, c, &l, &n, chain_start, chain_end);
    let st_step = st_custom!(ctx, ReplayActionsStep() = (st_action, st_rest)).unwrap();
    record(stats, "ReplayActionsStep");
    let st = st_custom!(ctx, ReplayActions() = (Statement::None, st_step)).unwrap();
    record(stats, "ReplayActions");
    (st, c2, l2, n2)
}

/// Extract an action from a top-level `ChainEvent` and delegate to
/// `build_replay_action`. Panics on non-action variants.
#[allow(clippy::too_many_arguments)]
fn build_top_level_action(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    event: &ChainEvent,
    chain: Hash,
    live: &Set,
    nullifiers: &Set,
    chain_start: Hash,
    chain_end: Hash,
) -> (Statement, Hash, Set, Set) {
    match event {
        ChainEvent::Action {
            chain_after,
            contents,
        } => {
            let (st, new_live, new_null) = build_replay_action(
                ctx,
                stats,
                contents,
                chain,
                live,
                nullifiers,
                chain_start,
                chain_end,
                *chain_after,
            );
            (st, *chain_after, new_live, new_null)
        }
        ChainEvent::Insert { .. } | ChainEvent::Mutate { .. } | ChainEvent::Delete { .. } => {
            panic!(
                "top-level event must be a ChainEvent::Action (bare events are only allowed inside an action scope)"
            );
        }
    }
}

/// Recursively build `ReplayContents` for a list of events. `events`
/// is guaranteed non-empty (TxBuilder asserts on `end_action`). The
/// K=1 case lands on `ReplayElement`; K>=2 dispatches to the
/// type-specialized `ReplayContentsStep<X>` for the head.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_replay_contents(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    events: &[ChainEvent],
    chain: Hash,
    live: &Set,
    nullifiers: &Set,
    chain_start: Hash,
    chain_end: Hash,
) -> (Statement, Hash, Set, Set) {
    assert!(
        !events.is_empty(),
        "build_replay_contents: empty event list (empty action scope is forbidden)"
    );

    if events.len() == 1 {
        let (st_elem, c, l, n) = build_replay_element(
            ctx,
            stats,
            &events[0],
            chain,
            live,
            nullifiers,
            chain_start,
            chain_end,
        );
        let st = st_custom!(
            ctx,
            ReplayContents() = (
                st_elem,
                Statement::None,
                Statement::None,
                Statement::None,
                Statement::None
            )
        )
        .unwrap();
        record(stats, "ReplayContents");
        return (st, c, l, n);
    }

    // K>=2 step: peel off head, dispatch on its type, recurse on tail.
    // For Insert and Mutate the Replay<X> body is inlined into the
    // ReplayContentsStep<X> predicate, with `new`/`new_live` (Insert)
    // or `old`/`new` (Mutate) packed into a small private dict so the
    // wildcard count stays at the pod2 limit. Delete keeps its
    // ReplayDelete wrapping (already at the 5-sub-stmt limit), and
    // Action is opaque to this dispatch.
    let (first, rest) = events.split_first().unwrap();
    let (st_step, tag, c2, l2, n2) = match first {
        ChainEvent::Insert {
            new,
            chain_after,
            tx_stmt,
            guard_evidence,
            ..
        } => {
            let evidence = guard_evidence
                .clone()
                .expect("missing guard evidence for insert");
            let mut nl = live.clone();
            nl.insert(&Value::from(new.clone())).unwrap();
            let (st_rest, c2, l2, n2) = build_replay_contents(
                ctx,
                stats,
                rest,
                *chain_after,
                &nl,
                nullifiers,
                chain_start,
                chain_end,
            );
            let st = build_replay_step_insert(
                ctx,
                stats,
                new,
                live,
                &nl,
                nullifiers,
                chain_start,
                chain_end,
                tx_stmt.clone(),
                evidence,
                st_rest,
            );
            (st, EventTag::Insert, c2, l2, n2)
        }
        ChainEvent::Mutate {
            new,
            old,
            chain_after,
            tx_stmt,
            guard_evidence,
            ..
        } => {
            let evidence = guard_evidence
                .clone()
                .expect("missing guard evidence for mutate");
            let mut lm = live.clone();
            lm.delete(&Value::from(old.commitment())).unwrap();
            let mut nl = lm.clone();
            nl.insert(&Value::from(new.clone())).unwrap();
            let nul = object_nullifier_from_key_hash(object_key_hash(old).unwrap());
            let mut nn = nullifiers.clone();
            nn.insert(&Value::from(nul)).unwrap();
            let (st_rest, c2, l2, n2) = build_replay_contents(
                ctx,
                stats,
                rest,
                *chain_after,
                &nl,
                &nn,
                chain_start,
                chain_end,
            );
            let st = build_replay_step_mutate(
                ctx,
                stats,
                new,
                old,
                live,
                &lm,
                &nl,
                nullifiers,
                &nn,
                chain_start,
                chain_end,
                tx_stmt.clone(),
                evidence,
                st_rest,
            );
            (st, EventTag::Mutate, c2, l2, n2)
        }
        ChainEvent::Delete {
            old,
            chain_after,
            tx_stmt,
            guard_evidence,
            ..
        } => {
            let evidence = guard_evidence
                .clone()
                .expect("missing guard evidence for delete");
            let (st_head, l, n) = build_replay_delete(
                ctx,
                stats,
                old,
                live,
                nullifiers,
                chain_start,
                chain_end,
                tx_stmt.clone(),
                evidence,
            );
            let (st_rest, c2, l2, n2) = build_replay_contents(
                ctx,
                stats,
                rest,
                *chain_after,
                &l,
                &n,
                chain_start,
                chain_end,
            );
            let st = ctx
                .apply_custom_pred_simple(false, "ReplayContentsStepDelete", vec![st_head, st_rest])
                .unwrap();
            record(stats, "ReplayContentsStepDelete");
            (st, EventTag::Delete, c2, l2, n2)
        }
        ChainEvent::Action {
            chain_after,
            contents,
            ..
        } => {
            let (st_head, l, n) = build_replay_action(
                ctx,
                stats,
                contents,
                chain,
                live,
                nullifiers,
                chain_start,
                chain_end,
                *chain_after,
            );
            let (st_rest, c2, l2, n2) = build_replay_contents(
                ctx,
                stats,
                rest,
                *chain_after,
                &l,
                &n,
                chain_start,
                chain_end,
            );
            let st = ctx
                .apply_custom_pred_simple(false, "ReplayContentsStepAction", vec![st_head, st_rest])
                .unwrap();
            record(stats, "ReplayContentsStepAction");
            (st, EventTag::Action, c2, l2, n2)
        }
    };

    let st = match tag {
        EventTag::Insert => st_custom!(
            ctx,
            ReplayContents() = (
                Statement::None,
                st_step,
                Statement::None,
                Statement::None,
                Statement::None
            )
        ),
        EventTag::Mutate => st_custom!(
            ctx,
            ReplayContents() = (
                Statement::None,
                Statement::None,
                st_step,
                Statement::None,
                Statement::None
            )
        ),
        EventTag::Delete => st_custom!(
            ctx,
            ReplayContents() = (
                Statement::None,
                Statement::None,
                Statement::None,
                st_step,
                Statement::None
            )
        ),
        EventTag::Action => st_custom!(
            ctx,
            ReplayContents() = (
                Statement::None,
                Statement::None,
                Statement::None,
                Statement::None,
                st_step
            )
        ),
    }
    .unwrap();
    record(stats, "ReplayContents");
    (st, c2, l2, n2)
}

/// Tag for the four event variants, used to pick the right
/// `ReplayContentsStep<X>` or `ReplayElement` slot.
#[derive(Clone, Copy, Debug)]
enum EventTag {
    Insert,
    Mutate,
    Delete,
    Action,
}

/// Build the inner `Replay<X>` statement for one event, returning the
/// statement plus a tag identifying which event variant produced it.
/// Shared between `build_replay_element` (which wraps the result in
/// `ReplayElement`) and the K>=2 step branch of `build_replay_contents`
/// (which wraps in `ReplayContentsStep<X>`).
#[allow(clippy::too_many_arguments)]
fn build_replay_event(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    event: &ChainEvent,
    chain: Hash,
    live: &Set,
    nullifiers: &Set,
    chain_start: Hash,
    chain_end: Hash,
) -> (Statement, EventTag, Hash, Set, Set) {
    match event {
        ChainEvent::Insert {
            new,
            chain_after,
            tx_stmt,
            guard_evidence,
            ..
        } => {
            let evidence = guard_evidence
                .clone()
                .expect("missing guard evidence for insert");
            let (st, new_live) = build_replay_insert(
                ctx,
                stats,
                new,
                live,
                nullifiers,
                chain_start,
                chain_end,
                tx_stmt.clone(),
                evidence,
            );
            (
                st,
                EventTag::Insert,
                *chain_after,
                new_live,
                nullifiers.clone(),
            )
        }
        ChainEvent::Mutate {
            new,
            old,
            chain_after,
            tx_stmt,
            guard_evidence,
            ..
        } => {
            let evidence = guard_evidence
                .clone()
                .expect("missing guard evidence for mutate");
            let (st, new_live, new_null) = build_replay_mutate(
                ctx,
                stats,
                new,
                old,
                live,
                nullifiers,
                chain_start,
                chain_end,
                tx_stmt.clone(),
                evidence,
            );
            (st, EventTag::Mutate, *chain_after, new_live, new_null)
        }
        ChainEvent::Delete {
            old,
            chain_after,
            tx_stmt,
            guard_evidence,
            ..
        } => {
            let evidence = guard_evidence
                .clone()
                .expect("missing guard evidence for delete");
            let (st, new_live, new_null) = build_replay_delete(
                ctx,
                stats,
                old,
                live,
                nullifiers,
                chain_start,
                chain_end,
                tx_stmt.clone(),
                evidence,
            );
            (st, EventTag::Delete, *chain_after, new_live, new_null)
        }
        ChainEvent::Action {
            chain_after,
            contents,
            ..
        } => {
            let (st, new_live, new_null) = build_replay_action(
                ctx,
                stats,
                contents,
                chain,
                live,
                nullifiers,
                chain_start,
                chain_end,
                *chain_after,
            );
            (st, EventTag::Action, *chain_after, new_live, new_null)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_replay_element(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    event: &ChainEvent,
    chain: Hash,
    live: &Set,
    nullifiers: &Set,
    chain_start: Hash,
    chain_end: Hash,
) -> (Statement, Hash, Set, Set) {
    let (st_inner, tag, c, l, n) = build_replay_event(
        ctx,
        stats,
        event,
        chain,
        live,
        nullifiers,
        chain_start,
        chain_end,
    );
    let st = match tag {
        EventTag::Insert => st_custom!(
            ctx,
            ReplayElement() = (st_inner, Statement::None, Statement::None, Statement::None)
        ),
        EventTag::Mutate => st_custom!(
            ctx,
            ReplayElement() = (Statement::None, st_inner, Statement::None, Statement::None)
        ),
        EventTag::Delete => st_custom!(
            ctx,
            ReplayElement() = (Statement::None, Statement::None, st_inner, Statement::None)
        ),
        EventTag::Action => st_custom!(
            ctx,
            ReplayElement() = (Statement::None, Statement::None, Statement::None, st_inner)
        ),
    }
    .unwrap();
    record(stats, "ReplayElement");
    (st, c, l, n)
}

#[allow(clippy::too_many_arguments)]
fn build_replay_insert(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    new: &Dictionary,
    live: &Set,
    nullifiers: &Set,
    chain_start: Hash,
    chain_end: Hash,
    tx_stmt: Statement,
    guard_evidence: Statement,
) -> (Statement, Set) {
    let btx = build_tx(live, nullifiers, chain_start, chain_end);
    let mut nl = live.clone();
    nl.insert(&Value::from(new.clone())).unwrap();
    let atx = tx_with(&btx, "live", Value::from(nl.clone()));

    let op_si = ctx
        .builder
        .priv_op(op!(SetInsert(nl, (&btx, "live"), new)))
        .unwrap();
    let op_du = ctx
        .builder
        .priv_op(op!(DictUpdate(atx, btx, "live", nl)))
        .unwrap();
    let rebound_evidence = ctx
        .builder
        .priv_op(Operation::replace_value_with_entry(
            vec![None, Some((&btx, "chain_start")), Some((&btx, "chain_end"))],
            guard_evidence,
        ))
        .unwrap();
    let st = ctx
        .apply_custom_pred_simple(
            false,
            "ReplayInsert",
            vec![tx_stmt, op_si, op_du, rebound_evidence],
        )
        .unwrap();
    record(stats, "ReplayInsert");
    (st, nl)
}

/// Build a `ReplayContentsStepInsert` statement: the inlined body of
/// `ReplayInsert` plus the recursive `ReplayContents` tail. The `new`
/// object and the resulting `new_live` set are packed into a tiny
/// `ins` dict so they share a single wildcard slot (keeps the
/// predicate at 8 wildcards). Body sub-statements anchor to
/// `ins.new` / `ins.new_live` instead of using bare wildcards.
/// `nl` is `live + {new}`, supplied by the caller (which already
/// needs it for the recursive tail).
#[allow(clippy::too_many_arguments)]
fn build_replay_step_insert(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    new: &Dictionary,
    live: &Set,
    nl: &Set,
    nullifiers: &Set,
    chain_start: Hash,
    chain_end: Hash,
    tx_stmt: Statement,
    guard_evidence: Statement,
    st_rest: Statement,
) -> Statement {
    let btx = build_tx(live, nullifiers, chain_start, chain_end);
    let atx = tx_with(&btx, "live", Value::from(nl.clone()));
    let ins = dict!({
        "new" => new.clone(),
        "new_live" => nl.clone()
    });

    // Re-anchor TxInsert's `new` slot (slot 2) from literal to ins.new.
    let tx_stmt_wrapped = ctx
        .builder
        .priv_op(Operation::replace_value_with_entry(
            vec![None, None, Some((&ins, "new")), None],
            tx_stmt,
        ))
        .unwrap();
    let op_si = ctx
        .builder
        .priv_op(op!(SetInsert(
            (&ins, "new_live"),
            (&btx, "live"),
            (&ins, "new")
        )))
        .unwrap();
    let op_du = ctx
        .builder
        .priv_op(op!(DictUpdate(atx, btx, "live", (&ins, "new_live"))))
        .unwrap();
    // Re-anchor guard call's slot 0 (new) to ins.new, plus the existing
    // chain_start/chain_end anchors to btx.
    let rebound_evidence = ctx
        .builder
        .priv_op(Operation::replace_value_with_entry(
            vec![
                Some((&ins, "new")),
                Some((&btx, "chain_start")),
                Some((&btx, "chain_end")),
            ],
            guard_evidence,
        ))
        .unwrap();
    let st = ctx
        .apply_custom_pred_simple(
            false,
            "ReplayContentsStepInsert",
            vec![tx_stmt_wrapped, op_si, op_du, rebound_evidence, st_rest],
        )
        .unwrap();
    record(stats, "ReplayContentsStepInsert");
    st
}

#[allow(clippy::too_many_arguments)]
fn build_replay_mutate(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    new: &Dictionary,
    old: &Dictionary,
    live: &Set,
    nullifiers: &Set,
    chain_start: Hash,
    chain_end: Hash,
    tx_stmt: Statement,
    guard_evidence: Statement,
) -> (Statement, Set, Set) {
    let btx = build_tx(live, nullifiers, chain_start, chain_end);

    let mut lm = live.clone();
    lm.delete(&Value::from(old.commitment())).unwrap();
    let mut nl = lm.clone();
    nl.insert(&Value::from(new.clone())).unwrap();
    let nul = object_nullifier_from_key_hash(object_key_hash(old).unwrap());
    let mut nn = nullifiers.clone();
    nn.insert(&Value::from(nul)).unwrap();

    let st_event = build_replay_mutate_event(ctx, stats, new, old, &btx, &lm, &nl, &nn);

    let rebound_evidence = ctx
        .builder
        .priv_op(Operation::replace_value_with_entry(
            vec![None, Some((&btx, "chain_start")), Some((&btx, "chain_end"))],
            guard_evidence,
        ))
        .unwrap();
    let st = ctx
        .apply_custom_pred_simple(
            false,
            "ReplayMutate",
            vec![tx_stmt, st_event, rebound_evidence],
        )
        .unwrap();
    record(stats, "ReplayMutate");
    (st, nl, nn)
}

/// Build a `ReplayNullify` statement: derives the object key hash and
/// nullifier from `old`, then accumulates the nullifier into the tx's
/// nullifiers set. `mid_tx` is the tx state with the new live set
/// already in place; `after_tx` is `mid_tx` with `nullifiers` updated
/// to `nn`. Used by both mutate and delete replay.
fn build_replay_nullify(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    old: &Dictionary,
    mid_tx: &Dictionary,
    after_tx: &Dictionary,
    nn: &Set,
) -> Statement {
    let okh = object_key_hash(old).unwrap();
    let nul = object_nullifier_from_key_hash(okh);
    let op_h1 = ctx
        .builder
        .priv_op(op!(HashOf(okh, old, (old, "key"))))
        .unwrap();
    let op_h2 = ctx
        .builder
        .priv_op(op!(HashOf(nul, okh, OBJECT_NULLIFIER_VERSION)))
        .unwrap();
    let op_si = ctx
        .builder
        .priv_op(op!(SetInsert(nn, (mid_tx, "nullifiers"), nul)))
        .unwrap();
    let op_du_null = ctx
        .builder
        .priv_op(op!(DictUpdate(after_tx, mid_tx, "nullifiers", nn)))
        .unwrap();
    let st = ctx
        .apply_custom_pred_simple(
            false,
            "ReplayNullify",
            vec![op_h1, op_h2, op_si, op_du_null],
        )
        .unwrap();
    record(stats, "ReplayNullify");
    st
}

/// Build `ReplayMutateEvent` (and its inner `ReplayNullify`). Shared
/// between `build_replay_mutate` and `build_replay_step_mutate` (these
/// inner predicates don't reference `new`/`old` via anchored keys --
/// they take the dicts directly as wildcards).
#[allow(clippy::too_many_arguments)]
fn build_replay_mutate_event(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    new: &Dictionary,
    old: &Dictionary,
    btx: &Dictionary,
    lm: &Set,
    nl: &Set,
    nn: &Set,
) -> Statement {
    let m1 = tx_with(btx, "live", Value::from(nl.clone()));
    let atx = tx_with(&m1, "nullifiers", Value::from(nn.clone()));
    let st_nullify = build_replay_nullify(ctx, stats, old, &m1, &atx, nn);

    // Live swap + nullify; chain/event-hash work is delegated to the
    // parent's TxMutate statement.
    let op_sd = ctx
        .builder
        .priv_op(op!(SetDelete(lm, (btx, "live"), old)))
        .unwrap();
    let op_si = ctx.builder.priv_op(op!(SetInsert(nl, lm, new))).unwrap();
    let op_du_live = ctx
        .builder
        .priv_op(op!(DictUpdate(m1, btx, "live", nl)))
        .unwrap();
    let st_event = ctx
        .apply_custom_pred_simple(
            false,
            "ReplayMutateEvent",
            vec![op_sd, op_si, op_du_live, st_nullify],
        )
        .unwrap();
    record(stats, "ReplayMutateEvent");
    st_event
}

/// Build a `ReplayContentsStepMutate` statement: the inlined body of
/// `ReplayMutate` plus the recursive `ReplayContents` tail. `old` and
/// `new` are packed into a `pair` dict so they share a single
/// wildcard slot (keeps the predicate at 8 wildcards). The TxMutate
/// and guard sub-statements are re-anchored to `pair.old` / `pair.new`.
/// `lm`/`nl`/`nn` are supplied by the caller (which already needs them
/// for the recursive tail).
#[allow(clippy::too_many_arguments)]
fn build_replay_step_mutate(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    new: &Dictionary,
    old: &Dictionary,
    live: &Set,
    lm: &Set,
    nl: &Set,
    nullifiers: &Set,
    nn: &Set,
    chain_start: Hash,
    chain_end: Hash,
    tx_stmt: Statement,
    guard_evidence: Statement,
    st_rest: Statement,
) -> Statement {
    let btx = build_tx(live, nullifiers, chain_start, chain_end);
    let pair = dict!({
        "old" => old.clone(),
        "new" => new.clone()
    });

    let st_event = build_replay_mutate_event(ctx, stats, new, old, &btx, lm, nl, nn);

    // Re-anchor TxMutate's `new` (slot 2) and `old` (slot 3) to pair.
    let tx_stmt_wrapped = ctx
        .builder
        .priv_op(Operation::replace_value_with_entry(
            vec![None, None, Some((&pair, "new")), Some((&pair, "old")), None],
            tx_stmt,
        ))
        .unwrap();
    // Re-anchor guard call's slot 0 (new) to pair.new.
    let rebound_evidence = ctx
        .builder
        .priv_op(Operation::replace_value_with_entry(
            vec![
                Some((&pair, "new")),
                Some((&btx, "chain_start")),
                Some((&btx, "chain_end")),
            ],
            guard_evidence,
        ))
        .unwrap();
    let st = ctx
        .apply_custom_pred_simple(
            false,
            "ReplayContentsStepMutate",
            vec![tx_stmt_wrapped, st_event, rebound_evidence, st_rest],
        )
        .unwrap();
    record(stats, "ReplayContentsStepMutate");
    st
}

#[allow(clippy::too_many_arguments)]
fn build_replay_delete(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    old: &Dictionary,
    live: &Set,
    nullifiers: &Set,
    chain_start: Hash,
    chain_end: Hash,
    tx_stmt: Statement,
    guard_evidence: Statement,
) -> (Statement, Set, Set) {
    let btx = build_tx(live, nullifiers, chain_start, chain_end);

    let mut nl = live.clone();
    nl.delete(&Value::from(old.commitment())).unwrap();
    let nul = object_nullifier_from_key_hash(object_key_hash(old).unwrap());
    let mut nn = nullifiers.clone();
    nn.insert(&Value::from(nul)).unwrap();
    let m1 = tx_with(&btx, "live", Value::from(nl.clone()));
    let atx = tx_with(&m1, "nullifiers", Value::from(nn.clone()));

    let st_nullify = build_replay_nullify(ctx, stats, old, &m1, &atx, &nn);

    let op_sd = ctx
        .builder
        .priv_op(op!(SetDelete(nl, (&btx, "live"), old)))
        .unwrap();
    let op_du_live = ctx
        .builder
        .priv_op(op!(DictUpdate(m1, btx, "live", nl)))
        .unwrap();
    let rebound_evidence = ctx
        .builder
        .priv_op(Operation::replace_value_with_entry(
            vec![None, Some((&btx, "chain_start")), Some((&btx, "chain_end"))],
            guard_evidence,
        ))
        .unwrap();
    let st = ctx
        .apply_custom_pred_simple(
            false,
            "ReplayDelete",
            vec![tx_stmt, op_sd, op_du_live, st_nullify, rebound_evidence],
        )
        .unwrap();
    record(stats, "ReplayDelete");
    (st, nl, nn)
}

#[allow(clippy::too_many_arguments)]
fn build_replay_action(
    ctx: &mut BuildContext,
    stats: &mut TxStats,
    contents: &[ChainEvent],
    chain: Hash,
    live: &Set,
    nullifiers: &Set,
    parent_chain_start: Hash,
    parent_chain_end: Hash,
    chain_after: Hash,
) -> (Statement, Set, Set) {
    let btx = build_tx(live, nullifiers, parent_chain_start, parent_chain_end);

    let ms = tx_with(&btx, "chain_start", Value::from(chain));
    let itx = tx_with(&ms, "chain_end", Value::from(chain_after));

    let (st_contents, _ce, le, ne) = build_replay_contents(
        ctx,
        stats,
        contents,
        chain,
        live,
        nullifiers,
        chain,
        chain_after,
    );

    let etx = build_tx(&le, &ne, chain, chain_after);

    let fm1 = tx_with(&btx, "live", Value::from(le.clone()));
    let atx = tx_with(&fm1, "nullifiers", Value::from(ne.clone()));

    // ReplayAction (scope setup + contents + live/nullifier copy-back)
    let op_scope1 = ctx
        .builder
        .priv_op(op!(DictUpdate(ms, btx, "chain_start", chain)))
        .unwrap();
    let op_scope2 = ctx
        .builder
        .priv_op(op!(DictUpdate(itx, ms, "chain_end", chain_after)))
        .unwrap();
    let op_du1 = ctx
        .builder
        .priv_op(op!(DictUpdate(fm1, btx, "live", (&etx, "live"))))
        .unwrap();
    let op_du2 = ctx
        .builder
        .priv_op(op!(DictUpdate(
            atx,
            fm1,
            "nullifiers",
            (&etx, "nullifiers")
        )))
        .unwrap();
    let st = ctx
        .apply_custom_pred(
            false,
            "ReplayAction",
            map!({"before_tx" => btx.clone(), "after_tx" => atx.clone(), "before_chain" => chain, "after_chain" => chain_after}),
            vec![op_scope1, op_scope2, st_contents, op_du1, op_du2],
        )
        .unwrap();
    record(stats, "ReplayAction");
    (st, le, ne)
}
