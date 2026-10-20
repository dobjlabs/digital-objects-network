use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use stateright::{
    Expectation,
    actor::{Actor, ActorModel, ActorModelState, Envelope, Id, Network, Out, model_timeout},
};

use crate::objects::Objects;

/// The store restarts explored; each restart begins a new token epoch.
const MAX_EPOCH: u8 = 1;
/// The reservations the store may grant within one epoch.
const MAX_GRANTS_PER_EPOCH: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scenario {
    /// Two orchestrators compete for x, renewing their leases and possibly giving up;
    /// nobody crashes.
    Contention,
    /// One orchestrator; the store and the orchestrator each crash and restart at
    /// most once.
    Recovery,
    /// Two orchestrators compete for x while the store may restart once.
    StoreRestart,
}

/// Failure modes injected into the store or the orchestrators while the other
/// participants follow the protocol.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Fault {
    #[default]
    None,
    /// The store releases a prepared attempt's inputs when its former lease expires.
    ExpirePrepared,
    /// The store acknowledges a preparation before saving it.
    AcknowledgeBeforeCommit,
    /// The store accepts a preparation under an ended lease while another lease is active.
    AcceptStaleReservation,
    /// The store revives an ended lease when asked to renew it.
    RenewExpiredLease,
    /// The store honors a release of a prepared reservation.
    ReleasePrepared,
    /// The store's reservation check ignores prepared attempts.
    ReserveOverPreparedInputs,
    /// A store restart loses the saved preparations.
    ForgetPreparationOnRestart,
    /// The store answers a release but keeps the lease.
    IgnoreRelease,
    /// The orchestrator submits as soon as it requests preparation, without waiting
    /// for the acknowledgement.
    SubmitWithoutAcknowledgement,
}

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
/// may delay, reorder, or lose any message; an orchestrator repeats its outstanding
/// request after a restart.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Message {
    /// Reserve these inputs exclusively.
    Reserve(Objects),
    /// The requested inputs are reserved under this token, with a lease.
    Granted(Token),
    /// Some requested input is held by another reservation or preparation.
    Refused,
    /// Extend this token's lease.
    Renew(Token),
    /// End this token's lease.
    Release(Token),
    /// No lease remains under this token.
    Released(Token),
    /// The token is prepared, so its inputs stay protected.
    StillPrepared(Token),
    /// Durably record the saved attempt and its output contents under this token.
    /// Repeating the request returns the recorded result.
    Prepare { token: Token, outputs: Objects },
    /// The attempt under this token is durably prepared.
    Prepared(Token),
    /// The token holds neither an active lease nor a preparation.
    PrepareRejected(Token),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Timer {
    /// The store ends this token's lease.
    LeaseExpiry(Token),
    /// The store saves a preparation it has already acknowledged; only the
    /// acknowledge-before-commit fault sets it.
    Commit(Token),
    /// The orchestrator's proof is ready.
    ProofReady,
    /// The orchestrator renews its lease. With time abstracted, one renewal racing
    /// the lease expiry stands for periodic renewals.
    Renew,
    /// The action is cancelled before the store acknowledges it.
    GiveUp,
}

pub fn store() -> Id {
    Id::from(0)
}

/// The scenario's orchestrators. When there are two, the inputs they request overlap
/// in x.
pub fn orchestrators(scenario: Scenario) -> Vec<Id> {
    let count = if scenario == Scenario::Recovery { 1 } else { 2 };
    (1..=count).map(Id::from).collect()
}

pub fn inputs(orchestrator: Id) -> Objects {
    if orchestrator == Id::from(1) {
        Objects::X.union(Objects::Y)
    } else {
        Objects::X.union(Objects::Z)
    }
}

pub fn outputs(orchestrator: Id) -> Objects {
    if orchestrator == Id::from(1) {
        Objects::P
    } else {
        Objects::Q
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Lease {
    pub inputs: Objects,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Preparation {
    pub inputs: Objects,
    pub outputs: Objects,
}

/// What the store keeps across restarts.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct StoreStorage {
    pub epoch: u8,
    pub preparations: BTreeMap<Token, Preparation>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StoreState {
    pub saved: StoreStorage,
    /// Leases granted in this epoch; ended leases stay recorded as inactive.
    pub leases: BTreeMap<Token, Lease>,
    /// Preparations acknowledged but not yet saved; only the acknowledge-before-commit
    /// fault fills it.
    pub unsaved: BTreeMap<Token, Preparation>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Outcome {
    Submitted,
    Abandoned,
}

/// What an orchestrator keeps across restarts.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Journal {
    /// Set once the proof and payload are saved: the token to prepare them under,
    /// replaced if the orchestrator has to reserve again.
    pub prepare_under: Option<Token>,
    pub outcome: Option<Outcome>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Stage {
    Reserving,
    Proving(Token),
    Preparing(Token),
    Finished,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OrchestratorState {
    pub journal: Journal,
    pub stage: Stage,
    /// Started from a saved journal rather than for the first time.
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

/// Keeps a reservation's inputs in memory and its preparation in storage, handling
/// one message or timer at a time.
#[derive(Clone)]
pub struct Store {
    fault: Fault,
}

impl Store {
    fn start(&self, storage: Option<&StoreStorage>, o: &mut Out<Participant>) -> StoreState {
        let saved = match storage {
            None => StoreStorage::default(),
            Some(previous) => StoreStorage {
                epoch: previous.epoch + 1,
                preparations: if self.fault == Fault::ForgetPreparationOnRestart {
                    BTreeMap::new()
                } else {
                    previous.preparations.clone()
                },
            },
        };
        o.save(ParticipantStorage::Store(saved.clone()));
        StoreState {
            saved,
            leases: BTreeMap::new(),
            unsaved: BTreeMap::new(),
        }
    }

    fn holds(&self, state: &StoreState, inputs: Objects) -> bool {
        let leased = state
            .leases
            .values()
            .any(|lease| lease.active && lease.inputs.overlaps(inputs));
        let prepared = self.fault != Fault::ReserveOverPreparedInputs
            && state
                .saved
                .preparations
                .values()
                .any(|preparation| preparation.inputs.overlaps(inputs));
        leased || prepared
    }

    fn on_msg(&self, state: &mut StoreState, src: Id, msg: Message, o: &mut Out<Participant>) {
        match msg {
            Message::Reserve(inputs) => {
                if self.holds(state, inputs) {
                    o.send(src, Message::Refused);
                    return;
                }
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
                o.send(src, Message::Granted(token));
            }
            Message::Renew(token) => {
                let revivable = self.fault == Fault::RenewExpiredLease
                    && !state.saved.preparations.contains_key(&token);
                if let Some(lease) = state.leases.get_mut(&token)
                    && (lease.active || revivable)
                {
                    lease.active = true;
                    o.set_timer(Timer::LeaseExpiry(token), model_timeout());
                }
            }
            Message::Release(token) => {
                if state.saved.preparations.contains_key(&token) {
                    if self.fault == Fault::ReleasePrepared {
                        state.saved.preparations.remove(&token);
                        o.save(ParticipantStorage::Store(state.saved.clone()));
                        o.send(src, Message::Released(token));
                    } else {
                        o.send(src, Message::StillPrepared(token));
                    }
                    return;
                }
                if let Some(lease) = state.leases.get_mut(&token)
                    && self.fault != Fault::IgnoreRelease
                {
                    lease.active = false;
                    o.cancel_timer(Timer::LeaseExpiry(token));
                }
                o.send(src, Message::Released(token));
            }
            Message::Prepare { token, outputs } => {
                if state.saved.preparations.contains_key(&token)
                    || state.unsaved.contains_key(&token)
                {
                    o.send(src, Message::Prepared(token));
                    return;
                }
                let stale_accepted = self.fault == Fault::AcceptStaleReservation
                    && state.leases.values().any(|lease| lease.active);
                let Some(lease) = state
                    .leases
                    .get_mut(&token)
                    .filter(|lease| lease.active || stale_accepted)
                else {
                    o.send(src, Message::PrepareRejected(token));
                    return;
                };
                lease.active = false;
                let preparation = Preparation {
                    inputs: lease.inputs,
                    outputs,
                };
                if self.fault != Fault::ExpirePrepared {
                    o.cancel_timer(Timer::LeaseExpiry(token));
                }
                if self.fault == Fault::AcknowledgeBeforeCommit {
                    state.unsaved.insert(token, preparation);
                    o.set_timer(Timer::Commit(token), model_timeout());
                } else {
                    state.saved.preparations.insert(token, preparation);
                    o.save(ParticipantStorage::Store(state.saved.clone()));
                }
                o.send(src, Message::Prepared(token));
            }
            Message::Granted(_)
            | Message::Refused
            | Message::Released(_)
            | Message::StillPrepared(_)
            | Message::Prepared(_)
            | Message::PrepareRejected(_) => {}
        }
    }

    fn on_timeout(&self, state: &mut StoreState, timer: &Timer, o: &mut Out<Participant>) {
        match *timer {
            Timer::LeaseExpiry(token) => {
                if self.fault == Fault::ExpirePrepared
                    && state.saved.preparations.remove(&token).is_some()
                {
                    o.save(ParticipantStorage::Store(state.saved.clone()));
                }
                if let Some(lease) = state.leases.get_mut(&token) {
                    lease.active = false;
                }
            }
            Timer::Commit(token) => {
                if let Some(preparation) = state.unsaved.remove(&token) {
                    state.saved.preparations.insert(token, preparation);
                    o.save(ParticipantStorage::Store(state.saved.clone()));
                }
            }
            Timer::ProofReady | Timer::Renew | Timer::GiveUp => {}
        }
    }
}

/// Reserves its inputs, proves, saves the payload, and submits once the store
/// acknowledges the preparation. It may give up before that acknowledgement.
#[derive(Clone)]
pub struct Orchestrator {
    fault: Fault,
    /// Renews its lease and may give up.
    cancellable: bool,
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

    fn request_preparation(&self, id: Id, token: Token, o: &mut Out<Participant>) {
        o.send(
            store(),
            Message::Prepare {
                token,
                outputs: outputs(id),
            },
        );
        for timer in self.lease_timers() {
            o.set_timer(*timer, model_timeout());
        }
    }

    fn start(
        &self,
        id: Id,
        journal: Option<&Journal>,
        o: &mut Out<Participant>,
    ) -> OrchestratorState {
        let restarted = journal.is_some();
        let journal = journal.cloned().unwrap_or_default();
        if !restarted {
            // An empty journal lets a restart be told apart from the first start.
            o.save(ParticipantStorage::Orchestrator(journal.clone()));
        }
        let stage = match (journal.outcome, journal.prepare_under) {
            (Some(_), _) => Stage::Finished,
            (None, Some(token)) => {
                self.request_preparation(id, token, o);
                Stage::Preparing(token)
            }
            (None, None) => {
                o.send(store(), Message::Reserve(inputs(id)));
                Stage::Reserving
            }
        };
        OrchestratorState {
            journal,
            stage,
            restarted,
        }
    }

    fn on_msg(
        &self,
        id: Id,
        state: &mut OrchestratorState,
        msg: Message,
        o: &mut Out<Participant>,
    ) {
        match (state.stage, msg) {
            (Stage::Reserving, Message::Granted(token)) => {
                if state.journal.prepare_under.is_some() {
                    state.journal.prepare_under = Some(token);
                    save_journal(state, o);
                    self.request_preparation(id, token, o);
                    state.stage = Stage::Preparing(token);
                } else {
                    o.set_timer(Timer::ProofReady, model_timeout());
                    for timer in self.lease_timers() {
                        o.set_timer(*timer, model_timeout());
                    }
                    state.stage = Stage::Proving(token);
                }
            }
            (Stage::Reserving, Message::Refused) => finish(state, Outcome::Abandoned, o),
            (Stage::Preparing(current), Message::Prepared(token)) if token == current => {
                finish(state, Outcome::Submitted, o)
            }
            (Stage::Preparing(current), Message::PrepareRejected(token)) if token == current => {
                cancel_timers(o);
                o.send(store(), Message::Reserve(inputs(id)));
                state.stage = Stage::Reserving;
            }
            _ => {}
        }
    }

    fn on_timeout(
        &self,
        id: Id,
        state: &mut OrchestratorState,
        timer: &Timer,
        o: &mut Out<Participant>,
    ) {
        match (state.stage, timer) {
            (Stage::Proving(token), Timer::ProofReady) => {
                state.journal.prepare_under = Some(token);
                save_journal(state, o);
                self.request_preparation(id, token, o);
                state.stage = Stage::Preparing(token);
                if self.fault == Fault::SubmitWithoutAcknowledgement {
                    finish(state, Outcome::Submitted, o);
                }
            }
            (Stage::Proving(token) | Stage::Preparing(token), Timer::Renew) => {
                o.send(store(), Message::Renew(token));
            }
            (Stage::Proving(token) | Stage::Preparing(token), Timer::GiveUp) => {
                o.send(store(), Message::Release(token));
                finish(state, Outcome::Abandoned, o);
            }
            _ => {}
        }
    }
}

fn save_journal(state: &OrchestratorState, o: &mut Out<Participant>) {
    o.save(ParticipantStorage::Orchestrator(state.journal.clone()));
}

fn cancel_timers(o: &mut Out<Participant>) {
    for timer in [Timer::ProofReady, Timer::Renew, Timer::GiveUp] {
        o.cancel_timer(timer);
    }
}

fn finish(state: &mut OrchestratorState, outcome: Outcome, o: &mut Out<Participant>) {
    state.journal.outcome = Some(outcome);
    save_journal(state, o);
    cancel_timers(o);
    state.stage = Stage::Finished;
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
        id: Id,
        storage: &Option<ParticipantStorage>,
        o: &mut Out<Self>,
    ) -> ParticipantState {
        match self {
            Self::Store(store) => ParticipantState::Store(
                store.start(storage.as_ref().map(ParticipantStorage::store), o),
            ),
            Self::Orchestrator(orchestrator) => ParticipantState::Orchestrator(orchestrator.start(
                id,
                storage.as_ref().map(ParticipantStorage::journal),
                o,
            )),
        }
    }

    fn on_msg(
        &self,
        id: Id,
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
                orchestrator.on_msg(id, state.orchestrator_mut(), msg, o)
            }),
        }
    }

    fn on_timeout(
        &self,
        id: Id,
        state: &mut Cow<ParticipantState>,
        timer: &Timer,
        o: &mut Out<Self>,
    ) {
        match self {
            Self::Store(store) => {
                update(state, |state| store.on_timeout(state.store_mut(), timer, o))
            }
            Self::Orchestrator(orchestrator) => update(state, |state| {
                orchestrator.on_timeout(id, state.orchestrator_mut(), timer, o)
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
    pub acknowledged: BTreeSet<Token>,
    /// Tokens whose release reached the store before any acknowledgement was sent.
    pub released: BTreeSet<Token>,
}

fn record_sent(_: &Scenario, history: &History, envelope: Envelope<&Message>) -> Option<History> {
    let mut next = history.clone();
    match *envelope.msg {
        Message::Granted(token) => {
            next.granted_to.insert(token, envelope.dst);
        }
        Message::Prepared(token) => {
            next.acknowledged.insert(token);
        }
        _ => {}
    }
    (next != *history).then_some(next)
}

fn record_received(
    _: &Scenario,
    history: &History,
    envelope: Envelope<&Message>,
) -> Option<History> {
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

pub type HandshakeModel = ActorModel<Participant, Scenario, History>;
pub type HandshakeState = ActorModelState<Participant, History>;

pub fn store_state(state: &HandshakeState) -> &StoreState {
    match &*state.actor_states[usize::from(store())] {
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

/// The preparations the store has saved, as opposed to those in its memory.
pub fn saved_preparations(state: &HandshakeState) -> &BTreeMap<Token, Preparation> {
    let storage = state.actor_storages[usize::from(store())]
        .as_ref()
        .expect("the store saves its storage when it starts");
    &storage.store().preparations
}

pub fn saved_journal(state: &HandshakeState, orchestrator: Id) -> &Journal {
    state.actor_storages[usize::from(orchestrator)]
        .as_ref()
        .expect("an orchestrator saves its journal when it starts")
        .journal()
}

/// No input is acknowledged to two attempts, judged from the acknowledgements sent
/// rather than from the store's own admission check.
fn acknowledged_attempts_are_exclusive(_: &HandshakeModel, state: &HandshakeState) -> bool {
    let history = &state.history;
    let requested = |token: &Token| {
        inputs(
            *history
                .granted_to
                .get(token)
                .expect("an acknowledged token was granted"),
        )
    };
    history.acknowledged.iter().all(|token| {
        history
            .acknowledged
            .range(..*token)
            .all(|earlier| !requested(token).overlaps(requested(earlier)))
    })
}

/// An acknowledgement promises recovery data, so the store must already have saved
/// the preparation it acknowledges.
fn acknowledgements_are_saved(_: &HandshakeModel, state: &HandshakeState) -> bool {
    let saved = saved_preparations(state);
    state
        .history
        .acknowledged
        .iter()
        .all(|token| saved.contains_key(token))
}

/// A submitted payload has a saved preparation under the token it was prepared under.
fn submissions_are_prepared(model: &HandshakeModel, state: &HandshakeState) -> bool {
    let saved = saved_preparations(state);
    orchestrators(model.cfg).into_iter().all(|orchestrator| {
        let journal = saved_journal(state, orchestrator);
        journal.outcome != Some(Outcome::Submitted)
            || journal
                .prepare_under
                .is_some_and(|token| saved.contains_key(&token))
    })
}

/// A token whose release reached the store before it was acknowledged must never be
/// acknowledged: the orchestrator has abandoned that attempt.
fn released_tokens_stay_unacknowledged(_: &HandshakeModel, state: &HandshakeState) -> bool {
    state
        .history
        .released
        .is_disjoint(&state.history.acknowledged)
}

fn submitted(state: &HandshakeState, orchestrator: Id) -> bool {
    saved_journal(state, orchestrator).outcome == Some(Outcome::Submitted)
}

fn an_orchestrator_submits(model: &HandshakeModel, state: &HandshakeState) -> bool {
    orchestrators(model.cfg)
        .into_iter()
        .any(|orchestrator| submitted(state, orchestrator))
}

fn submission_under_a_token_from_after_a_store_restart(
    model: &HandshakeModel,
    state: &HandshakeState,
) -> bool {
    orchestrators(model.cfg).into_iter().any(|orchestrator| {
        submitted(state, orchestrator)
            && saved_journal(state, orchestrator)
                .prepare_under
                .is_some_and(|token| token.epoch > 0)
    })
}

fn submission_by_a_restarted_orchestrator(model: &HandshakeModel, state: &HandshakeState) -> bool {
    orchestrators(model.cfg).into_iter().any(|orchestrator| {
        submitted(state, orchestrator) && orchestrator_state(state, orchestrator).restarted
    })
}

/// Each store restart begins a new epoch, so the store restarts at most once. An
/// orchestrator crashes only in the recovery scenario, and at most once.
fn within_bounds(scenario: &Scenario, state: &HandshakeState) -> bool {
    let current = store_state(state);
    let store_crashed_again =
        state.crashed[usize::from(store())] && current.saved.epoch >= MAX_EPOCH;
    let orchestrator_crash_excluded = orchestrators(*scenario).into_iter().any(|orchestrator| {
        state.crashed[usize::from(orchestrator)]
            && (*scenario != Scenario::Recovery
                || orchestrator_state(state, orchestrator).restarted)
    });
    !store_crashed_again
        && !orchestrator_crash_excluded
        && current.leases.len() <= MAX_GRANTS_PER_EPOCH
}

/// The store and the scenario's orchestrators. The network may delay, reorder, or
/// lose messages.
pub fn model(scenario: Scenario, fault: Fault) -> HandshakeModel {
    let orchestrator = Orchestrator {
        fault,
        cancellable: scenario == Scenario::Contention,
    };
    let model = ActorModel::new(scenario, History::default())
        .actor(Participant::Store(Store { fault }))
        .actors(
            orchestrators(scenario)
                .into_iter()
                .map(|_| Participant::Orchestrator(orchestrator.clone())),
        )
        .init_network(Network::new_unordered_nonduplicating([]))
        .max_crashes(if scenario == Scenario::Contention {
            0
        } else {
            1
        })
        .record_msg_out(record_sent)
        .record_msg_in(record_received)
        .within_boundary(within_bounds)
        .property(
            Expectation::Always,
            "acknowledged attempts are exclusive",
            acknowledged_attempts_are_exclusive,
        )
        .property(
            Expectation::Always,
            "acknowledgements are saved",
            acknowledgements_are_saved,
        )
        .property(
            Expectation::Always,
            "submissions are prepared",
            submissions_are_prepared,
        )
        .property(
            Expectation::Always,
            "released tokens stay unacknowledged",
            released_tokens_stay_unacknowledged,
        )
        .property(
            Expectation::Sometimes,
            "an orchestrator submits",
            an_orchestrator_submits,
        );
    match scenario {
        Scenario::Contention => model,
        Scenario::Recovery => model
            .property(
                Expectation::Sometimes,
                "submission by a restarted orchestrator",
                submission_by_a_restarted_orchestrator,
            )
            .property(
                Expectation::Sometimes,
                "submission under a token from after a store restart",
                submission_under_a_token_from_after_a_store_restart,
            ),
        Scenario::StoreRestart => model.property(
            Expectation::Sometimes,
            "submission under a token from after a store restart",
            submission_under_a_token_from_after_a_store_restart,
        ),
    }
}
