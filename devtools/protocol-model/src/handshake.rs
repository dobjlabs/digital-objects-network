use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use stateright::{
    Expectation,
    actor::{
        Actor, ActorModel, ActorModelAction, ActorModelState, Envelope, Id, Network, Out,
        model_timeout,
    },
};

use crate::objects::Objects;

/// Identifies one reservation. A real store draws these at random; here they stay
/// unique because each store restart begins a new epoch.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Reservation {
    pub epoch: u8,
    pub sequence: u8,
}

impl fmt::Debug for Reservation {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "r{}.{}", self.epoch, self.sequence)
    }
}

/// Requests from an orchestrator to the store and the store's replies, and the
/// store's exchange with the synchronizer. The network may delay or reorder any
/// message without bound; an orchestrator repeats its outstanding request after a
/// restart.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Message {
    /// Reserve these inputs exclusively.
    Reserve(Objects),
    /// The requested inputs are reserved under this reservation, with a lease.
    Reserved(Reservation),
    /// Some requested input is held by another reservation or pending transaction.
    Refused,
    /// Extend this reservation's lease.
    Renew(Reservation),
    /// End this reservation's lease.
    Release(Reservation),
    /// No lease remains under this reservation.
    Released(Reservation),
    /// The reservation's transaction is pending, so its inputs stay protected.
    StillPending(Reservation),
    /// Save the transaction and the contents of the objects it creates as pending
    /// under this reservation. Repeating the request returns the recorded result.
    MarkPending {
        reservation: Reservation,
        outputs: Objects,
    },
    /// The transaction under this reservation is saved as pending.
    MarkedPending(Reservation),
    /// No lease is active and no transaction is pending under this reservation.
    NotReserved(Reservation),
    /// The reservation's transaction was pending and has since been settled.
    Ended(Reservation),
    /// The store starts watching for a pending transaction's effects on chain.
    Watch {
        reservation: Reservation,
        inputs: Objects,
        outputs: Objects,
    },
    /// The synchronizer has settled the transaction: the verdict will not change.
    Settled {
        reservation: Reservation,
        verdict: Verdict,
    },
}

/// What the chain finally shows of a transaction's effects. All of its inputs
/// consumed and all of its outputs created means the transaction landed. Anything
/// less means it ended without landing, and whatever is present belongs to other
/// transactions: the chain applies a transaction whole or not at all.
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Verdict {
    pub consumed: Objects,
    pub created: Objects,
}

impl fmt::Debug for Verdict {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "consumed {:?} created {:?}",
            self.consumed, self.created
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Timer {
    /// The store ends this reservation's lease.
    LeaseExpiry(Reservation),
    /// The synchronizer settles this reservation's transaction with this verdict.
    Settle(Reservation, Verdict),
    /// The orchestrator's proof is ready.
    ProofReady,
    /// The orchestrator renews its lease. With time abstracted, one renewal per lease,
    /// racing its expiry, stands for periodic renewals.
    Renew,
    /// The action is cancelled before the store acknowledges it.
    GiveUp,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Lease {
    pub inputs: Objects,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PendingTransaction {
    pub inputs: Objects,
    pub outputs: Objects,
    /// Set once the synchronizer settled the transaction. The record is kept.
    pub verdict: Option<Verdict>,
}

impl PendingTransaction {
    pub fn ended(&self) -> bool {
        self.verdict.is_some()
    }

    pub fn landed(&self) -> bool {
        self.verdict
            == Some(Verdict {
                consumed: self.inputs,
                created: self.outputs,
            })
    }
}

/// What the store keeps across restarts.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct StoreStorage {
    pub epoch: u8,
    pub pending: BTreeMap<Reservation, PendingTransaction>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StoreState {
    pub saved: StoreStorage,
    /// Leases granted in this epoch; ended leases stay recorded as inactive.
    pub leases: BTreeMap<Reservation, Lease>,
}

impl StoreState {
    pub fn leases_cover(&self, inputs: Objects) -> bool {
        self.leases
            .values()
            .any(|lease| lease.active && lease.inputs.overlaps(inputs))
    }

    /// Whether a pending transaction not yet settled holds any of the inputs.
    pub fn pending_covers(&self, inputs: Objects) -> bool {
        self.saved
            .pending
            .values()
            .any(|transaction| !transaction.ended() && transaction.inputs.overlaps(inputs))
    }

    /// Inputs consumed on chain, which no transaction can use again.
    pub fn nullified(&self) -> Objects {
        self.saved
            .pending
            .values()
            .filter_map(|transaction| transaction.verdict)
            .fold(Objects::default(), |consumed, verdict| {
                consumed.union(verdict.consumed)
            })
    }

    pub fn lease_active(&self, reservation: Reservation) -> bool {
        self.leases
            .get(&reservation)
            .is_some_and(|lease| lease.active)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Outcome {
    Submitted,
    Abandoned,
}

/// What an orchestrator keeps across restarts.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Journal {
    /// Set once the proof and payload are saved: the reservation to mark them pending
    /// under, replaced if the orchestrator has to reserve again.
    pub reservation: Option<Reservation>,
    pub outcome: Option<Outcome>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Stage {
    Reserving,
    Proving(Reservation),
    MarkingPending(Reservation),
    Finished,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OrchestratorState {
    pub journal: Journal,
    pub stage: Stage,
    /// Started from a saved journal rather than for the first time. Only the checks
    /// read it.
    pub restarted: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ParticipantState {
    Store(StoreState),
    Orchestrator(OrchestratorState),
    Synchronizer(SynchronizerState),
}

impl ParticipantState {
    fn store(&self) -> &StoreState {
        match self {
            Self::Store(state) => state,
            _ => unreachable!("the store actor holds store state"),
        }
    }

    fn store_mut(&mut self) -> &mut StoreState {
        match self {
            Self::Store(state) => state,
            _ => unreachable!("the store actor holds store state"),
        }
    }

    fn orchestrator(&self) -> &OrchestratorState {
        match self {
            Self::Orchestrator(state) => state,
            _ => unreachable!("an orchestrator actor holds orchestrator state"),
        }
    }

    fn orchestrator_mut(&mut self) -> &mut OrchestratorState {
        match self {
            Self::Orchestrator(state) => state,
            _ => unreachable!("an orchestrator actor holds orchestrator state"),
        }
    }

    fn synchronizer_mut(&mut self) -> &mut SynchronizerState {
        match self {
            Self::Synchronizer(state) => state,
            _ => unreachable!("the synchronizer actor holds synchronizer state"),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ParticipantStorage {
    Store(StoreStorage),
    Orchestrator(Journal),
}

impl ParticipantStorage {
    fn store(&self) -> &StoreStorage {
        match self {
            Self::Store(saved) => saved,
            Self::Orchestrator(_) => unreachable!("the store actor saves store storage"),
        }
    }

    fn journal(&self) -> &Journal {
        match self {
            Self::Orchestrator(journal) => journal,
            Self::Store(_) => unreachable!("an orchestrator actor saves a journal"),
        }
    }
}

/// Grants a fresh reservation with a lease on the inputs.
pub fn grant(state: &mut StoreState, to: Id, inputs: Objects, o: &mut Out<Participant>) {
    let reservation = Reservation {
        epoch: state.saved.epoch,
        sequence: state.leases.len() as u8,
    };
    state.leases.insert(
        reservation,
        Lease {
            inputs,
            active: true,
        },
    );
    o.set_timer(Timer::LeaseExpiry(reservation), model_timeout());
    o.send(to, Message::Reserved(reservation));
}

pub fn end_lease(state: &mut StoreState, reservation: Reservation, o: &mut Out<Participant>) {
    if let Some(lease) = state.leases.get_mut(&reservation) {
        lease.active = false;
        o.cancel_timer(Timer::LeaseExpiry(reservation));
    }
}

pub fn save(storage: &StoreStorage, o: &mut Out<Participant>) {
    o.save(ParticipantStorage::Store(storage.clone()));
}

/// Ends the reservation's lease, saves its transaction as pending, starts watching for its
/// effects, and acknowledges.
pub fn record_pending(
    store: &Store,
    state: &mut StoreState,
    from: Id,
    reservation: Reservation,
    outputs: Objects,
    o: &mut Out<Participant>,
) {
    let inputs = state.leases[&reservation].inputs;
    end_lease(state, reservation, o);
    state.saved.pending.insert(
        reservation,
        PendingTransaction {
            inputs,
            outputs,
            verdict: None,
        },
    );
    save(&state.saved, o);
    o.send(
        store.synchronizer,
        Message::Watch {
            reservation,
            inputs,
            outputs,
        },
    );
    o.send(from, Message::MarkedPending(reservation));
}

/// How the store handles each request and timer. The default methods are the
/// protocol's rules; a test can substitute a store that breaks one of them.
pub trait StoreRules: Sync {
    /// The storage the store resumes from. Each restart begins a new epoch, so reservations
    /// granted before it are never granted again.
    fn restart(&self, previous: Option<&StoreStorage>) -> StoreStorage {
        previous.map_or_else(StoreStorage::default, |previous| StoreStorage {
            epoch: previous.epoch + 1,
            pending: previous.pending.clone(),
        })
    }

    fn reserve(
        &self,
        _: &Store,
        state: &mut StoreState,
        from: Id,
        inputs: Objects,
        o: &mut Out<Participant>,
    ) {
        if state.leases_cover(inputs)
            || state.pending_covers(inputs)
            || state.nullified().overlaps(inputs)
        {
            o.send(from, Message::Refused);
        } else {
            grant(state, from, inputs, o);
        }
    }

    fn renew(
        &self,
        _: &Store,
        state: &mut StoreState,
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        if state.lease_active(reservation) {
            o.set_timer(Timer::LeaseExpiry(reservation), model_timeout());
        }
    }

    fn release(
        &self,
        _: &Store,
        state: &mut StoreState,
        from: Id,
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        match state.saved.pending.get(&reservation) {
            Some(transaction) if transaction.ended() => o.send(from, Message::Ended(reservation)),
            Some(_) => o.send(from, Message::StillPending(reservation)),
            None => {
                end_lease(state, reservation, o);
                o.send(from, Message::Released(reservation));
            }
        }
    }

    fn mark_pending(
        &self,
        store: &Store,
        state: &mut StoreState,
        from: Id,
        reservation: Reservation,
        outputs: Objects,
        o: &mut Out<Participant>,
    ) {
        match state.saved.pending.get(&reservation) {
            Some(transaction) if transaction.ended() => o.send(from, Message::Ended(reservation)),
            Some(_) => o.send(from, Message::MarkedPending(reservation)),
            None if state.lease_active(reservation) => {
                record_pending(store, state, from, reservation, outputs, o)
            }
            None => o.send(from, Message::NotReserved(reservation)),
        }
    }

    /// Saves the verdict on the pending transaction, which keeps its record. Its
    /// consumed inputs are nullified from here on and its other inputs are free.
    fn settled(
        &self,
        _: &Store,
        state: &mut StoreState,
        reservation: Reservation,
        verdict: Verdict,
        o: &mut Out<Participant>,
    ) {
        let Some(transaction) = state.saved.pending.get_mut(&reservation) else {
            return;
        };
        if transaction.ended() {
            return;
        }
        transaction.verdict = Some(verdict);
        save(&state.saved, o);
    }

    fn lease_expired(
        &self,
        _: &Store,
        state: &mut StoreState,
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        end_lease(state, reservation, o);
    }
}

/// The store that follows the protocol.
pub struct ProtocolStore;

impl StoreRules for ProtocolStore {}

/// Keeps reservations in memory and pending transactions in storage, applying one
/// update at a time: a request, a lease expiring, or a settlement from the
/// synchronizer.
#[derive(Clone)]
pub struct Store {
    pub rules: &'static dyn StoreRules,
    pub synchronizer: Id,
}

impl Store {
    fn start(&self, storage: Option<&StoreStorage>, o: &mut Out<Participant>) -> StoreState {
        let saved = self.rules.restart(storage);
        save(&saved, o);
        StoreState {
            saved,
            leases: BTreeMap::new(),
        }
    }

    fn on_msg(&self, state: &mut StoreState, src: Id, msg: Message, o: &mut Out<Participant>) {
        let rules = self.rules;
        match msg {
            Message::Reserve(inputs) => rules.reserve(self, state, src, inputs, o),
            Message::Renew(reservation) => rules.renew(self, state, reservation, o),
            Message::Release(reservation) => rules.release(self, state, src, reservation, o),
            Message::MarkPending {
                reservation,
                outputs,
            } => rules.mark_pending(self, state, src, reservation, outputs, o),
            Message::Settled {
                reservation,
                verdict,
            } => rules.settled(self, state, reservation, verdict, o),
            Message::Reserved(_)
            | Message::Refused
            | Message::Released(_)
            | Message::StillPending(_)
            | Message::MarkedPending(_)
            | Message::NotReserved(_)
            | Message::Ended(_)
            | Message::Watch { .. } => unreachable!("the store receives requests and settlements"),
        }
    }

    fn on_timeout(&self, state: &mut StoreState, timer: &Timer, o: &mut Out<Participant>) {
        match *timer {
            Timer::LeaseExpiry(reservation) => {
                self.rules.lease_expired(self, state, reservation, o)
            }
            Timer::Settle(..) | Timer::ProofReady | Timer::Renew | Timer::GiveUp => {
                unreachable!("the store sets lease expiry timers only")
            }
        }
    }
}

/// Stands in for the synchronizer, with no chain behind it. Asked to watch a
/// transaction's effects, it later settles the transaction with any verdict, once.
/// Whether the verdict is right, and when it is safe to trust, is outside this model.
#[derive(Clone)]
pub struct Synchronizer;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Watched {
    pub store: Id,
    pub inputs: Objects,
    pub outputs: Objects,
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SynchronizerState {
    /// Transactions watched and not yet settled.
    pub watched: BTreeMap<Reservation, Watched>,
}

/// Every verdict a transaction with these effects could receive.
fn verdicts(inputs: Objects, outputs: Objects) -> impl Iterator<Item = Verdict> {
    inputs.subsets().flat_map(move |consumed| {
        outputs
            .subsets()
            .map(move |created| Verdict { consumed, created })
    })
}

impl Synchronizer {
    fn on_msg(
        &self,
        state: &mut SynchronizerState,
        src: Id,
        msg: Message,
        o: &mut Out<Participant>,
    ) {
        let Message::Watch {
            reservation,
            inputs,
            outputs,
        } = msg
        else {
            unreachable!("the synchronizer receives watch requests only")
        };
        state.watched.insert(
            reservation,
            Watched {
                store: src,
                inputs,
                outputs,
            },
        );
        for verdict in verdicts(inputs, outputs) {
            o.set_timer(Timer::Settle(reservation, verdict), model_timeout());
        }
    }

    fn on_timeout(&self, state: &mut SynchronizerState, timer: &Timer, o: &mut Out<Participant>) {
        let Timer::Settle(reservation, verdict) = *timer else {
            unreachable!("the synchronizer sets settle timers only")
        };
        let watched = state
            .watched
            .remove(&reservation)
            .expect("a settle timer belongs to a watched transaction");
        for other in verdicts(watched.inputs, watched.outputs) {
            o.cancel_timer(Timer::Settle(reservation, other));
        }
        o.send(
            watched.store,
            Message::Settled {
                reservation,
                verdict,
            },
        );
    }
}

fn save_journal(journal: &Journal, o: &mut Out<Participant>) {
    o.save(ParticipantStorage::Orchestrator(journal.clone()));
}

/// Saves the reservation to mark the payload pending under, and asks the store.
fn start_marking_pending(
    orchestrator: &Orchestrator,
    state: &mut OrchestratorState,
    reservation: Reservation,
    o: &mut Out<Participant>,
) {
    state.journal.reservation = Some(reservation);
    save_journal(&state.journal, o);
    orchestrator.request_marking_pending(reservation, o);
    state.stage = Stage::MarkingPending(reservation);
}

fn cancel_timers(o: &mut Out<Participant>) {
    for timer in [Timer::ProofReady, Timer::Renew, Timer::GiveUp] {
        o.cancel_timer(timer);
    }
}

/// Records the outcome in the journal and stops.
pub fn finish(state: &mut OrchestratorState, outcome: Outcome, o: &mut Out<Participant>) {
    state.journal.outcome = Some(outcome);
    save_journal(&state.journal, o);
    cancel_timers(o);
    state.stage = Stage::Finished;
}

/// How an orchestrator responds to each reply and timer. The default methods are the
/// protocol's rules; a test can substitute an orchestrator that breaks one of them.
pub trait OrchestratorRules: Sync {
    /// Resumes from the journal: finished, asking again to mark the saved payload
    /// pending, or reserving.
    fn start(
        &self,
        orchestrator: &Orchestrator,
        journal: &Journal,
        o: &mut Out<Participant>,
    ) -> Stage {
        match (journal.outcome, journal.reservation) {
            (Some(_), _) => Stage::Finished,
            (None, Some(reservation)) => {
                orchestrator.start_lease_timers(o);
                orchestrator.request_marking_pending(reservation, o);
                Stage::MarkingPending(reservation)
            }
            (None, None) => {
                o.send(orchestrator.store, Message::Reserve(orchestrator.inputs));
                Stage::Reserving
            }
        }
    }

    fn reserved(
        &self,
        orchestrator: &Orchestrator,
        state: &mut OrchestratorState,
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        orchestrator.start_lease_timers(o);
        if state.journal.reservation.is_some() {
            start_marking_pending(orchestrator, state, reservation, o);
        } else {
            o.set_timer(Timer::ProofReady, model_timeout());
            state.stage = Stage::Proving(reservation);
        }
    }

    fn refused(&self, _: &Orchestrator, state: &mut OrchestratorState, o: &mut Out<Participant>) {
        finish(state, Outcome::Abandoned, o);
    }

    fn marked_pending(
        &self,
        _: &Orchestrator,
        state: &mut OrchestratorState,
        o: &mut Out<Participant>,
    ) {
        finish(state, Outcome::Submitted, o);
    }

    fn not_reserved(
        &self,
        orchestrator: &Orchestrator,
        state: &mut OrchestratorState,
        o: &mut Out<Participant>,
    ) {
        cancel_timers(o);
        o.send(orchestrator.store, Message::Reserve(orchestrator.inputs));
        state.stage = Stage::Reserving;
    }

    /// The transaction was settled before the orchestrator heard it was pending, so
    /// the payload is not submitted.
    fn ended(&self, _: &Orchestrator, state: &mut OrchestratorState, o: &mut Out<Participant>) {
        finish(state, Outcome::Abandoned, o);
    }

    fn proof_ready(
        &self,
        orchestrator: &Orchestrator,
        state: &mut OrchestratorState,
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        start_marking_pending(orchestrator, state, reservation, o);
    }

    fn renew(
        &self,
        orchestrator: &Orchestrator,
        _: &mut OrchestratorState,
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        o.send(orchestrator.store, Message::Renew(reservation));
    }

    fn give_up(
        &self,
        orchestrator: &Orchestrator,
        state: &mut OrchestratorState,
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        o.send(orchestrator.store, Message::Release(reservation));
        finish(state, Outcome::Abandoned, o);
    }
}

/// The orchestrator that follows the protocol.
pub struct ProtocolOrchestrator;

impl OrchestratorRules for ProtocolOrchestrator {}

/// Reserves its inputs, proves, saves the payload, and submits once the store
/// confirms the transaction is pending. It may give up before that confirmation.
#[derive(Clone)]
pub struct Orchestrator {
    pub store: Id,
    pub inputs: Objects,
    pub outputs: Objects,
    /// Whether the run explores renewing the lease and giving up. A bound on the
    /// exploration rather than a capability: a real orchestrator can always do both.
    pub cancellable: bool,
    pub rules: &'static dyn OrchestratorRules,
}

impl Orchestrator {
    fn start(&self, journal: Option<&Journal>, o: &mut Out<Participant>) -> OrchestratorState {
        let restarted = journal.is_some();
        let journal = journal.cloned().unwrap_or_default();
        if !restarted {
            // An empty journal lets the checks tell a restart from the first start.
            save_journal(&journal, o);
        }
        let stage = self.rules.start(self, &journal, o);
        OrchestratorState {
            journal,
            stage,
            restarted,
        }
    }

    /// Arms the timers that run while the orchestrator holds a lease, once for each
    /// lease it is granted or resumes after a restart.
    fn start_lease_timers(&self, o: &mut Out<Participant>) {
        if self.cancellable {
            for timer in [Timer::Renew, Timer::GiveUp] {
                o.set_timer(timer, model_timeout());
            }
        }
    }

    fn request_marking_pending(&self, reservation: Reservation, o: &mut Out<Participant>) {
        o.send(
            self.store,
            Message::MarkPending {
                reservation,
                outputs: self.outputs,
            },
        );
    }

    fn on_msg(&self, state: &mut OrchestratorState, msg: Message, o: &mut Out<Participant>) {
        let rules = self.rules;
        match (state.stage, msg) {
            (Stage::Reserving, Message::Reserved(reservation)) => {
                rules.reserved(self, state, reservation, o)
            }
            (Stage::Reserving, Message::Refused) => rules.refused(self, state, o),
            (Stage::MarkingPending(current), Message::MarkedPending(reservation))
                if reservation == current =>
            {
                rules.marked_pending(self, state, o)
            }
            (Stage::MarkingPending(current), Message::NotReserved(reservation))
                if reservation == current =>
            {
                rules.not_reserved(self, state, o)
            }
            (Stage::MarkingPending(current), Message::Ended(reservation))
                if reservation == current =>
            {
                rules.ended(self, state, o)
            }
            _ => {}
        }
    }

    fn on_timeout(&self, state: &mut OrchestratorState, timer: &Timer, o: &mut Out<Participant>) {
        let rules = self.rules;
        match (state.stage, timer) {
            (Stage::Proving(reservation), Timer::ProofReady) => {
                rules.proof_ready(self, state, reservation, o)
            }
            (Stage::Proving(reservation) | Stage::MarkingPending(reservation), Timer::Renew) => {
                rules.renew(self, state, reservation, o)
            }
            (Stage::Proving(reservation) | Stage::MarkingPending(reservation), Timer::GiveUp) => {
                rules.give_up(self, state, reservation, o)
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
pub enum Participant {
    Store(Store),
    Orchestrator(Orchestrator),
    Synchronizer(Synchronizer),
}

/// Keeps the borrowed state when a handler changed nothing, which the checker
/// treats as a step that did not happen.
fn update(state: &mut Cow<ParticipantState>, handle: impl FnOnce(&mut ParticipantState)) {
    let mut next = state.as_ref().clone();
    handle(&mut next);
    if next != **state {
        *state = Cow::Owned(next);
    }
}

impl Actor for Participant {
    type Msg = Message;
    type State = ParticipantState;
    type Timer = Timer;
    type Random = ();
    type Storage = ParticipantStorage;

    fn on_start(
        &self,
        _: Id,
        storage: &Option<ParticipantStorage>,
        o: &mut Out<Self>,
    ) -> ParticipantState {
        match self {
            Self::Store(store) => ParticipantState::Store(
                store.start(storage.as_ref().map(ParticipantStorage::store), o),
            ),
            Self::Orchestrator(orchestrator) => ParticipantState::Orchestrator(
                orchestrator.start(storage.as_ref().map(ParticipantStorage::journal), o),
            ),
            Self::Synchronizer(_) => ParticipantState::Synchronizer(SynchronizerState::default()),
        }
    }

    fn on_msg(
        &self,
        _: Id,
        state: &mut Cow<ParticipantState>,
        src: Id,
        msg: Message,
        o: &mut Out<Self>,
    ) {
        match self {
            Self::Store(store) => {
                update(state, |state| store.on_msg(state.store_mut(), src, msg, o))
            }
            Self::Orchestrator(orchestrator) => update(state, |state| {
                orchestrator.on_msg(state.orchestrator_mut(), msg, o)
            }),
            Self::Synchronizer(synchronizer) => update(state, |state| {
                synchronizer.on_msg(state.synchronizer_mut(), src, msg, o)
            }),
        }
    }

    fn on_timeout(
        &self,
        _: Id,
        state: &mut Cow<ParticipantState>,
        timer: &Timer,
        o: &mut Out<Self>,
    ) {
        match self {
            Self::Store(store) => {
                update(state, |state| store.on_timeout(state.store_mut(), timer, o))
            }
            Self::Orchestrator(orchestrator) => update(state, |state| {
                orchestrator.on_timeout(state.orchestrator_mut(), timer, o)
            }),
            Self::Synchronizer(synchronizer) => update(state, |state| {
                synchronizer.on_timeout(state.synchronizer_mut(), timer, o)
            }),
        }
    }

    fn name(&self) -> String {
        match self {
            Self::Store(_) => "store".into(),
            Self::Orchestrator(_) => "orchestrator".into(),
            Self::Synchronizer(_) => "synchronizer".into(),
        }
    }
}

/// Events recorded for checking only; no participant reads them. Acknowledgements
/// and settlements carry their position in one sequence, so the checks can order
/// them against each other.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct History {
    /// The orchestrator each reservation was granted to.
    pub granted_to: BTreeMap<Reservation, Id>,
    /// Reservations granted a second time.
    pub regranted: BTreeSet<Reservation>,
    /// When each reservation was first acknowledged.
    pub acknowledged: BTreeMap<Reservation, usize>,
    /// Reservations acknowledged after their transaction was settled.
    pub acknowledged_after_settlement: BTreeSet<Reservation>,
    /// When each reservation's transaction was settled, and how, as the store received it.
    pub settled: BTreeMap<Reservation, (usize, Verdict)>,
    /// Reservations whose release reached the store before any acknowledgement was sent.
    pub released: BTreeSet<Reservation>,
    pub events: usize,
}

/// Both recorders clone the history only in the arms that change it, since they run
/// for every message sent or delivered and most messages leave no record.
fn record_sent<C>(_: &C, history: &History, envelope: Envelope<&Message>) -> Option<History> {
    match *envelope.msg {
        Message::Reserved(reservation) => {
            let mut next = history.clone();
            if next.granted_to.insert(reservation, envelope.dst).is_some() {
                next.regranted.insert(reservation);
            }
            Some(next)
        }
        Message::MarkedPending(reservation) if history.settled.contains_key(&reservation) => {
            let mut next = history.clone();
            next.acknowledged_after_settlement.insert(reservation);
            Some(next)
        }
        Message::MarkedPending(reservation) if !history.acknowledged.contains_key(&reservation) => {
            let mut next = history.clone();
            next.acknowledged.insert(reservation, next.events);
            next.events += 1;
            Some(next)
        }
        _ => None,
    }
}

fn record_received<C>(_: &C, history: &History, envelope: Envelope<&Message>) -> Option<History> {
    match *envelope.msg {
        Message::Release(reservation) if !history.acknowledged.contains_key(&reservation) => {
            let mut next = history.clone();
            next.released.insert(reservation);
            Some(next)
        }
        Message::Settled {
            reservation,
            verdict,
        } if !history.settled.contains_key(&reservation) => {
            let mut next = history.clone();
            next.settled.insert(reservation, (next.events, verdict));
            next.events += 1;
            Some(next)
        }
        _ => None,
    }
}

/// A system of participants, with `C` holding whatever configuration the checks of a
/// particular setup need.
pub type HandshakeModel<C> = ActorModel<Participant, C, History>;
pub type HandshakeState = ActorModelState<Participant, History>;
pub type HandshakeAction = ActorModelAction<Message, Timer, ()>;

pub fn store_state(state: &HandshakeState, store: Id) -> &StoreState {
    state.actor_states[usize::from(store)].store()
}

pub fn orchestrator_state(state: &HandshakeState, orchestrator: Id) -> &OrchestratorState {
    state.actor_states[usize::from(orchestrator)].orchestrator()
}

/// The pending transactions the store has saved, as opposed to those in its memory.
pub fn saved_pending(
    state: &HandshakeState,
    store: Id,
) -> &BTreeMap<Reservation, PendingTransaction> {
    let storage = state.actor_storages[usize::from(store)]
        .as_ref()
        .expect("the store saves its storage when it starts");
    &storage.store().pending
}

pub fn saved_journal(state: &HandshakeState, orchestrator: Id) -> &Journal {
    state.actor_storages[usize::from(orchestrator)]
        .as_ref()
        .expect("an orchestrator saves its journal when it starts")
        .journal()
}

fn store_of<C>(model: &HandshakeModel<C>) -> Id {
    let index = model
        .actors
        .iter()
        .position(|actor| matches!(actor, Participant::Store(_)))
        .expect("the system has a store");
    Id::from(index)
}

pub fn orchestrators_of<C>(model: &HandshakeModel<C>) -> impl Iterator<Item = (Id, &Orchestrator)> {
    model
        .actors
        .iter()
        .enumerate()
        .filter_map(|(index, actor)| match actor {
            Participant::Orchestrator(orchestrator) => Some((Id::from(index), orchestrator)),
            _ => None,
        })
}

/// The actor's name, with an ordinal when several orchestrators share it.
fn participant_name<C>(model: &HandshakeModel<C>, id: Id) -> String {
    let name = model.actors[usize::from(id)].name();
    let orchestrators: Vec<Id> = orchestrators_of(model)
        .map(|(orchestrator, _)| orchestrator)
        .collect();
    match orchestrators
        .iter()
        .position(|&orchestrator| orchestrator == id)
    {
        Some(position) if orchestrators.len() > 1 => format!("{name} {}", position + 1),
        _ => name,
    }
}

/// A step of a run with its participants named, for traces.
pub fn describe_action<C>(model: &HandshakeModel<C>, action: &HandshakeAction) -> String {
    let name = |id: Id| participant_name(model, id);
    match action {
        ActorModelAction::Deliver { src, dst, msg } => {
            format!("{} -> {msg:?} -> {}", name(*src), name(*dst))
        }
        ActorModelAction::Timeout(actor, timer) => format!("{timer:?} at {}", name(*actor)),
        ActorModelAction::Crash(actor) => format!("{} crashes", name(*actor)),
        ActorModelAction::Recover(actor) => format!("{} restarts", name(*actor)),
        _ => format!("{action:?}"),
    }
}

fn orchestrator_of<C>(model: &HandshakeModel<C>, id: Id) -> &Orchestrator {
    match &model.actors[usize::from(id)] {
        Participant::Orchestrator(orchestrator) => orchestrator,
        _ => unreachable!("reservations are granted to orchestrators"),
    }
}

/// The store never grants the same reservation twice, so a reservation always names one grant to one orchestrator. The other
/// guarantees tell transactions apart by reservation and rely on this.
fn reservations_are_granted_once<C>(_: &HandshakeModel<C>, state: &HandshakeState) -> bool {
    state.history.regranted.is_empty()
}

/// No input is acknowledged to a transaction while an earlier acknowledged
/// transaction holding it is unsettled, nor after a settlement consumed it. Judged
/// from the acknowledgements sent and the settlements received rather than from the
/// store's own admission check.
fn pending_transactions_are_exclusive<C>(
    model: &HandshakeModel<C>,
    state: &HandshakeState,
) -> bool {
    let history = &state.history;
    let inputs_of = |reservation: &Reservation| {
        let owner = history
            .granted_to
            .get(reservation)
            .expect("an acknowledged reservation was granted");
        orchestrator_of(model, *owner).inputs
    };
    history
        .acknowledged
        .iter()
        .all(|(later, &acknowledged_at)| {
            let later_inputs = inputs_of(later);
            history
                .acknowledged
                .iter()
                .filter(|(_, at)| **at < acknowledged_at)
                .all(|(earlier, _)| {
                    let shared = later_inputs.intersection(inputs_of(earlier));
                    shared.is_empty()
                        || history
                            .settled
                            .get(earlier)
                            .is_some_and(|(settled_at, verdict)| {
                                *settled_at < acknowledged_at && !verdict.consumed.overlaps(shared)
                            })
                })
        })
}

/// An acknowledgement promises recovery data, so the store must already have saved
/// the pending transaction it acknowledges, and it keeps the record once settled.
fn acknowledgements_are_saved<C>(model: &HandshakeModel<C>, state: &HandshakeState) -> bool {
    let saved = saved_pending(state, store_of(model));
    state
        .history
        .acknowledged
        .keys()
        .all(|reservation| saved.contains_key(reservation))
}

/// The store saves each settlement as the synchronizer gave it, in the same update
/// that receives it, and keeps it across restarts.
fn settlements_are_saved<C>(model: &HandshakeModel<C>, state: &HandshakeState) -> bool {
    let saved = saved_pending(state, store_of(model));
    state
        .history
        .settled
        .iter()
        .all(|(reservation, (_, verdict))| {
            saved
                .get(reservation)
                .is_some_and(|transaction| transaction.verdict == Some(*verdict))
        })
}

/// A settled transaction is never acknowledged again, so an orchestrator that asks
/// after the settlement learns the transaction ended instead of submitting its
/// payload.
fn settled_transactions_are_not_acknowledged<C>(
    _: &HandshakeModel<C>,
    state: &HandshakeState,
) -> bool {
    state.history.acknowledged_after_settlement.is_empty()
}

/// What the store saved under a reservation is the transaction its holder asked for: the
/// inputs the reservation reserved and the outputs sent with the request to mark it
/// pending, judged against the orchestrator rather than the store's record of either.
fn saved_transactions_match_their_requests<C>(
    model: &HandshakeModel<C>,
    state: &HandshakeState,
) -> bool {
    saved_pending(state, store_of(model))
        .iter()
        .all(|(reservation, transaction)| {
            state
                .history
                .granted_to
                .get(reservation)
                .is_some_and(|holder| {
                    let orchestrator = orchestrator_of(model, *holder);
                    transaction.inputs == orchestrator.inputs
                        && transaction.outputs == orchestrator.outputs
                })
        })
}

/// A submitted transaction is saved under its reservation, as pending or as settled.
fn submissions_are_saved<C>(model: &HandshakeModel<C>, state: &HandshakeState) -> bool {
    let saved = saved_pending(state, store_of(model));
    orchestrators_of(model).all(|(orchestrator, _)| {
        let journal = saved_journal(state, orchestrator);
        journal.outcome != Some(Outcome::Submitted)
            || journal
                .reservation
                .is_some_and(|reservation| saved.contains_key(&reservation))
    })
}

/// A reservation whose release reached the store before it was acknowledged must never be
/// acknowledged: the orchestrator has abandoned that transaction.
fn released_reservations_stay_unacknowledged<C>(
    _: &HandshakeModel<C>,
    state: &HandshakeState,
) -> bool {
    let history = &state.history;
    history
        .released
        .iter()
        .all(|reservation| !history.acknowledged.contains_key(reservation))
}

fn an_orchestrator_submits<C>(model: &HandshakeModel<C>, state: &HandshakeState) -> bool {
    orchestrators_of(model).any(|(orchestrator, _)| {
        saved_journal(state, orchestrator).outcome == Some(Outcome::Submitted)
    })
}

fn a_transaction_lands<C>(model: &HandshakeModel<C>, state: &HandshakeState) -> bool {
    saved_pending(state, store_of(model))
        .values()
        .any(PendingTransaction::landed)
}

/// The participants exchange messages over a network that may delay or reorder them
/// without bound, and are checked against the handshake's guarantees. A lost message
/// is one never delivered, so loss adds nothing the guarantees can see. Every system
/// must also reach a submission and a landing, so a store that refused everything
/// would not pass.
pub fn system<C>(cfg: C, participants: impl IntoIterator<Item = Participant>) -> HandshakeModel<C> {
    ActorModel::new(cfg, History::default())
        .actors(participants)
        .init_network(Network::new_unordered_nonduplicating([]))
        .record_msg_out(record_sent)
        .record_msg_in(record_received)
        .property(
            Expectation::Always,
            "reservations are granted once",
            reservations_are_granted_once,
        )
        .property(
            Expectation::Always,
            "pending transactions are exclusive",
            pending_transactions_are_exclusive,
        )
        .property(
            Expectation::Always,
            "acknowledgements are saved",
            acknowledgements_are_saved,
        )
        .property(
            Expectation::Always,
            "settlements are saved",
            settlements_are_saved,
        )
        .property(
            Expectation::Always,
            "settled transactions are not acknowledged",
            settled_transactions_are_not_acknowledged,
        )
        .property(
            Expectation::Always,
            "saved transactions match their requests",
            saved_transactions_match_their_requests,
        )
        .property(
            Expectation::Always,
            "submissions are saved",
            submissions_are_saved,
        )
        .property(
            Expectation::Always,
            "released reservations stay unacknowledged",
            released_reservations_stay_unacknowledged,
        )
        .property(
            Expectation::Sometimes,
            "an orchestrator submits",
            an_orchestrator_submits,
        )
        .property(
            Expectation::Sometimes,
            "a transaction lands",
            a_transaction_lands,
        )
}
