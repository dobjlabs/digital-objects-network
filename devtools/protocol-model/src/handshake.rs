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

/// Identifies one reservation. A real store issues random tokens; here they stay
/// unique because each store restart begins a new epoch.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Token {
    pub epoch: u8,
    pub sequence: u8,
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "t{}.{}", self.epoch, self.sequence)
    }
}

/// Requests from an orchestrator to the store, and the store's replies. The network
/// may delay or reorder any message without bound; an orchestrator repeats its
/// outstanding request after a restart.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Message {
    /// Reserve these inputs exclusively.
    Reserve(Objects),
    /// The requested inputs are reserved under this token, with a lease.
    Reserved(Token),
    /// Some requested input is held by another reservation or pending transaction.
    Refused,
    /// Extend this token's lease.
    Renew(Token),
    /// End this token's lease.
    Release(Token),
    /// No lease remains under this token.
    Released(Token),
    /// The token's transaction is pending, so its inputs stay protected.
    StillPending(Token),
    /// Save the transaction and the contents of the objects it creates as pending
    /// under this token. Repeating the request returns the recorded result.
    MarkPending { token: Token, outputs: Objects },
    /// The transaction under this token is saved as pending.
    MarkedPending(Token),
    /// The token holds neither an active lease nor a pending transaction.
    NotReserved(Token),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Timer {
    /// The store ends this token's lease.
    LeaseExpiry(Token),
    /// The orchestrator's proof is ready.
    ProofReady,
    /// The orchestrator renews its lease. With time abstracted, one renewal racing
    /// the lease expiry stands for periodic renewals.
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
}

/// What the store keeps across restarts.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct StoreStorage {
    pub epoch: u8,
    pub pending: BTreeMap<Token, PendingTransaction>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StoreState {
    pub saved: StoreStorage,
    /// Leases granted in this epoch; ended leases stay recorded as inactive.
    pub leases: BTreeMap<Token, Lease>,
}

impl StoreState {
    pub fn leases_cover(&self, inputs: Objects) -> bool {
        self.leases
            .values()
            .any(|lease| lease.active && lease.inputs.overlaps(inputs))
    }

    fn pending_covers(&self, inputs: Objects) -> bool {
        self.saved
            .pending
            .values()
            .any(|transaction| transaction.inputs.overlaps(inputs))
    }

    pub fn lease_active(&self, token: Token) -> bool {
        self.leases.get(&token).is_some_and(|lease| lease.active)
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
    pub reservation: Option<Token>,
    pub outcome: Option<Outcome>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Stage {
    Reserving,
    Proving(Token),
    MarkingPending(Token),
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
}

impl ParticipantState {
    fn store_mut(&mut self) -> &mut StoreState {
        match self {
            Self::Store(state) => state,
            Self::Orchestrator(_) => unreachable!("the store actor holds store state"),
        }
    }

    fn orchestrator_mut(&mut self) -> &mut OrchestratorState {
        match self {
            Self::Orchestrator(state) => state,
            Self::Store(_) => unreachable!("an orchestrator actor holds orchestrator state"),
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

/// Issues a fresh token with a lease on the inputs.
pub fn grant(state: &mut StoreState, to: Id, inputs: Objects, o: &mut Out<Participant>) {
    let token = Token {
        epoch: state.saved.epoch,
        sequence: state.leases.len() as u8,
    };
    state.leases.insert(
        token,
        Lease {
            inputs,
            active: true,
        },
    );
    o.set_timer(Timer::LeaseExpiry(token), model_timeout());
    o.send(to, Message::Reserved(token));
}

pub fn end_lease(state: &mut StoreState, token: Token, o: &mut Out<Participant>) {
    if let Some(lease) = state.leases.get_mut(&token) {
        lease.active = false;
        o.cancel_timer(Timer::LeaseExpiry(token));
    }
}

pub fn save(state: &StoreState, o: &mut Out<Participant>) {
    o.save(ParticipantStorage::Store(state.saved.clone()));
}

/// Ends the token's lease, saves its transaction as pending, and acknowledges.
pub fn record_pending(
    state: &mut StoreState,
    from: Id,
    token: Token,
    outputs: Objects,
    o: &mut Out<Participant>,
) {
    let transaction = PendingTransaction {
        inputs: state.leases[&token].inputs,
        outputs,
    };
    end_lease(state, token, o);
    state.saved.pending.insert(token, transaction);
    save(state, o);
    o.send(from, Message::MarkedPending(token));
}

/// How the store handles each request and timer. The default methods are the
/// protocol's rules; a test can substitute a store that breaks one of them.
pub trait StoreRules: Sync {
    /// The storage the store resumes from. Each restart begins a new epoch, so tokens
    /// issued before it are never issued again.
    fn restart(&self, previous: Option<&StoreStorage>) -> StoreStorage {
        previous.map_or_else(StoreStorage::default, |previous| StoreStorage {
            epoch: previous.epoch + 1,
            pending: previous.pending.clone(),
        })
    }

    fn reserve(&self, state: &mut StoreState, from: Id, inputs: Objects, o: &mut Out<Participant>) {
        if state.leases_cover(inputs) || state.pending_covers(inputs) {
            o.send(from, Message::Refused);
        } else {
            grant(state, from, inputs, o);
        }
    }

    fn renew(&self, state: &mut StoreState, token: Token, o: &mut Out<Participant>) {
        if state.lease_active(token) {
            o.set_timer(Timer::LeaseExpiry(token), model_timeout());
        }
    }

    fn release(&self, state: &mut StoreState, from: Id, token: Token, o: &mut Out<Participant>) {
        if state.saved.pending.contains_key(&token) {
            o.send(from, Message::StillPending(token));
        } else {
            end_lease(state, token, o);
            o.send(from, Message::Released(token));
        }
    }

    fn mark_pending(
        &self,
        state: &mut StoreState,
        from: Id,
        token: Token,
        outputs: Objects,
        o: &mut Out<Participant>,
    ) {
        if state.saved.pending.contains_key(&token) {
            o.send(from, Message::MarkedPending(token));
        } else if state.lease_active(token) {
            record_pending(state, from, token, outputs, o);
        } else {
            o.send(from, Message::NotReserved(token));
        }
    }

    fn lease_expired(&self, state: &mut StoreState, token: Token, _: &mut Out<Participant>) {
        if let Some(lease) = state.leases.get_mut(&token) {
            lease.active = false;
        }
    }
}

/// The store that follows the protocol.
pub struct ProtocolStore;

impl StoreRules for ProtocolStore {}

/// Keeps reservations in memory and pending transactions in storage, applying one
/// update at a time: a request, or a lease expiring.
#[derive(Clone)]
pub struct Store {
    pub rules: &'static dyn StoreRules,
}

impl Store {
    fn start(&self, storage: Option<&StoreStorage>, o: &mut Out<Participant>) -> StoreState {
        let saved = self.rules.restart(storage);
        o.save(ParticipantStorage::Store(saved.clone()));
        StoreState {
            saved,
            leases: BTreeMap::new(),
        }
    }

    fn on_msg(&self, state: &mut StoreState, src: Id, msg: Message, o: &mut Out<Participant>) {
        match msg {
            Message::Reserve(inputs) => self.rules.reserve(state, src, inputs, o),
            Message::Renew(token) => self.rules.renew(state, token, o),
            Message::Release(token) => self.rules.release(state, src, token, o),
            Message::MarkPending { token, outputs } => {
                self.rules.mark_pending(state, src, token, outputs, o)
            }
            Message::Reserved(_)
            | Message::Refused
            | Message::Released(_)
            | Message::StillPending(_)
            | Message::MarkedPending(_)
            | Message::NotReserved(_) => {}
        }
    }

    fn on_timeout(&self, state: &mut StoreState, timer: &Timer, o: &mut Out<Participant>) {
        match *timer {
            Timer::LeaseExpiry(token) => self.rules.lease_expired(state, token, o),
            Timer::ProofReady | Timer::Renew | Timer::GiveUp => {}
        }
    }
}

fn save_journal(state: &OrchestratorState, o: &mut Out<Participant>) {
    o.save(ParticipantStorage::Orchestrator(state.journal.clone()));
}

/// Saves the reservation to mark the payload pending under, and asks the store.
fn start_marking_pending(
    orchestrator: &Orchestrator,
    state: &mut OrchestratorState,
    token: Token,
    o: &mut Out<Participant>,
) {
    state.journal.reservation = Some(token);
    save_journal(state, o);
    orchestrator.request_marking_pending(token, o);
    state.stage = Stage::MarkingPending(token);
}

fn cancel_timers(o: &mut Out<Participant>) {
    for timer in [Timer::ProofReady, Timer::Renew, Timer::GiveUp] {
        o.cancel_timer(timer);
    }
}

/// Records the outcome in the journal and stops.
pub fn finish(state: &mut OrchestratorState, outcome: Outcome, o: &mut Out<Participant>) {
    state.journal.outcome = Some(outcome);
    save_journal(state, o);
    cancel_timers(o);
    state.stage = Stage::Finished;
}

/// How an orchestrator responds to each reply and timer. The default methods are the
/// protocol's rules; a test can substitute an orchestrator that breaks one of them.
pub trait OrchestratorRules: Sync {
    fn start(
        &self,
        orchestrator: &Orchestrator,
        journal: Option<&Journal>,
        o: &mut Out<Participant>,
    ) -> OrchestratorState {
        let restarted = journal.is_some();
        let journal = journal.cloned().unwrap_or_default();
        if !restarted {
            // An empty journal lets the checks tell a restart from the first start.
            o.save(ParticipantStorage::Orchestrator(journal.clone()));
        }
        let stage = match (journal.outcome, journal.reservation) {
            (Some(_), _) => Stage::Finished,
            (None, Some(token)) => {
                orchestrator.request_marking_pending(token, o);
                Stage::MarkingPending(token)
            }
            (None, None) => {
                o.send(orchestrator.store, Message::Reserve(orchestrator.inputs));
                Stage::Reserving
            }
        };
        OrchestratorState {
            journal,
            stage,
            restarted,
        }
    }

    fn reserved(
        &self,
        orchestrator: &Orchestrator,
        state: &mut OrchestratorState,
        token: Token,
        o: &mut Out<Participant>,
    ) {
        if state.journal.reservation.is_some() {
            start_marking_pending(orchestrator, state, token, o);
        } else {
            o.set_timer(Timer::ProofReady, model_timeout());
            for timer in orchestrator.lease_timers() {
                o.set_timer(*timer, model_timeout());
            }
            state.stage = Stage::Proving(token);
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

    fn proof_ready(
        &self,
        orchestrator: &Orchestrator,
        state: &mut OrchestratorState,
        token: Token,
        o: &mut Out<Participant>,
    ) {
        start_marking_pending(orchestrator, state, token, o);
    }

    fn renew(
        &self,
        orchestrator: &Orchestrator,
        _: &mut OrchestratorState,
        token: Token,
        o: &mut Out<Participant>,
    ) {
        o.send(orchestrator.store, Message::Renew(token));
    }

    fn give_up(
        &self,
        orchestrator: &Orchestrator,
        state: &mut OrchestratorState,
        token: Token,
        o: &mut Out<Participant>,
    ) {
        o.send(orchestrator.store, Message::Release(token));
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
    /// Renews its lease and may give up.
    pub cancellable: bool,
    pub rules: &'static dyn OrchestratorRules,
}

impl Orchestrator {
    /// The timers that run while the orchestrator holds a lease.
    fn lease_timers(&self) -> &'static [Timer] {
        if self.cancellable {
            &[Timer::Renew, Timer::GiveUp]
        } else {
            &[]
        }
    }

    fn request_marking_pending(&self, token: Token, o: &mut Out<Participant>) {
        o.send(
            self.store,
            Message::MarkPending {
                token,
                outputs: self.outputs,
            },
        );
        for timer in self.lease_timers() {
            o.set_timer(*timer, model_timeout());
        }
    }

    fn on_msg(&self, state: &mut OrchestratorState, msg: Message, o: &mut Out<Participant>) {
        let rules = self.rules;
        match (state.stage, msg) {
            (Stage::Reserving, Message::Reserved(token)) => rules.reserved(self, state, token, o),
            (Stage::Reserving, Message::Refused) => rules.refused(self, state, o),
            (Stage::MarkingPending(current), Message::MarkedPending(token)) if token == current => {
                rules.marked_pending(self, state, o)
            }
            (Stage::MarkingPending(current), Message::NotReserved(token)) if token == current => {
                rules.not_reserved(self, state, o)
            }
            _ => {}
        }
    }

    fn on_timeout(&self, state: &mut OrchestratorState, timer: &Timer, o: &mut Out<Participant>) {
        let rules = self.rules;
        match (state.stage, timer) {
            (Stage::Proving(token), Timer::ProofReady) => rules.proof_ready(self, state, token, o),
            (Stage::Proving(token) | Stage::MarkingPending(token), Timer::Renew) => {
                rules.renew(self, state, token, o)
            }
            (Stage::Proving(token) | Stage::MarkingPending(token), Timer::GiveUp) => {
                rules.give_up(self, state, token, o)
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
pub enum Participant {
    Store(Store),
    Orchestrator(Orchestrator),
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
            Self::Orchestrator(orchestrator) => {
                ParticipantState::Orchestrator(orchestrator.rules.start(
                    orchestrator,
                    storage.as_ref().map(ParticipantStorage::journal),
                    o,
                ))
            }
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
        }
    }

    fn name(&self) -> String {
        match self {
            Self::Store(_) => "store".into(),
            Self::Orchestrator(_) => "orchestrator".into(),
        }
    }
}

/// Events recorded for checking only; no participant reads them.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct History {
    /// The orchestrator each token was granted to.
    pub granted_to: BTreeMap<Token, Id>,
    /// Tokens granted a second time.
    pub reissued: BTreeSet<Token>,
    pub acknowledged: BTreeSet<Token>,
    /// Tokens whose release reached the store before any acknowledgement was sent.
    pub released: BTreeSet<Token>,
}

fn record_sent<C>(_: &C, history: &History, envelope: Envelope<&Message>) -> Option<History> {
    let mut next = history.clone();
    match *envelope.msg {
        Message::Reserved(token) => {
            if next.granted_to.insert(token, envelope.dst).is_some() {
                next.reissued.insert(token);
            }
        }
        Message::MarkedPending(token) => {
            next.acknowledged.insert(token);
        }
        _ => {}
    }
    (next != *history).then_some(next)
}

fn record_received<C>(_: &C, history: &History, envelope: Envelope<&Message>) -> Option<History> {
    let Message::Release(token) = *envelope.msg else {
        return None;
    };
    if history.acknowledged.contains(&token) || history.released.contains(&token) {
        return None;
    }
    let mut next = history.clone();
    next.released.insert(token);
    Some(next)
}

/// A system of participants, with `C` holding whatever configuration the checks of a
/// particular setup need.
pub type HandshakeModel<C> = ActorModel<Participant, C, History>;
pub type HandshakeState = ActorModelState<Participant, History>;
pub type HandshakeAction = ActorModelAction<Message, Timer, ()>;

pub fn store_state(state: &HandshakeState, store: Id) -> &StoreState {
    match &*state.actor_states[usize::from(store)] {
        ParticipantState::Store(store) => store,
        ParticipantState::Orchestrator(_) => unreachable!("the store actor holds store state"),
    }
}

pub fn orchestrator_state(state: &HandshakeState, orchestrator: Id) -> &OrchestratorState {
    match &*state.actor_states[usize::from(orchestrator)] {
        ParticipantState::Orchestrator(orchestrator) => orchestrator,
        ParticipantState::Store(_) => {
            unreachable!("an orchestrator actor holds orchestrator state")
        }
    }
}

/// The pending transactions the store has saved, as opposed to those in its memory.
pub fn saved_pending(state: &HandshakeState, store: Id) -> &BTreeMap<Token, PendingTransaction> {
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
            Participant::Store(_) => None,
        })
}

/// The actor's name, with an ordinal when several orchestrators share it.
fn participant_name<C>(model: &HandshakeModel<C>, id: Id) -> String {
    let name = model.actors[usize::from(id)].name();
    let mut orchestrators = orchestrators_of(model).map(|(orchestrator, _)| orchestrator);
    match orchestrators.position(|orchestrator| orchestrator == id) {
        Some(position) if position > 0 || orchestrators.next().is_some() => {
            format!("{name} {}", position + 1)
        }
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
        Participant::Store(_) => unreachable!("tokens are granted to orchestrators"),
    }
}

/// A token names one reservation, so the store never issues a token twice. The other
/// guarantees tell transactions apart by token and rely on this.
fn tokens_are_issued_once<C>(_: &HandshakeModel<C>, state: &HandshakeState) -> bool {
    state.history.reissued.is_empty()
}

/// No input is acknowledged to two transactions, judged from the acknowledgements sent
/// rather than from the store's own admission check.
fn acknowledged_transactions_are_exclusive<C>(
    model: &HandshakeModel<C>,
    state: &HandshakeState,
) -> bool {
    let history = &state.history;
    let requested = |token: &Token| {
        let owner = history
            .granted_to
            .get(token)
            .expect("an acknowledged token was granted");
        orchestrator_of(model, *owner).inputs
    };
    history.acknowledged.iter().all(|token| {
        history
            .acknowledged
            .range(..*token)
            .all(|earlier| !requested(token).overlaps(requested(earlier)))
    })
}

/// An acknowledgement promises recovery data, so the store must already have saved
/// the pending transaction it acknowledges.
fn acknowledgements_are_saved<C>(model: &HandshakeModel<C>, state: &HandshakeState) -> bool {
    let saved = saved_pending(state, store_of(model));
    state
        .history
        .acknowledged
        .iter()
        .all(|token| saved.contains_key(token))
}

/// What the store saved under a token is the transaction its holder asked for: the
/// inputs the token reserved and the outputs sent with the request to mark it
/// pending, judged against the orchestrator rather than the store's record of either.
fn saved_transactions_match_their_requests<C>(
    model: &HandshakeModel<C>,
    state: &HandshakeState,
) -> bool {
    saved_pending(state, store_of(model))
        .iter()
        .all(|(token, transaction)| {
            state.history.granted_to.get(token).is_some_and(|holder| {
                let orchestrator = orchestrator_of(model, *holder);
                transaction.inputs == orchestrator.inputs
                    && transaction.outputs == orchestrator.outputs
            })
        })
}

/// A submitted transaction is saved as pending under its reservation.
fn submissions_are_saved_as_pending<C>(model: &HandshakeModel<C>, state: &HandshakeState) -> bool {
    let saved = saved_pending(state, store_of(model));
    orchestrators_of(model).all(|(orchestrator, _)| {
        let journal = saved_journal(state, orchestrator);
        journal.outcome != Some(Outcome::Submitted)
            || journal
                .reservation
                .is_some_and(|token| saved.contains_key(&token))
    })
}

/// A token whose release reached the store before it was acknowledged must never be
/// acknowledged: the orchestrator has abandoned that transaction.
fn released_tokens_stay_unacknowledged<C>(_: &HandshakeModel<C>, state: &HandshakeState) -> bool {
    state
        .history
        .released
        .is_disjoint(&state.history.acknowledged)
}

/// The participants exchange messages over a network that may delay or reorder them
/// without bound, and are checked against the handshake's guarantees. A lost message
/// is one never delivered, so loss adds nothing the guarantees can see.
pub fn system<C>(cfg: C, participants: impl IntoIterator<Item = Participant>) -> HandshakeModel<C> {
    ActorModel::new(cfg, History::default())
        .actors(participants)
        .init_network(Network::new_unordered_nonduplicating([]))
        .record_msg_out(record_sent::<C>)
        .record_msg_in(record_received::<C>)
        .property(
            Expectation::Always,
            "tokens are issued once",
            tokens_are_issued_once::<C>,
        )
        .property(
            Expectation::Always,
            "acknowledged transactions are exclusive",
            acknowledged_transactions_are_exclusive::<C>,
        )
        .property(
            Expectation::Always,
            "acknowledgements are saved",
            acknowledgements_are_saved::<C>,
        )
        .property(
            Expectation::Always,
            "saved transactions match their requests",
            saved_transactions_match_their_requests::<C>,
        )
        .property(
            Expectation::Always,
            "submissions are saved as pending",
            submissions_are_saved_as_pending::<C>,
        )
        .property(
            Expectation::Always,
            "released tokens stay unacknowledged",
            released_tokens_stay_unacknowledged::<C>,
        )
}
