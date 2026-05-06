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
        Hash, Key, Statement, Value,
        containers::{Dictionary, Set},
        hash_values,
    },
};
use pod2utils::{macros::BuildContext, map, op, st_custom};

use crate::{ChainEvent, OBJECT_NULLIFIER_VERSION, TxStats, build_tx, record, tx_with};

/// Walk the top-level event list and build a `ReplayActions` statement.
/// Every top-level event must be `ChainEvent::Action` -- the prover
/// API enforces this by construction, and we panic here if not.
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
    if events.is_empty() {
        // Done: reuse ReplayContentsDone.
        let d = build_tx(live, nullifiers, chain_start, chain_end);
        let st_done = st_custom!(
            ctx,
            ReplayContentsDone() = (Equal(d, d), Equal(chain, chain))
        )
        .unwrap();
        record(stats, "ReplayContentsDone");
        let st = st_custom!(
            ctx,
            ReplayActions() = (st_done, Statement::None, Statement::None)
        )
        .unwrap();
        record(stats, "ReplayActions");
        return (st, chain, live.clone(), nullifiers.clone());
    }

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
        let st = st_custom!(
            ctx,
            ReplayActions() = (Statement::None, st_action, Statement::None)
        )
        .unwrap();
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
    let st = st_custom!(
        ctx,
        ReplayActions() = (Statement::None, Statement::None, st_step)
    )
    .unwrap();
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

/// Recursively build `ReplayContents` for a list of events.
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
    if events.is_empty() {
        // Done: before_tx = after_tx AND before_chain = after_chain
        let d = build_tx(live, nullifiers, chain_start, chain_end);
        let st_done = st_custom!(
            ctx,
            ReplayContentsDone() = (Equal(d, d), Equal(chain, chain))
        )
        .unwrap();
        record(stats, "ReplayContentsDone");
        let st = st_custom!(
            ctx,
            ReplayContents() = (st_done, Statement::None, Statement::None, Statement::None)
        )
        .unwrap();
        record(stats, "ReplayContents");
        return (st, chain, live.clone(), nullifiers.clone());
    }

    if events.len() == 1 {
        // Single branch: exactly one element, skip Step + Done overhead
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
            ReplayContents() = (Statement::None, st_elem, Statement::None, Statement::None)
        )
        .unwrap();
        record(stats, "ReplayContents");
        return (st, c, l, n);
    }

    if events.len() == 2 {
        // Pair branch: exactly two elements, skip Step + recursive Contents
        let (st_elem1, c1, l1, n1) = build_replay_element(
            ctx,
            stats,
            &events[0],
            chain,
            live,
            nullifiers,
            chain_start,
            chain_end,
        );
        let (st_elem2, c2, l2, n2) =
            build_replay_element(ctx, stats, &events[1], c1, &l1, &n1, chain_start, chain_end);
        let st_pair = st_custom!(ctx, ReplayContentsPair() = (st_elem1, st_elem2)).unwrap();
        record(stats, "ReplayContentsPair");
        let st = st_custom!(
            ctx,
            ReplayContents() = (Statement::None, Statement::None, st_pair, Statement::None)
        )
        .unwrap();
        record(stats, "ReplayContents");
        return (st, c2, l2, n2);
    }

    // Step branch: 3+ elements, peel off first and recurse
    let (first, rest) = events.split_first().unwrap();
    let (st_elem, c, l, n) = build_replay_element(
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
        build_replay_contents(ctx, stats, rest, c, &l, &n, chain_start, chain_end);

    let st_step = st_custom!(ctx, ReplayContentsStep() = (st_elem, st_rest)).unwrap();
    record(stats, "ReplayContentsStep");
    let st = st_custom!(
        ctx,
        ReplayContents() = (Statement::None, Statement::None, Statement::None, st_step)
    )
    .unwrap();
    record(stats, "ReplayContents");
    (st, c2, l2, n2)
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
            let st = st_custom!(
                ctx,
                ReplayElement() = (st, Statement::None, Statement::None, Statement::None)
            )
            .unwrap();
            record(stats, "ReplayElement");
            (st, *chain_after, new_live, nullifiers.clone())
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
            let st = st_custom!(
                ctx,
                ReplayElement() = (Statement::None, st, Statement::None, Statement::None)
            )
            .unwrap();
            record(stats, "ReplayElement");
            (st, *chain_after, new_live, new_null)
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
            let st = st_custom!(
                ctx,
                ReplayElement() = (Statement::None, Statement::None, st, Statement::None)
            )
            .unwrap();
            record(stats, "ReplayElement");
            (st, *chain_after, new_live, new_null)
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
            let st = st_custom!(
                ctx,
                ReplayElement() = (Statement::None, Statement::None, Statement::None, st)
            )
            .unwrap();
            record(stats, "ReplayElement");
            (st, *chain_after, new_live, new_null)
        }
    }
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

    // ReplayInsert: TxInsert (from record time) + state update + guard.
    let op_si = ctx
        .builder
        .priv_op(op!(SetInsert(nl, (&btx, "live"), new)))
        .unwrap();
    let op_du = ctx
        .builder
        .priv_op(op!(DictUpdate(atx, btx, "live", nl)))
        .unwrap();
    let guard = new
        .get(&Key::from("type"))
        .unwrap()
        .expect("object missing 'type' field");
    let op_dc = ctx
        .builder
        .priv_op(op!(DictContains(new, "type", guard.clone())))
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
            vec![tx_stmt, op_si, op_du, op_dc, rebound_evidence],
        )
        .unwrap();
    record(stats, "ReplayInsert");
    (st, nl)
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
    let okh = hash_values(&[
        Value::from(old.commitment()),
        old.get(&Key::from("key")).unwrap().unwrap(),
    ]);
    let nul = hash_values(&[Value::from(okh), Value::from(OBJECT_NULLIFIER_VERSION)]);
    let mut nn = nullifiers.clone();
    nn.insert(&Value::from(nul)).unwrap();
    let m1 = tx_with(&btx, "live", Value::from(nl.clone()));
    let atx = tx_with(&m1, "nullifiers", Value::from(nn.clone()));

    // ReplayNullify (called inside ReplayMutateEvent)
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
        .priv_op(op!(SetInsert(nn, (&m1, "nullifiers"), nul)))
        .unwrap();
    let op_du_null = ctx
        .builder
        .priv_op(op!(DictUpdate(atx, m1, "nullifiers", nn)))
        .unwrap();
    let st_nullify = ctx
        .apply_custom_pred_simple(
            false,
            "ReplayNullify",
            vec![op_h1, op_h2, op_si, op_du_null],
        )
        .unwrap();
    record(stats, "ReplayNullify");

    // ReplayMutateEvent (live swap + nullify; chain/event-hash work is
    // delegated to the TxMutate statement referenced by ReplayMutate).
    let op_sd = ctx
        .builder
        .priv_op(op!(SetDelete(lm, (&btx, "live"), old)))
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

    // ReplayMutate (type-preserve + TxMutate ref + event + guard).
    let op_eq = ctx
        .builder
        .priv_op(op!(Equal((old, "type"), (new, "type"))))
        .unwrap();
    let guard = new
        .get(&Key::from("type"))
        .unwrap()
        .expect("object missing 'type' field");
    let op_dc = ctx
        .builder
        .priv_op(op!(DictContains(new, "type", guard.clone())))
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
            "ReplayMutate",
            vec![op_eq, tx_stmt, st_event, op_dc, rebound_evidence],
        )
        .unwrap();
    record(stats, "ReplayMutate");
    (st, nl, nn)
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
    let okh = hash_values(&[
        Value::from(old.commitment()),
        old.get(&Key::from("key")).unwrap().unwrap(),
    ]);
    let nul = hash_values(&[Value::from(okh), Value::from(OBJECT_NULLIFIER_VERSION)]);
    let mut nn = nullifiers.clone();
    nn.insert(&Value::from(nul)).unwrap();
    let m1 = tx_with(&btx, "live", Value::from(nl.clone()));
    let atx = tx_with(&m1, "nullifiers", Value::from(nn.clone()));

    // ReplayNullify (called inside ReplayDeleteEvent)
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
        .priv_op(op!(SetInsert(nn, (&m1, "nullifiers"), nul)))
        .unwrap();
    let op_du_null = ctx
        .builder
        .priv_op(op!(DictUpdate(atx, m1, "nullifiers", nn)))
        .unwrap();
    let st_nullify = ctx
        .apply_custom_pred_simple(
            false,
            "ReplayNullify",
            vec![op_h1, op_h2, op_si, op_du_null],
        )
        .unwrap();
    record(stats, "ReplayNullify");

    // ReplayDeleteEvent (live removal + nullify; chain/event-hash work is
    // delegated to the TxDelete statement referenced by ReplayDelete).
    let op_sd = ctx
        .builder
        .priv_op(op!(SetDelete(nl, (&btx, "live"), old)))
        .unwrap();
    let op_du_live = ctx
        .builder
        .priv_op(op!(DictUpdate(m1, btx, "live", nl)))
        .unwrap();
    let st_event = ctx
        .apply_custom_pred_simple(
            false,
            "ReplayDeleteEvent",
            vec![op_sd, op_du_live, st_nullify],
        )
        .unwrap();
    record(stats, "ReplayDeleteEvent");

    // ReplayDelete (TxDelete ref + event + guard inline).
    let guard = old
        .get(&Key::from("type"))
        .unwrap()
        .expect("object missing 'type' field");
    let op_dc = ctx
        .builder
        .priv_op(op!(DictContains(old, "type", guard.clone())))
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
            vec![tx_stmt, st_event, op_dc, rebound_evidence],
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
