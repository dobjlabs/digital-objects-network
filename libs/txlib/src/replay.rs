//! Replay circuit construction for `TxBuilder::finalize`.
//!
//! Finalization walks the recorded action tree and proves each event's hash
//! step, set updates, and class-guard dispatch. `Replayer` owns the mutable
//! proof state; `ReplayFrame` carries set and chain-scope snapshots through
//! the recursion.

use pod2::{
    frontend::Operation,
    middleware::{
        Hash, Statement, Value,
        containers::{Dictionary, Set},
    },
};
use pod2utils::{dict, macros::BuildContext, map, op, st_custom};

use crate::{
    ChainEvent, SpendFacts, StateFacts, TxStats, build_tx, object_key_hash,
    object_nullifier_from_key_hash, prove_endorse_spend, record, tx_with,
};

/// Replay walker and its transaction-wide proof state.
///
/// Guards bind to `context.state_header`; spends endorse the full context.
pub(crate) struct Replayer<'a> {
    ctx: &'a mut BuildContext,
    stats: &'a mut TxStats,
    context: &'a Dictionary,
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

/// Owned snapshots shared by a mutation's proof and the following frame.
pub(crate) struct MutateScratch {
    pub(crate) btx: Dictionary,
    pub(crate) live_minus_old: Set,
    pub(crate) new_live: Set,
    pub(crate) new_nullifiers: Set,
}

impl<'a> ReplayFrame<'a> {
    /// Compute the pre-mutate tx context + post-mutate set snapshots
    /// for a `(old -> new)` mutate.
    pub(crate) fn mutate_scratch(self, old: &SpendFacts, new: &StateFacts) -> MutateScratch {
        let btx = self.to_tx_dict();
        let mut live_minus_old = self.live.clone();
        live_minus_old.delete(&old.state().value()).unwrap();
        let mut new_live = live_minus_old.clone();
        new_live.insert(&new.value()).unwrap();
        let mut new_nullifiers = self.nullifiers.clone();
        new_nullifiers
            .insert(&Value::from(old.nullifier()))
            .unwrap();
        MutateScratch {
            btx,
            live_minus_old,
            new_live,
            new_nullifiers,
        }
    }
}

impl<'a> Replayer<'a> {
    pub(crate) fn new(
        ctx: &'a mut BuildContext,
        stats: &'a mut TxStats,
        context: &'a Dictionary,
    ) -> Self {
        Self {
            ctx,
            stats,
            context,
        }
    }

    fn record(&mut self, name: &str) {
        record(self.stats, name);
    }

    /// Build `ReplayActions` for the nonempty top-level action list.
    ///
    /// A single action containing one insert uses `ReplayActionInsert`,
    /// reducing the path from five statements to two. Multiple actions use
    /// `ReplayAction` because `ReplayActionsStep` requires it.
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

    /// Build `ReplayContents` for a nonempty event list.
    /// K=1 uses `ReplayElement`; K>=2 specializes on the head event type.
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

        // Insert and Mutate inline their replay bodies and pack private values
        // to stay within pod2's wildcard limit. Delete keeps ReplayDelete
        // because it already reaches the five-substatement limit.
        let (first, rest) = events.split_first().unwrap();
        let (st_step, tag, final_chain, final_live, final_nulls) = match first {
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

                let (st_head, next_live) =
                    self.build_replay_insert(initial, new, frame, tx_stmt.clone(), evidence);
                let (st_rest, final_chain, final_live, final_nulls) =
                    self.build_replay_contents(rest, *chain_after, frame.with_live(&next_live));
                let st = self
                    .ctx
                    .apply_custom_pred_simple(
                        false,
                        "ReplayContentsStepInsert",
                        vec![st_head, st_rest],
                    )
                    .unwrap();
                self.record("ReplayContentsStepInsert");
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

                let (st_head, next_live, next_nulls) =
                    self.build_replay_mutate(new, old, frame, tx_stmt.clone(), evidence);
                let (st_rest, final_chain, final_live, final_nulls) = self.build_replay_contents(
                    rest,
                    *chain_after,
                    frame.advance(&next_live, &next_nulls),
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

    /// Build one `Replay<X>` statement and return its event tag.
    /// Both `ReplayElement` and `ReplayContentsStep<X>` use this result.
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
                let (st, new_live) =
                    self.build_replay_insert(initial, new, frame, tx_stmt.clone(), evidence);
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
        initial: &Dictionary,
        new: &Dictionary,
        frame: ReplayFrame<'_>,
        tx_stmt: Statement,
        guard_evidence: Statement,
    ) -> (Statement, Set) {
        // ReplayInsert's `initial` private wildcard is bound by the
        // TxInsert sub-statement we pass in; no standalone op needed.
        let btx = frame.to_tx_dict();
        let mut new_live = frame.live.clone();
        new_live.insert(&Value::from(new.clone())).unwrap();
        let atx = tx_with(&btx, "live", Value::from(new_live.clone()));

        let pair = dict!({
            "initial" => initial.clone(),
            "new_live" => new_live.clone()
        });

        let tx_stmt_wrapped = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, None, Some((&pair, "initial")), None, None],
                tx_stmt,
            ))
            .unwrap();

        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert((&btx, "live"), new, (&pair, "new_live"))))
            .unwrap();
        let op_du = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(btx, "live", (&pair, "new_live"), atx)))
            .unwrap();
        let rebound_evidence = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    None,
                    Some((self.context, "state_header")),
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
                "ReplayInsert",
                vec![tx_stmt_wrapped, op_si, op_du, rebound_evidence],
            )
            .unwrap();
        self.record("ReplayInsert");
        (st, new_live)
    }

    fn build_replay_mutate(
        &mut self,
        new: &StateFacts,
        old: &SpendFacts,
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
                    Some((self.context, "state_header")),
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

    /// Authorize a spend and add its nullifier to `mid_tx`.
    /// Used by mutation and deletion replay.
    fn build_replay_nullify(
        &mut self,
        old: &SpendFacts,
        mid_tx: &Dictionary,
        after_tx: &Dictionary,
        new_nullifiers: &Set,
    ) -> Statement {
        // Use a supplied endorsement when the old key is unavailable.
        let st_endorse = match (old.endorsement(), old.dict()) {
            (Some(st), _) => st.clone(),
            (None, Some(d)) => self.build_endorse_spend(d),
            (None, None) => unreachable!("SpendFacts::Statements always carries an endorsement"),
        };

        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert(
                (mid_tx, "nullifiers"),
                old.nullifier(),
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
            .apply_custom_pred_simple(false, "ReplayNullify", vec![st_endorse, op_si, op_du_null])
            .unwrap();
        self.record("ReplayNullify");
        st
    }

    /// Build `EndorseSpend` for a locally available state.
    /// This waits until finalization, when the transaction context exists.
    fn build_endorse_spend(&mut self, old: &Dictionary) -> Statement {
        let context = Value::from(self.context.commitment());
        let (_, st) = prove_endorse_spend(self.ctx, false, context, old);
        self.record("EndorseSpend");
        st
    }

    /// Build the live-set swap and nullification for a mutation.
    /// The predicates take state commitments directly, so indirect states
    /// need no additional openings here.
    fn build_replay_mutate_event(
        &mut self,
        new: &StateFacts,
        old: &SpendFacts,
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
            .priv_op(op!(SetDelete(
                (btx, "live"),
                old.state().value(),
                live_minus_old
            )))
            .unwrap();
        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert(live_minus_old, new.value(), new_live)))
            .unwrap();
        let op_du_live = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(btx, "live", new_live, m1)))
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

        let pair = dict!({
            "new_live" => new_live.clone(),
            "mid_tx" => m1.clone()
        });

        // Deletes are always of a state this builder holds, so there is
        // no indirect-side delete path.
        let st_nullify =
            self.build_replay_nullify(&SpendFacts::Dict(old.clone()), &m1, &atx, &new_nullifiers);
        let st_nullify_wrapped = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, Some((&pair, "mid_tx")), None, None],
                st_nullify.clone(),
            ))
            .unwrap();

        let op_sd = self
            .ctx
            .builder
            .priv_op(op!(SetDelete((&btx, "live"), old, (&pair, "new_live"))))
            .unwrap();
        let op_du_live = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                btx,
                "live",
                (&pair, "new_live"),
                (&pair, "mid_tx")
            )))
            .unwrap();
        let rebound_evidence = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    None,
                    Some((self.context, "state_header")),
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
                "ReplayDelete",
                vec![
                    tx_stmt,
                    op_sd,
                    op_du_live,
                    st_nullify_wrapped,
                    rebound_evidence,
                ],
            )
            .unwrap();
        self.record("ReplayDelete");
        (st, new_live, new_nullifiers)
    }

    /// Replay an action in its chain scope and copy its sets to the parent.
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

        let pair = dict!({
            "scope_mid" => ms.clone(),
            "mid" => fm1.clone()
        });

        // ReplayAction (scope setup + contents + live/nullifier copy-back)
        let op_scope1 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                btx,
                "chain_start",
                chain,
                (&pair, "scope_mid")
            )))
            .unwrap();
        let op_scope2 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                (&pair, "scope_mid"),
                "chain_end",
                chain_after,
                itx
            )))
            .unwrap();
        let op_du1 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(btx, "live", (&etx, "live"), (&pair, "mid"))))
            .unwrap();
        let op_du2 = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(
                (&pair, "mid"),
                "nullifiers",
                (&etx, "nullifiers"),
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
        (st, next_live, next_nulls)
    }

    /// Fast path for one top-level action containing one insert.
    ///
    /// The action and transaction chain bounds are identical, so the guard's
    /// recorded bounds already match the public arguments; only its state
    /// header needs rebinding. The caller validates the event shape.
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
        let mut new_live = parent.live.clone();
        new_live.insert(&Value::from(new.clone())).unwrap();
        let atx = tx_with(&btx, "live", Value::from(new_live.clone()));

        let pair = dict!({
            "initial" => initial.clone(),
            "new_live" => new_live.clone()
        });

        let tx_stmt_wrapped = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, None, Some((&pair, "initial")), None, None],
                tx_stmt.clone(),
            ))
            .unwrap();

        let op_si = self
            .ctx
            .builder
            .priv_op(op!(SetInsert((&btx, "live"), new, (&pair, "new_live"))))
            .unwrap();
        let op_du = self
            .ctx
            .builder
            .priv_op(op!(DictUpdate(btx, "live", (&pair, "new_live"), atx)))
            .unwrap();
        let rebound_evidence = self
            .ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, Some((self.context, "state_header")), None, None],
                evidence,
            ))
            .unwrap();
        let st = self
            .ctx
            .apply_custom_pred_simple(
                false,
                "ReplayActionInsert",
                vec![tx_stmt_wrapped, op_si, op_du, rebound_evidence],
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
