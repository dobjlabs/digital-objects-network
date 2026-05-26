//! Replay circuit construction for `TxBuilder::finalize`.
//!
//! At finalize time the recorded events form a tree (actions containing
//! events and sub-actions). This module walks that tree and builds the
//! POD2 predicate statements that prove each event's hash step, update
//! the live/nullifier sets, and dispatch each event to its object-type
//! guard.
//!
//! The walker is a `Replayer` that owns the long-lived mutable builder
//! state (`BuildContext` + `TxStats`). A `ReplayFrame` carries the
//! per-step immutable world view -- the current live/nullifier sets
//! plus the chain-scope bounds -- and threads through the recursion.
//! Only `Replayer::build_replay_actions` is `pub(crate)`; every other
//! method here is a private helper it delegates to.

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

/// The replay walker. Owns the long-lived mutable builder state
/// (`BuildContext` + `TxStats`) that threads through every event.
pub(crate) struct Replayer<'a> {
    ctx: &'a mut BuildContext,
    stats: &'a mut TxStats,
}

/// Per-step immutable world view: the live/nullifier sets and the
/// chain-scope bounds at the current point in the replay walk.
#[derive(Clone, Copy)]
pub(crate) struct ReplayFrame<'a> {
    pub(crate) live: &'a Set,
    pub(crate) nullifiers: &'a Set,
    pub(crate) chain_start: Hash,
    pub(crate) chain_end: Hash,
}

impl<'a> ReplayFrame<'a> {
    /// Derive the next frame after a non-mutating step that only changes the
    /// live set (Insert).
    pub(crate) fn with_live<'b>(self, live: &'b Set) -> ReplayFrame<'b>
    where
        'a: 'b,
    {
        ReplayFrame {
            live,
            nullifiers: self.nullifiers,
            chain_start: self.chain_start,
            chain_end: self.chain_end,
        }
    }

    /// Derive the next frame after a step that updates both sets (Mutate,
    /// Delete, Action, or the top-level `ReplayActions` step).
    pub(crate) fn advance<'b>(self, live: &'b Set, nullifiers: &'b Set) -> ReplayFrame<'b>
    where
        'a: 'b,
    {
        ReplayFrame {
            live,
            nullifiers,
            chain_start: self.chain_start,
            chain_end: self.chain_end,
        }
    }

    /// Open a new action scope: same sets, fresh chain bounds.
    pub(crate) fn rescope(self, chain_start: Hash, chain_end: Hash) -> Self {
        ReplayFrame {
            chain_start,
            chain_end,
            ..self
        }
    }

    /// Build the tx-context dictionary that this frame represents.
    pub(crate) fn to_tx_dict(self) -> Dictionary {
        build_tx(self.live, self.nullifiers, self.chain_start, self.chain_end)
    }
}

/// Derived state needed to build a Mutate event's replay clauses.
/// `btx` is the pre-mutate tx-context dict; `live_minus_old` is the
/// live set with `old` removed; `new_live` is `live_minus_old` with
/// `new` inserted; `new_nullifiers` is the nullifiers set with
/// `nullifier(old)` accumulated. Owned because the caller also needs
/// `new_live` / `new_nullifiers` to thread into the recursive tail
/// frame.
pub(crate) struct MutateScratch {
    pub(crate) btx: Dictionary,
    pub(crate) live_minus_old: Set,
    pub(crate) new_live: Set,
    pub(crate) new_nullifiers: Set,
}

impl<'a> ReplayFrame<'a> {
    /// Compute the pre-mutate tx context + post-mutate set snapshots
    /// for a `(old -> new)` mutate.
    pub(crate) fn mutate_scratch(self, old: &Dictionary, new: &Dictionary) -> MutateScratch {
        let btx = self.to_tx_dict();
        let mut live_minus_old = self.live.clone();
        live_minus_old
            .delete(&Value::from(old.commitment()))
            .unwrap();
        let mut new_live = live_minus_old.clone();
        new_live.insert(&Value::from(new.clone())).unwrap();
        let nul = object_nullifier_from_key_hash(object_key_hash(old).unwrap());
        let mut new_nullifiers = self.nullifiers.clone();
        new_nullifiers.insert(&Value::from(nul)).unwrap();
        MutateScratch {
            btx,
            live_minus_old,
            new_live,
            new_nullifiers,
        }
    }
}

impl<'a> Replayer<'a> {
    pub(crate) fn new(ctx: &'a mut BuildContext, stats: &'a mut TxStats) -> Self {
        Self { ctx, stats }
    }

    fn record(&mut self, name: &str) {
        record(self.stats, name);
    }

    /// Walk the top-level event list and build a `ReplayActions`
    /// statement. Every top-level event must be `ChainEvent::Action` --
    /// the prover API enforces this by construction, and we panic here
    /// if not. `events` is guaranteed non-empty (TxBuilder::finalize
    /// asserts). Callers: `TxBuilder::finalize`.
    ///
    /// For a single top-level action whose body is a lone Insert event,
    /// dispatches into the `ReplayActionInsert` K=1 fast path (slot 3
    /// of the `ReplayActions` OR), which proves the whole transaction
    /// in 2 statements instead of going through ReplayAction/ReplayContents/
    /// ReplayElement/ReplayInsert. Multi-action transactions always use
    /// the slow ReplayAction for each action, because `ReplayActionsStep`
    /// expects a `ReplayAction` statement in its first slot.
    pub(crate) fn build_replay_actions(
        &mut self,
        events: &[ChainEvent],
        chain: Hash,
        frame: ReplayFrame<'_>,
    ) -> (Statement, Hash, Set, Set) {
        assert!(
            !events.is_empty(),
            "build_replay_actions: empty event list (empty Tx is forbidden)"
        );

        if events.len() == 1 {
            let event = &events[0];
            let ChainEvent::Action {
                chain_after,
                contents,
            } = event
            else {
                panic!(
                    "top-level event must be a ChainEvent::Action (bare events are only allowed inside an action scope)"
                );
            };

            if let [ChainEvent::Insert { .. }] = contents.as_slice() {
                let (st_inner, new_live, new_nulls) =
                    self.build_replay_action_insert(contents, frame);
                let st = st_custom!(
                    self.ctx,
                    ReplayActions() = (Statement::None, Statement::None, st_inner)
                )
                .unwrap();
                self.record("ReplayActions");
                return (st, *chain_after, new_live, new_nulls);
            }

            let (st_action, new_live, new_nulls) =
                self.build_replay_action(contents, chain, frame, *chain_after);
            let st = st_custom!(
                self.ctx,
                ReplayActions() = (st_action, Statement::None, Statement::None)
            )
            .unwrap();
            self.record("ReplayActions");
            return (st, *chain_after, new_live, new_nulls);
        }

        // Step: first action + recursive tail.
        let (first, rest) = events.split_first().unwrap();
        let (st_action, next_chain, next_live, next_nulls) =
            self.build_top_level_action(first, chain, frame);
        let (st_rest, final_chain, final_live, final_nulls) =
            self.build_replay_actions(rest, next_chain, frame.advance(&next_live, &next_nulls));
        let st_step = st_custom!(self.ctx, ReplayActionsStep() = (st_action, st_rest)).unwrap();
        self.record("ReplayActionsStep");
        let st = st_custom!(
            self.ctx,
            ReplayActions() = (Statement::None, st_step, Statement::None)
        )
        .unwrap();
        self.record("ReplayActions");
        (st, final_chain, final_live, final_nulls)
    }

    /// Extract an action from a top-level `ChainEvent` and delegate to
    /// `build_replay_action`. Panics on non-action variants.
    fn build_top_level_action(
        &mut self,
        event: &ChainEvent,
        chain: Hash,
        frame: ReplayFrame<'_>,
    ) -> (Statement, Hash, Set, Set) {
        match event {
            ChainEvent::Action {
                chain_after,
                contents,
            } => {
                let (st, new_live, new_null) =
                    self.build_replay_action(contents, chain, frame, *chain_after);
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
    fn build_replay_contents(
        &mut self,
        events: &[ChainEvent],
        chain: Hash,
        frame: ReplayFrame<'_>,
    ) -> (Statement, Hash, Set, Set) {
        assert!(
            !events.is_empty(),
            "build_replay_contents: empty event list (empty action scope is forbidden)"
        );

        if events.len() == 1 {
            let (st_elem, next_chain, next_live, next_nulls) =
                self.build_replay_element(&events[0], chain, frame);
            let st = st_custom!(
                self.ctx,
                ReplayContents() = (
                    st_elem,
                    Statement::None,
                    Statement::None,
                    Statement::None,
                    Statement::None
                )
            )
            .unwrap();
            self.record("ReplayContents");
            return (st, next_chain, next_live, next_nulls);
        }

        // K>=2 step: peel off head, dispatch on its type, recurse on tail.
        // For Insert and Mutate the Replay<X> body is inlined into the
        // ReplayContentsStep<X> predicate, with `new`/`new_live` (Insert)
        // or `old`/`new` (Mutate) packed into a small private dict so the
        // wildcard count stays at the pod2 limit. Delete keeps its
        // ReplayDelete wrapping (already at the 5-sub-stmt limit), and
        // Action is opaque to this dispatch.
        let (first, rest) = events.split_first().unwrap();
        let (st_step, tag, final_chain, final_live, final_nulls) = match first {
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
                let mut new_live = frame.live.clone();
                new_live.insert(&Value::from(new.clone())).unwrap();
                let (st_rest, final_chain, final_live, final_nulls) =
                    self.build_replay_contents(rest, *chain_after, frame.with_live(&new_live));
                let st = self.build_replay_step_insert(
                    new,
                    frame,
                    &new_live,
                    tx_stmt.clone(),
                    evidence,
                    st_rest,
                );
                (st, EventTag::Insert, final_chain, final_live, final_nulls)
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
                let scratch = frame.mutate_scratch(old, new);
                let (st_rest, final_chain, final_live, final_nulls) = self.build_replay_contents(
                    rest,
                    *chain_after,
                    frame.advance(&scratch.new_live, &scratch.new_nullifiers),
                );
                let st = self.build_replay_step_mutate(
                    new,
                    old,
                    &scratch,
                    tx_stmt.clone(),
                    evidence,
                    st_rest,
                );
                (st, EventTag::Mutate, final_chain, final_live, final_nulls)
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
                let (st_head, next_live, next_nulls) =
                    self.build_replay_delete(old, frame, tx_stmt.clone(), evidence);
                let (st_rest, final_chain, final_live, final_nulls) = self.build_replay_contents(
                    rest,
                    *chain_after,
                    frame.advance(&next_live, &next_nulls),
                );
                let st = self
                    .ctx
                    .apply_custom_pred_simple(
                        false,
                        "ReplayContentsStepDelete",
                        vec![st_head, st_rest],
                    )
                    .unwrap();
                self.record("ReplayContentsStepDelete");
                (st, EventTag::Delete, final_chain, final_live, final_nulls)
            }
            ChainEvent::Action {
                chain_after,
                contents,
                ..
            } => {
                let (st_head, next_live, next_nulls) =
                    self.build_replay_action(contents, chain, frame, *chain_after);
                let (st_rest, final_chain, final_live, final_nulls) = self.build_replay_contents(
                    rest,
                    *chain_after,
                    frame.advance(&next_live, &next_nulls),
                );
                let st = self
                    .ctx
                    .apply_custom_pred_simple(
                        false,
                        "ReplayContentsStepAction",
                        vec![st_head, st_rest],
                    )
                    .unwrap();
                self.record("ReplayContentsStepAction");
                (st, EventTag::Action, final_chain, final_live, final_nulls)
            }
        };

        let st = match tag {
            EventTag::Insert => st_custom!(
                self.ctx,
                ReplayContents() = (
                    Statement::None,
                    st_step,
                    Statement::None,
                    Statement::None,
                    Statement::None
                )
            ),
            EventTag::Mutate => st_custom!(
                self.ctx,
                ReplayContents() = (
                    Statement::None,
                    Statement::None,
                    st_step,
                    Statement::None,
                    Statement::None
                )
            ),
            EventTag::Delete => st_custom!(
                self.ctx,
                ReplayContents() = (
                    Statement::None,
                    Statement::None,
                    Statement::None,
                    st_step,
                    Statement::None
                )
            ),
            EventTag::Action => st_custom!(
                self.ctx,
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
        self.record("ReplayContents");
        (st, final_chain, final_live, final_nulls)
    }

    /// Build the inner `Replay<X>` statement for one event, returning
    /// the statement plus a tag identifying which event variant
    /// produced it. Shared between `build_replay_element` (which wraps
    /// the result in `ReplayElement`) and the K>=2 step branch of
    /// `build_replay_contents` (which wraps in `ReplayContentsStep<X>`).
    fn build_replay_event(
        &mut self,
        event: &ChainEvent,
        chain: Hash,
        frame: ReplayFrame<'_>,
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
                let (st, new_live) =
                    self.build_replay_insert(new, frame, tx_stmt.clone(), evidence);
                (
                    st,
                    EventTag::Insert,
                    *chain_after,
                    new_live,
                    frame.nullifiers.clone(),
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
                let (st, new_live, new_null) =
                    self.build_replay_mutate(new, old, frame, tx_stmt.clone(), evidence);
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
                let (st, new_live, new_null) =
                    self.build_replay_delete(old, frame, tx_stmt.clone(), evidence);
                (st, EventTag::Delete, *chain_after, new_live, new_null)
            }
            ChainEvent::Action {
                chain_after,
                contents,
                ..
            } => {
                let (st, new_live, new_null) =
                    self.build_replay_action(contents, chain, frame, *chain_after);
                (st, EventTag::Action, *chain_after, new_live, new_null)
            }
        }
    }

    fn build_replay_element(
        &mut self,
        event: &ChainEvent,
        chain: Hash,
        frame: ReplayFrame<'_>,
    ) -> (Statement, Hash, Set, Set) {
        let (st_inner, tag, next_chain, next_live, next_nulls) =
            self.build_replay_event(event, chain, frame);
        let st = match tag {
            EventTag::Insert => st_custom!(
                self.ctx,
                ReplayElement() = (st_inner, Statement::None, Statement::None, Statement::None)
            ),
            EventTag::Mutate => st_custom!(
                self.ctx,
                ReplayElement() = (Statement::None, st_inner, Statement::None, Statement::None)
            ),
            EventTag::Delete => st_custom!(
                self.ctx,
                ReplayElement() = (Statement::None, Statement::None, st_inner, Statement::None)
            ),
            EventTag::Action => st_custom!(
                self.ctx,
                ReplayElement() = (Statement::None, Statement::None, Statement::None, st_inner)
            ),
        }
        .unwrap();
        self.record("ReplayElement");
        (st, next_chain, next_live, next_nulls)
    }

    fn build_replay_insert(
        &mut self,
        new: &Dictionary,
        frame: ReplayFrame<'_>,
        tx_stmt: Statement,
        guard_evidence: Statement,
    ) -> (Statement, Set) {
        let btx = frame.to_tx_dict();
        let mut new_live = frame.live.clone();
        new_live.insert(&Value::from(new.clone())).unwrap();
        let atx = tx_with(&btx, "live", Value::from(new_live.clone()));

        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert(new_live, (&btx, "live"), new)))
            .unwrap();
        let op_du = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(atx, btx, "live", new_live)))
            .unwrap();
        let rebound_evidence = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, Some((&btx, "chain_start")), Some((&btx, "chain_end"))],
                guard_evidence,
            ))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayInsert",
                vec![tx_stmt, op_si, op_du, rebound_evidence],
            )
            .unwrap();
        self.record("ReplayInsert");
        (st, new_live)
    }

    /// Build a `ReplayContentsStepInsert` statement: the inlined body
    /// of `ReplayInsert` plus the recursive `ReplayContents` tail. The
    /// `new` object and the resulting `new_live` set are packed into a
    /// tiny `ins` dict so they share a single wildcard slot (keeps the
    /// predicate at 8 wildcards). Body sub-statements anchor to
    /// `ins.new` / `ins.new_live` instead of using bare wildcards.
    /// `new_live` is `live + {new}`, supplied by the caller (which already
    /// needs it for the recursive tail).
    fn build_replay_step_insert(
        &mut self,
        new: &Dictionary,
        frame: ReplayFrame<'_>,
        new_live: &Set,
        tx_stmt: Statement,
        guard_evidence: Statement,
        st_rest: Statement,
    ) -> Statement {
        let btx = frame.to_tx_dict();
        let atx = tx_with(&btx, "live", Value::from(new_live.clone()));
        let ins = dict!({
            "new" => new.clone(),
            "new_live" => new_live.clone()
        });

        // Re-anchor TxInsert's `new` slot (slot 2) from literal to ins.new.
        let tx_stmt_wrapped = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, None, Some((&ins, "new")), None],
                tx_stmt,
            ))
            .unwrap();
        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert(
                (&ins, "new_live"),
                (&btx, "live"),
                (&ins, "new")
            )))
            .unwrap();
        let op_du = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(atx, btx, "live", (&ins, "new_live"))))
            .unwrap();
        // Re-anchor guard call's slot 0 (new) to ins.new, plus the existing
        // chain_start/chain_end anchors to btx.
        let rebound_evidence = self
            .ctx
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
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayContentsStepInsert",
                vec![tx_stmt_wrapped, op_si, op_du, rebound_evidence, st_rest],
            )
            .unwrap();
        self.record("ReplayContentsStepInsert");
        st
    }

    fn build_replay_mutate(
        &mut self,
        new: &Dictionary,
        old: &Dictionary,
        frame: ReplayFrame<'_>,
        tx_stmt: Statement,
        guard_evidence: Statement,
    ) -> (Statement, Set, Set) {
        let scratch = frame.mutate_scratch(old, new);
        let st_event = self.build_replay_mutate_event(new, old, &scratch);

        let rebound_evidence = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    None,
                    Some((&scratch.btx, "chain_start")),
                    Some((&scratch.btx, "chain_end")),
                ],
                guard_evidence,
            ))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayMutate",
                vec![tx_stmt, st_event, rebound_evidence],
            )
            .unwrap();
        self.record("ReplayMutate");
        let MutateScratch {
            new_live,
            new_nullifiers,
            ..
        } = scratch;
        (st, new_live, new_nullifiers)
    }

    /// Build a `ReplayNullify` statement: derives the object key hash
    /// and nullifier from `old`, then accumulates the nullifier into
    /// the tx's nullifiers set. `mid_tx` is the tx state with the new
    /// live set already in place; `after_tx` is `mid_tx` with
    /// `nullifiers` updated to `new_nullifiers`. Used by both mutate and delete
    /// replay.
    fn build_replay_nullify(
        &mut self,
        old: &Dictionary,
        mid_tx: &Dictionary,
        after_tx: &Dictionary,
        new_nullifiers: &Set,
    ) -> Statement {
        let okh = object_key_hash(old).unwrap();
        let nul = object_nullifier_from_key_hash(okh);
        let op_h1 = self
            .ctx
            .builder
            .priv_op(op!(HashOf(okh, old, (old, "key"))))
            .unwrap();
        let op_h2 = self
            .ctx
            .builder
            .priv_op(op!(HashOf(nul, okh, OBJECT_NULLIFIER_VERSION)))
            .unwrap();
        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert(new_nullifiers, (mid_tx, "nullifiers"), nul)))
            .unwrap();
        let op_du_null = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                after_tx,
                mid_tx,
                "nullifiers",
                new_nullifiers
            )))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayNullify",
                vec![op_h1, op_h2, op_si, op_du_null],
            )
            .unwrap();
        self.record("ReplayNullify");
        st
    }

    /// Build `ReplayMutateEvent` (and its inner `ReplayNullify`).
    /// Shared between `build_replay_mutate` and
    /// `build_replay_step_mutate` (these inner predicates don't
    /// reference `new`/`old` via anchored keys -- they take the dicts
    /// directly as wildcards).
    fn build_replay_mutate_event(
        &mut self,
        new: &Dictionary,
        old: &Dictionary,
        scratch: &MutateScratch,
    ) -> Statement {
        let MutateScratch {
            btx,
            live_minus_old,
            new_live,
            new_nullifiers,
        } = scratch;
        let m1 = tx_with(btx, "live", Value::from(new_live.clone()));
        let atx = tx_with(&m1, "nullifiers", Value::from(new_nullifiers.clone()));
        let st_nullify = self.build_replay_nullify(old, &m1, &atx, new_nullifiers);

        // Live swap + nullify; chain/event-hash work is delegated to the
        // parent's TxMutate statement.
        let op_sd = self
            .ctx
            .builder
            .priv_op(op!(SetDelete(live_minus_old, (btx, "live"), old)))
            .unwrap();
        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert(new_live, live_minus_old, new)))
            .unwrap();
        let op_du_live = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(m1, btx, "live", new_live)))
            .unwrap();
        let st_event = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayMutateEvent",
                vec![op_sd, op_si, op_du_live, st_nullify],
            )
            .unwrap();
        self.record("ReplayMutateEvent");
        st_event
    }

    /// Build a `ReplayContentsStepMutate` statement: the inlined body
    /// of `ReplayMutate` plus the recursive `ReplayContents` tail.
    /// `old` and `new` are packed into a `pair` dict so they share a
    /// single wildcard slot (keeps the predicate at 8 wildcards). The
    /// TxMutate and guard sub-statements are re-anchored to
    /// `pair.old` / `pair.new`. `scratch` is supplied by the caller
    /// (which already needs `new_live`/`new_nullifiers` for the recursive tail).
    fn build_replay_step_mutate(
        &mut self,
        new: &Dictionary,
        old: &Dictionary,
        scratch: &MutateScratch,
        tx_stmt: Statement,
        guard_evidence: Statement,
        st_rest: Statement,
    ) -> Statement {
        let pair = dict!({
            "old" => old.clone(),
            "new" => new.clone()
        });

        let st_event = self.build_replay_mutate_event(new, old, scratch);

        // Re-anchor TxMutate's `new` (slot 2) and `old` (slot 3) to pair.
        let tx_stmt_wrapped = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, None, Some((&pair, "new")), Some((&pair, "old")), None],
                tx_stmt,
            ))
            .unwrap();
        // Re-anchor guard call's slot 0 (new) to pair.new.
        let rebound_evidence = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    Some((&pair, "new")),
                    Some((&scratch.btx, "chain_start")),
                    Some((&scratch.btx, "chain_end")),
                ],
                guard_evidence,
            ))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayContentsStepMutate",
                vec![tx_stmt_wrapped, st_event, rebound_evidence, st_rest],
            )
            .unwrap();
        self.record("ReplayContentsStepMutate");
        st
    }

    fn build_replay_delete(
        &mut self,
        old: &Dictionary,
        frame: ReplayFrame<'_>,
        tx_stmt: Statement,
        guard_evidence: Statement,
    ) -> (Statement, Set, Set) {
        let btx = frame.to_tx_dict();

        let mut new_live = frame.live.clone();
        new_live.delete(&Value::from(old.commitment())).unwrap();
        let nul = object_nullifier_from_key_hash(object_key_hash(old).unwrap());
        let mut new_nullifiers = frame.nullifiers.clone();
        new_nullifiers.insert(&Value::from(nul)).unwrap();
        let m1 = tx_with(&btx, "live", Value::from(new_live.clone()));
        let atx = tx_with(&m1, "nullifiers", Value::from(new_nullifiers.clone()));

        let st_nullify = self.build_replay_nullify(old, &m1, &atx, &new_nullifiers);

        let op_sd = self
            .ctx
            .builder
            .priv_op(op!(SetDelete(new_live, (&btx, "live"), old)))
            .unwrap();
        let op_du_live = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(m1, btx, "live", new_live)))
            .unwrap();
        let rebound_evidence = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, Some((&btx, "chain_start")), Some((&btx, "chain_end"))],
                guard_evidence,
            ))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayDelete",
                vec![tx_stmt, op_sd, op_du_live, st_nullify, rebound_evidence],
            )
            .unwrap();
        self.record("ReplayDelete");
        (st, new_live, new_nullifiers)
    }

    /// Build `ReplayAction`: open the action scope (rebind
    /// `chain_start`/`chain_end` in the tx context), replay the inner
    /// contents in a child frame, then copy the resulting live and
    /// nullifier sets back into the parent's tx state.
    fn build_replay_action(
        &mut self,
        contents: &[ChainEvent],
        chain: Hash,
        parent: ReplayFrame<'_>,
        chain_after: Hash,
    ) -> (Statement, Set, Set) {
        let btx = parent.to_tx_dict();

        let ms = tx_with(&btx, "chain_start", Value::from(chain));
        let itx = tx_with(&ms, "chain_end", Value::from(chain_after));

        let (st_contents, _next_chain, next_live, next_nulls) =
            self.build_replay_contents(contents, chain, parent.rescope(chain, chain_after));

        let etx = build_tx(&next_live, &next_nulls, chain, chain_after);

        let fm1 = tx_with(&btx, "live", Value::from(next_live.clone()));
        let atx = tx_with(&fm1, "nullifiers", Value::from(next_nulls.clone()));

        // ReplayAction (scope setup + contents + live/nullifier copy-back)
        let op_scope1 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(ms, btx, "chain_start", chain)))
            .unwrap();
        let op_scope2 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(itx, ms, "chain_end", chain_after)))
            .unwrap();
        let op_du1 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(fm1, btx, "live", (&etx, "live"))))
            .unwrap();
        let op_du2 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                atx,
                fm1,
                "nullifiers",
                (&etx, "nullifiers")
            )))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred(
                false,
                "ReplayAction",
                map!({"before_tx" => btx.clone(), "after_tx" => atx.clone(), "before_chain" => chain, "after_chain" => chain_after}),
                vec![op_scope1, op_scope2, st_contents, op_du1, op_du2],
            )
            .unwrap();
        self.record("ReplayAction");
        (st, next_live, next_nulls)
    }

    /// Build a `ReplayActionInsert` statement: the K=1 fast path for a
    /// single top-level action whose body is one Insert. Same shape as
    /// `build_replay_insert`, except the guard call uses `before_chain`
    /// and `after_chain` (the action's chain bounds, which are also the
    /// transaction's chain bounds in the K=1 single-action case)
    /// directly as public args rather than anchoring to
    /// `before_tx.chain_start`/`chain_end`. That means we don't need to
    /// rebind the chain slots of `guard_evidence` -- the literal chain
    /// values it carries from record time already match the public arg
    /// bindings.
    ///
    /// Caller must have verified that `contents` is `[ChainEvent::Insert
    /// { .. }]`; this method panics otherwise. The action's `chain_after`
    /// equals `contents[0].chain_after` by construction (K=1 makes the
    /// single Insert span the whole action), so we read it from there
    /// rather than threading it through a parameter.
    fn build_replay_action_insert(
        &mut self,
        contents: &[ChainEvent],
        parent: ReplayFrame<'_>,
    ) -> (Statement, Set, Set) {
        let ChainEvent::Insert {
            new,
            tx_stmt,
            guard_evidence,
            ..
        } = &contents[0]
        else {
            unreachable!("ReplayActionInsert fast path requires a single Insert event");
        };
        let evidence = guard_evidence
            .clone()
            .expect("missing guard evidence for insert");

        let btx = parent.to_tx_dict();
        let mut new_live = parent.live.clone();
        new_live.insert(&Value::from(new.clone())).unwrap();
        let atx = tx_with(&btx, "live", Value::from(new_live.clone()));

        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert(new_live, (&btx, "live"), new)))
            .unwrap();
        let op_du = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(atx, btx, "live", new_live)))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayActionInsert",
                vec![tx_stmt.clone(), op_si, op_du, evidence],
            )
            .unwrap();
        self.record("ReplayActionInsert");
        (st, new_live, parent.nullifiers.clone())
    }
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
