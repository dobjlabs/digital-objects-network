//! Replay circuit construction for `TxBuilder::finalize`.
//!
//! At finalize time the recorded events form a tree (actions containing
//! events and sub-actions). This module walks that tree and builds the
//! POD2 predicate statements that prove each event's hash step, update
//! the created/nullifier sets, and dispatch each event to its object-type
//! guard.
//!
//! The walker is a `Replayer` that owns the long-lived mutable builder
//! state (`BuildContext` + `TxStats`). A `ReplayFrame` carries the
//! per-step immutable world view -- the current created/nullifier sets
//! plus the chain-scope bounds -- and threads through the recursion.
//! Only `Replayer::build_replay_actions` is `pub(crate)`; every other
//! method here is a private helper it delegates to.

use pod2::{
    frontend::{Operation, entry},
    middleware::{
        Hash, Statement, Value,
        containers::{Dictionary, Set},
    },
};
use pod2utils::{dict, macros::BuildContext, map, op, set, st_custom};

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

/// Per-step immutable world view: the created/nullifier sets and the
/// chain-scope bounds at the current point in the replay walk.
#[derive(Clone, Copy)]
pub(crate) struct ReplayFrame<'a> {
    pub(crate) inputs: &'a Set,
    pub(crate) created: &'a Set,
    pub(crate) nullifiers: &'a Set,
    pub(crate) chain_start: Hash,
    pub(crate) chain_end: Hash,
}

impl<'a> ReplayFrame<'a> {
    /// Derive the next frame after a non-mutating step that only changes the
    /// created set (Insert).
    pub(crate) fn with_created<'b>(self, created: &'b Set) -> ReplayFrame<'b>
    where
        'a: 'b,
    {
        ReplayFrame {
            inputs: self.inputs,
            created,
            nullifiers: self.nullifiers,
            chain_start: self.chain_start,
            chain_end: self.chain_end,
        }
    }

    /// Derive the next frame after a step that updates both sets (Mutate,
    /// Delete, Action, or the top-level `ReplayActions` step).
    pub(crate) fn advance<'b>(self, created: &'b Set, nullifiers: &'b Set) -> ReplayFrame<'b>
    where
        'a: 'b,
    {
        ReplayFrame {
            inputs: self.inputs,
            created,
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
        build_tx(
            self.inputs,
            self.created,
            self.nullifiers,
            self.chain_start,
            self.chain_end,
        )
    }
}

pub(crate) struct MutateScratch {
    pub(crate) btx: Dictionary,
    pub(crate) new_created: Set,
    pub(crate) new_nullifiers: Set,
    source_key: &'static str,
    source: Set,
}

impl<'a> ReplayFrame<'a> {
    fn source_for(self, old: &Dictionary) -> (&'static str, &'a Set) {
        if self.inputs.contains(&Value::from(old.clone())).unwrap() {
            ("inputs", self.inputs)
        } else {
            ("created", self.created)
        }
    }

    pub(crate) fn mutate_scratch(self, old: &Dictionary, new: &Dictionary) -> MutateScratch {
        let btx = self.to_tx_dict();
        let (source_key, source) = self.source_for(old);
        let mut new_created = self.created.clone();
        new_created.insert(&Value::from(new.clone())).unwrap();
        let nul = object_nullifier_from_key_hash(object_key_hash(old).unwrap());
        let mut new_nullifiers = self.nullifiers.clone();
        new_nullifiers.insert(&Value::from(nul)).unwrap();
        MutateScratch {
            btx,
            new_created,
            new_nullifiers,
            source_key,
            source: source.clone(),
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
                let (st_inner, new_created, new_nulls) =
                    self.build_replay_action_insert(contents, frame);
                let st = st_custom!(
                    self.ctx,
                    ReplayActions() = (Statement::None, Statement::None, st_inner)
                )
                .unwrap();
                self.record("ReplayActions");
                return (st, *chain_after, new_created, new_nulls);
            }

            let (st_action, new_created, new_nulls) =
                self.build_replay_action(contents, chain, frame, *chain_after);
            let st = st_custom!(
                self.ctx,
                ReplayActions() = (st_action, Statement::None, Statement::None)
            )
            .unwrap();
            self.record("ReplayActions");
            return (st, *chain_after, new_created, new_nulls);
        }

        // Step: first action + recursive tail.
        let (first, rest) = events.split_first().unwrap();
        let (st_action, next_chain, next_created, next_nulls) =
            self.build_top_level_action(first, chain, frame);
        let (st_rest, final_chain, final_created, final_nulls) =
            self.build_replay_actions(rest, next_chain, frame.advance(&next_created, &next_nulls));
        let st_step = st_custom!(self.ctx, ReplayActionsStep() = (st_action, st_rest)).unwrap();
        self.record("ReplayActionsStep");
        let st = st_custom!(
            self.ctx,
            ReplayActions() = (Statement::None, st_step, Statement::None)
        )
        .unwrap();
        self.record("ReplayActions");
        (st, final_chain, final_created, final_nulls)
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
                let (st, new_created, new_null) =
                    self.build_replay_action(contents, chain, frame, *chain_after);
                (st, *chain_after, new_created, new_null)
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
            let (st_elem, next_chain, next_created, next_nulls) =
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
            return (st, next_chain, next_created, next_nulls);
        }

        let (first, rest) = events.split_first().unwrap();
        let (st_step, tag, final_chain, final_created, final_nulls) = match first {
            ChainEvent::Insert {
                new,
                initial,
                chain_after,
                tx_stmt,
                guard_evidence,
                ..
            } => {
                let evidence = guard_evidence
                    .clone()
                    .expect("missing guard evidence for insert");

                let (st_head, next_created) =
                    self.build_replay_insert(initial, new, frame, tx_stmt.clone(), evidence);
                let (st_rest, final_chain, final_created, final_nulls) = self
                    .build_replay_contents(rest, *chain_after, frame.with_created(&next_created));
                let st = self
                    .ctx
                    .apply_custom_pred_simple(
                        false,
                        "ReplayContentsStepInsert",
                        vec![st_head, st_rest],
                    )
                    .unwrap();
                self.record("ReplayContentsStepInsert");
                (
                    st,
                    EventTag::Insert,
                    final_chain,
                    final_created,
                    final_nulls,
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

                let (st_head, next_created, next_nulls) =
                    self.build_replay_mutate(new, old, frame, tx_stmt.clone(), evidence);
                let (st_rest, final_chain, final_created, final_nulls) = self
                    .build_replay_contents(
                        rest,
                        *chain_after,
                        frame.advance(&next_created, &next_nulls),
                    );
                let st = self
                    .ctx
                    .apply_custom_pred_simple(
                        false,
                        "ReplayContentsStepMutate",
                        vec![st_head, st_rest],
                    )
                    .unwrap();
                self.record("ReplayContentsStepMutate");
                (
                    st,
                    EventTag::Mutate,
                    final_chain,
                    final_created,
                    final_nulls,
                )
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
                let (st_head, next_created, next_nulls) =
                    self.build_replay_delete(old, frame, tx_stmt.clone(), evidence);
                let (st_rest, final_chain, final_created, final_nulls) = self
                    .build_replay_contents(
                        rest,
                        *chain_after,
                        frame.advance(&next_created, &next_nulls),
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
                (
                    st,
                    EventTag::Delete,
                    final_chain,
                    final_created,
                    final_nulls,
                )
            }
            ChainEvent::Action {
                chain_after,
                contents,
                ..
            } => {
                let (st_head, next_created, next_nulls) =
                    self.build_replay_action(contents, chain, frame, *chain_after);
                let (st_rest, final_chain, final_created, final_nulls) = self
                    .build_replay_contents(
                        rest,
                        *chain_after,
                        frame.advance(&next_created, &next_nulls),
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
                (
                    st,
                    EventTag::Action,
                    final_chain,
                    final_created,
                    final_nulls,
                )
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
        (st, final_chain, final_created, final_nulls)
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
                initial,
                chain_after,
                tx_stmt,
                guard_evidence,
                ..
            } => {
                let evidence = guard_evidence
                    .clone()
                    .expect("missing guard evidence for insert");
                let (st, new_created) =
                    self.build_replay_insert(initial, new, frame, tx_stmt.clone(), evidence);
                (
                    st,
                    EventTag::Insert,
                    *chain_after,
                    new_created,
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
                let (st, new_created, new_null) =
                    self.build_replay_mutate(new, old, frame, tx_stmt.clone(), evidence);
                (st, EventTag::Mutate, *chain_after, new_created, new_null)
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
                let (st, new_created, new_null) =
                    self.build_replay_delete(old, frame, tx_stmt.clone(), evidence);
                (st, EventTag::Delete, *chain_after, new_created, new_null)
            }
            ChainEvent::Action {
                chain_after,
                contents,
                ..
            } => {
                let (st, new_created, new_null) =
                    self.build_replay_action(contents, chain, frame, *chain_after);
                (st, EventTag::Action, *chain_after, new_created, new_null)
            }
        }
    }

    fn build_replay_element(
        &mut self,
        event: &ChainEvent,
        chain: Hash,
        frame: ReplayFrame<'_>,
    ) -> (Statement, Hash, Set, Set) {
        let (st_inner, tag, next_chain, next_created, next_nulls) =
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
        (st, next_chain, next_created, next_nulls)
    }

    fn build_replay_insert(
        &mut self,
        initial: &Dictionary,
        new: &Dictionary,
        frame: ReplayFrame<'_>,
        tx_stmt: Statement,
        guard_evidence: Statement,
    ) -> (Statement, Set) {
        // ReplayInsert's `initial` private wildcard is bound by the
        // TxInsert sub-statement we pass in; no standalone op needed.
        let btx = frame.to_tx_dict();
        let mut new_created = frame.created.clone();
        new_created.insert(&Value::from(new.clone())).unwrap();
        let atx = tx_with(&btx, "created", Value::from(new_created.clone()));

        let pair = dict!({
            "initial" => initial.clone(),
            "new_created" => new_created.clone()
        });

        let tx_stmt_wrapped = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    None,
                    None,
                    Some(entry(&pair, "initial").expect("referenced container entry exists")),
                    None,
                    None,
                ],
                tx_stmt,
            ))
            .unwrap();

        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert(
                entry(&btx, "created").expect("referenced container entry exists"),
                new,
                entry(&pair, "new_created").expect("referenced container entry exists")
            )))
            .unwrap();
        let op_du = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                btx,
                "created",
                entry(&pair, "new_created").expect("referenced container entry exists"),
                atx
            )))
            .unwrap();
        let rebound_evidence = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    None,
                    None,
                    Some(entry(&btx, "chain_start").expect("referenced container entry exists")),
                    Some(entry(&btx, "chain_end").expect("referenced container entry exists")),
                ],
                guard_evidence,
            ))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayInsert",
                vec![tx_stmt_wrapped, op_si, op_du, rebound_evidence],
            )
            .unwrap();
        self.record("ReplayInsert");
        (st, new_created)
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
                    None,
                    Some(
                        entry(&scratch.btx, "chain_start")
                            .expect("referenced container entry exists"),
                    ),
                    Some(
                        entry(&scratch.btx, "chain_end")
                            .expect("referenced container entry exists"),
                    ),
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
            new_created,
            new_nullifiers,
            ..
        } = scratch;
        (st, new_created, new_nullifiers)
    }

    fn build_replay_nullify(
        &mut self,
        old: &Dictionary,
        mid_tx: &Dictionary,
        after_tx: &Dictionary,
        new_nullifiers: &Set,
        source: &Set,
    ) -> Statement {
        let op_contains = self
            .ctx
            .builder
            .priv_op(op!(SetContains(source, old)))
            .unwrap();
        let okh = object_key_hash(old).unwrap();
        let nul = object_nullifier_from_key_hash(okh);
        let op_h1 = self
            .ctx
            .builder
            .priv_op(op!(Hash(
                old,
                entry(old, "key").expect("referenced container entry exists"),
                okh
            )))
            .unwrap();
        let op_h2 = self
            .ctx
            .builder
            .priv_op(op!(Hash(okh, OBJECT_NULLIFIER_VERSION, nul)))
            .unwrap();
        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert(
                entry(mid_tx, "nullifiers").expect("referenced container entry exists"),
                nul,
                new_nullifiers
            )))
            .unwrap();
        let op_du_null = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                mid_tx,
                "nullifiers",
                new_nullifiers,
                after_tx
            )))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayNullify",
                vec![op_contains, op_h1, op_h2, op_si, op_du_null],
            )
            .unwrap();
        self.record("ReplayNullify");
        st
    }

    fn build_replay_mutate_event(
        &mut self,
        new: &Dictionary,
        old: &Dictionary,
        scratch: &MutateScratch,
    ) -> Statement {
        let MutateScratch {
            btx,
            source_key,
            source,
            new_created,
            new_nullifiers,
        } = scratch;
        let m1 = tx_with(btx, "created", Value::from(new_created.clone()));
        let atx = tx_with(&m1, "nullifiers", Value::from(new_nullifiers.clone()));
        let st_nullify = self.build_replay_nullify(old, &m1, &atx, new_nullifiers, source);

        let op_source_key = self
            .ctx
            .builder
            .priv_op(op!(SetContains(set!("inputs", "created"), *source_key)))
            .unwrap();
        let op_source = self
            .ctx
            .builder
            .priv_op(op!(DictContains(btx, *source_key, source)))
            .unwrap();
        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert(
                entry(btx, "created").unwrap(),
                new,
                new_created
            )))
            .unwrap();
        let op_du_created = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(btx, "created", new_created, m1)))
            .unwrap();
        let st_event = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayMutateEvent",
                vec![op_source_key, op_source, op_si, op_du_created, st_nullify],
            )
            .unwrap();
        self.record("ReplayMutateEvent");
        st_event
    }

    fn build_replay_delete(
        &mut self,
        old: &Dictionary,
        frame: ReplayFrame<'_>,
        tx_stmt: Statement,
        guard_evidence: Statement,
    ) -> (Statement, Set, Set) {
        let btx = frame.to_tx_dict();

        let (source_key, source) = frame.source_for(old);
        let nul = object_nullifier_from_key_hash(object_key_hash(old).unwrap());
        let mut new_nullifiers = frame.nullifiers.clone();
        new_nullifiers.insert(&Value::from(nul)).unwrap();
        let atx = tx_with(&btx, "nullifiers", Value::from(new_nullifiers.clone()));
        let pair = dict!({
            "source_key" => source_key,
            "source" => source.clone()
        });
        let st_nullify = self.build_replay_nullify(old, &btx, &atx, &new_nullifiers, source);
        let st_nullify_wrapped = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, None, None, Some(entry(&pair, "source").unwrap())],
                st_nullify,
            ))
            .unwrap();
        let op_source_key = self
            .ctx
            .builder
            .priv_op(op!(SetContains(
                set!("inputs", "created"),
                entry(&pair, "source_key").unwrap()
            )))
            .unwrap();
        let op_source = self
            .ctx
            .builder
            .priv_op(op!(DictContains(
                btx,
                entry(&pair, "source_key").unwrap(),
                entry(&pair, "source").unwrap()
            )))
            .unwrap();
        let rebound_evidence = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    None,
                    None,
                    Some(entry(&btx, "chain_start").expect("referenced container entry exists")),
                    Some(entry(&btx, "chain_end").expect("referenced container entry exists")),
                ],
                guard_evidence,
            ))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayDelete",
                vec![
                    tx_stmt,
                    op_source_key,
                    op_source,
                    st_nullify_wrapped,
                    rebound_evidence,
                ],
            )
            .unwrap();
        self.record("ReplayDelete");
        (st, frame.created.clone(), new_nullifiers)
    }

    /// Build `ReplayAction`: open the action scope (rebind
    /// `chain_start`/`chain_end` in the tx context), replay the inner
    /// contents in a child frame, then copy the resulting created and
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

        let (st_contents, _next_chain, next_created, next_nulls) =
            self.build_replay_contents(contents, chain, parent.rescope(chain, chain_after));

        let etx = build_tx(
            parent.inputs,
            &next_created,
            &next_nulls,
            chain,
            chain_after,
        );

        let fm1 = tx_with(&btx, "created", Value::from(next_created.clone()));
        let atx = tx_with(&fm1, "nullifiers", Value::from(next_nulls.clone()));

        let pair = dict!({
            "scope_mid" => ms.clone(),
            "mid" => fm1.clone()
        });

        // ReplayAction (scope setup + contents + created/nullifier copy-back)
        let op_scope1 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                btx,
                "chain_start",
                chain,
                entry(&pair, "scope_mid").expect("referenced container entry exists")
            )))
            .unwrap();
        let op_scope2 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                entry(&pair, "scope_mid").expect("referenced container entry exists"),
                "chain_end",
                chain_after,
                itx
            )))
            .unwrap();
        let op_du1 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                btx,
                "created",
                entry(&etx, "created").expect("referenced container entry exists"),
                entry(&pair, "mid").expect("referenced container entry exists")
            )))
            .unwrap();
        let op_du2 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                entry(&pair, "mid").expect("referenced container entry exists"),
                "nullifiers",
                entry(&etx, "nullifiers").expect("referenced container entry exists"),
                atx
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
        (st, next_created, next_nulls)
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
            initial,
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
        let mut new_created = parent.created.clone();
        new_created.insert(&Value::from(new.clone())).unwrap();
        let atx = tx_with(&btx, "created", Value::from(new_created.clone()));

        let pair = dict!({
            "initial" => initial.clone(),
            "new_created" => new_created.clone()
        });

        let tx_stmt_wrapped = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    None,
                    None,
                    Some(entry(&pair, "initial").expect("referenced container entry exists")),
                    None,
                    None,
                ],
                tx_stmt.clone(),
            ))
            .unwrap();

        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert(
                entry(&btx, "created").expect("referenced container entry exists"),
                new,
                entry(&pair, "new_created").expect("referenced container entry exists")
            )))
            .unwrap();
        let op_du = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                btx,
                "created",
                entry(&pair, "new_created").expect("referenced container entry exists"),
                atx
            )))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayActionInsert",
                vec![tx_stmt_wrapped, op_si, op_du, evidence],
            )
            .unwrap();
        self.record("ReplayActionInsert");
        (st, new_created, parent.nullifiers.clone())
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

#[cfg(test)]
mod tests {
    use super::*;
    use pod2::{
        frontend::MultiPodBuilder,
        middleware::{Params, VDSet},
    };
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::Arc,
    };

    fn context() -> BuildContext {
        BuildContext::new(
            MultiPodBuilder::new(&Params::default(), &VDSet::new(&[])),
            vec![
                Arc::new(crate::predicates::events_module()),
                Arc::new(crate::predicates::module()),
            ],
        )
    }

    #[test]
    fn rejects_unapproved_source_field() {
        let old = dict!({"key" => 1_i64});
        let new = dict!({"key" => 2_i64});
        let empty = set!();
        let forged_source = set!(old);
        let zero = empty.commitment();
        let frame = ReplayFrame {
            inputs: &empty,
            created: &empty,
            nullifiers: &forged_source,
            chain_start: zero,
            chain_end: zero,
        };
        let mut scratch = frame.mutate_scratch(&old, &new);
        scratch.source_key = "nullifiers";
        scratch.source = forged_source;
        let mut ctx = context();
        let mut stats = TxStats::new();
        let result = catch_unwind(AssertUnwindSafe(|| {
            Replayer::new(&mut ctx, &mut stats).build_replay_mutate_event(&new, &old, &scratch)
        }));
        assert!(
            result.is_err(),
            "nullifiers must not be usable as an existence source"
        );
        assert_eq!(stats.get("ReplayNullify"), Some(&1));
        assert!(!stats.contains_key("ReplayMutateEvent"));
    }

    #[test]
    fn rejects_mutation_using_its_own_output_as_source() {
        let object = dict!({"key" => 1_i64});
        let empty = set!();
        let zero = empty.commitment();
        let frame = ReplayFrame {
            inputs: &empty,
            created: &empty,
            nullifiers: &empty,
            chain_start: zero,
            chain_end: zero,
        };
        let mut scratch = frame.mutate_scratch(&object, &object);
        scratch.source = scratch.new_created.clone();
        let mut ctx = context();
        let mut stats = TxStats::new();
        let result = catch_unwind(AssertUnwindSafe(|| {
            Replayer::new(&mut ctx, &mut stats)
                .build_replay_mutate_event(&object, &object, &scratch)
        }));
        assert!(
            result.is_err(),
            "the source must be bound to the pre-mutation context"
        );
        assert_eq!(stats.get("ReplayNullify"), Some(&1));
        assert!(!stats.contains_key("ReplayMutateEvent"));
    }

    #[test]
    fn rejects_second_consumption_even_when_source_still_contains_state() {
        let old = dict!({"key" => 1_i64});
        let source = set!(old);
        let empty = set!();
        let zero = empty.commitment();
        let before_tx = build_tx(&empty, &source, &empty, zero, zero);
        let nullifiers = set!(crate::compute_nullifier(&old));
        let after_tx = tx_with(&before_tx, "nullifiers", Value::from(nullifiers.clone()));
        let mut ctx = context();
        let mut stats = TxStats::new();
        let mut replayer = Replayer::new(&mut ctx, &mut stats);
        replayer.build_replay_nullify(&old, &before_tx, &after_tx, &nullifiers, &source);
        let result = catch_unwind(AssertUnwindSafe(|| {
            replayer.build_replay_nullify(&old, &after_tx, &after_tx, &nullifiers, &source)
        }));
        assert!(result.is_err(), "SetInsert must require a fresh nullifier");
        assert_eq!(stats.get("ReplayNullify"), Some(&1));
    }
}
