use stateright::{Model, Property};

use crate::effects::Objects;

/// Failure modes in which the store violates the reservation protocol.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Fault {
    /// The store follows the protocol without an injected failure.
    #[default]
    None,
    /// The store incorrectly releases prepared inputs when their former lease expires.
    ExpirePrepared,
    /// The store acknowledges preparation before its data is durably committed.
    AcknowledgeBeforeCommit,
    /// The store accepts a stale token because another attempt still holds a reservation.
    AcceptStaleReservation,
    /// The store renews a lease whose deadline has already passed.
    RenewExpiredLease,
    /// The store honors an explicit release of a prepared reservation.
    ReleasePrepared,
    /// The store grants inputs that a prepared attempt holds once that attempt's lease expires.
    ReserveOverPreparedInputs,
    /// A store crash loses preparation records and output contents that should survive.
    ForgetPreparationOnRestart,
}

#[derive(Clone, Debug)]
pub struct ReservationModel {
    pub attempts: usize,
    pub max_ticks: u8,
    pub crashes: bool,
    pub fault: Fault,
}

impl ReservationModel {
    pub fn leases() -> Self {
        Self {
            attempts: 2,
            max_ticks: 3,
            crashes: false,
            fault: Fault::None,
        }
    }

    pub fn recovery() -> Self {
        Self {
            attempts: 1,
            max_ticks: 2,
            crashes: true,
            fault: Fault::None,
        }
    }

    fn inputs(attempt: usize) -> Objects {
        Objects::X.union(if attempt == 0 { Objects::Y } else { Objects::Z })
    }

    fn outputs(attempt: usize) -> Objects {
        if attempt == 0 { Objects::P } else { Objects::Q }
    }

    fn holds_inputs(&self, reservation: &Reservation, now: u8) -> bool {
        if reservation.prepared && self.fault != Fault::ReserveOverPreparedInputs {
            reservation.protected
        } else {
            reservation.lease_valid(now)
        }
    }

    fn can_prepare(&self, state: &ReservationState, attempt: usize) -> bool {
        if self.fault == Fault::AcceptStaleReservation {
            state
                .attempts
                .iter()
                .any(|owner| self.holds_inputs(&owner.reservation, state.now))
        } else {
            state.attempts[attempt].reservation.lease_valid(state.now)
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReservationState {
    pub now: u8,
    pub store_up: bool,
    pub store_crashed: bool,
    pub contents: Objects,
    pub attempts: Vec<Attempt>,
    pub unsafe_submission: bool,
}

/// One reservation attempt, grouped by the participant that holds each fact.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Attempt {
    pub reservation: Reservation,
    pub orchestrator: Orchestrator,
    /// The lease grant has been sent and can be delivered, possibly again.
    pub grant_sent: bool,
    /// A preparation acknowledgement has been sent and can be delivered, possibly again.
    pub ack_sent: bool,
    /// Set when an unprepared lease is released or expires, and never cleared, so a
    /// later change to the store's deadline cannot hide that the lease ended.
    pub lease_ended: bool,
}

impl Attempt {
    /// Whether the protocol entitles the attempt to its inputs, judged from its
    /// preparation and lease history rather than from the store's own check.
    fn entitled_to_inputs(&self) -> bool {
        self.reservation.prepared || (self.reservation.registered && !self.lease_ended)
    }
}

/// The store's record of one reservation token.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Reservation {
    pub registered: bool,
    pub deadline: Option<u8>,
    pub prepared: bool,
    pub protected: bool,
    /// A received preparation request that has not been committed; a crash loses it.
    pub pending_prepare: bool,
}

impl Reservation {
    fn lease_valid(&self, now: u8) -> bool {
        self.deadline.is_some_and(|deadline| deadline > now)
    }
}

/// What one orchestrator knows about its attempt.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Orchestrator {
    pub up: bool,
    pub crashed: bool,
    pub journal: bool,
    pub grant_received: bool,
    pub ack_received: bool,
    pub submitted: bool,
}

impl Default for Orchestrator {
    fn default() -> Self {
        Self {
            up: true,
            crashed: false,
            journal: false,
            grant_received: false,
            ack_received: false,
            submitted: false,
        }
    }
}

impl Orchestrator {
    fn has_grant(&self) -> bool {
        self.up && self.grant_received
    }

    /// Stops the orchestrator, losing the replies it received but not its journal.
    fn crash(&mut self) {
        self.up = false;
        self.crashed = true;
        self.grant_received = false;
        self.ack_received = false;
    }
}

/// Events in reservation, preparation, submission, and process recovery.
/// Each index identifies one reservation attempt and is never reused after expiry.
#[derive(Clone, Debug, PartialEq)]
pub enum ReservationStep {
    /// The store grants the orchestrator an exclusive lease on all requested inputs.
    Reserve(usize),
    /// The orchestrator receives the lease grant, which may have been delayed.
    ReceiveGrant(usize),
    /// The store extends a still-valid lease at the orchestrator's request.
    Renew(usize),
    /// The store releases an unprepared reservation at the orchestrator's request.
    Release(usize),
    /// The orchestrator durably saves its generated proof and submission payload.
    JournalProof(usize),
    /// The store receives a preparation request; it remains pending and is not durable.
    RequestPrepare(usize),
    /// The store checks the lease and atomically saves the preparation and output
    /// contents, protecting the inputs beyond lease expiry if the request is valid.
    CommitPrepare(usize),
    /// The store sends confirmation of durable preparation, possibly as a repeat reply.
    SendAck(usize),
    /// The orchestrator receives the store's confirmation and can authorize submission.
    ReceiveAck(usize),
    /// The orchestrator submits its saved payload using the confirmation it received.
    Submit(usize),
    /// Time passes on the store's lease clock; an unrenewed lease may expire.
    Tick,
    /// The store stops and loses pending requests; committed preparations survive.
    CrashStore,
    /// The store resumes serving requests using its retained records.
    RestartStore,
    /// The orchestrator stops and loses received replies; its saved payload survives.
    CrashActor(usize),
    /// The orchestrator resumes with its journal and can receive replies again.
    RestartActor(usize),
}

impl Model for ReservationModel {
    type State = ReservationState;
    type Action = ReservationStep;

    fn init_states(&self) -> Vec<Self::State> {
        vec![ReservationState {
            now: 0,
            store_up: true,
            store_crashed: false,
            contents: Objects::INITIAL,
            attempts: vec![Attempt::default(); self.attempts],
            unsafe_submission: false,
        }]
    }

    fn actions(&self, _: &Self::State, actions: &mut Vec<Self::Action>) {
        use ReservationStep::*;
        actions.extend([Tick, CrashStore, RestartStore]);
        for index in 0..self.attempts {
            actions.extend([
                Reserve(index),
                ReceiveGrant(index),
                Renew(index),
                Release(index),
                JournalProof(index),
                RequestPrepare(index),
                CommitPrepare(index),
                SendAck(index),
                ReceiveAck(index),
                Submit(index),
                CrashActor(index),
                RestartActor(index),
            ]);
        }
    }

    fn next_state(&self, previous: &Self::State, action: Self::Action) -> Option<Self::State> {
        use ReservationStep::*;
        let mut state = previous.clone();
        let now = state.now;
        match action {
            Reserve(index) if state.store_up && state.attempts[index].orchestrator.up => {
                if state.attempts[index].reservation.registered {
                    return None;
                }
                if state.attempts.iter().enumerate().any(|(other, attempt)| {
                    self.holds_inputs(&attempt.reservation, now)
                        && Self::inputs(other).overlaps(Self::inputs(index))
                }) {
                    return None;
                }
                let attempt = &mut state.attempts[index];
                attempt.reservation.registered = true;
                attempt.reservation.deadline = Some(now + 2);
                attempt.grant_sent = true;
            }
            ReceiveGrant(index)
                if state.attempts[index].orchestrator.up && state.attempts[index].grant_sent =>
            {
                state.attempts[index].orchestrator.grant_received = true;
            }
            Renew(index) if state.store_up && state.attempts[index].orchestrator.has_grant() => {
                let reservation = &mut state.attempts[index].reservation;
                let renewable = reservation.deadline.is_some_and(|deadline| {
                    deadline > now || self.fault == Fault::RenewExpiredLease
                });
                if !reservation.prepared && renewable {
                    reservation.deadline = Some(now + 2);
                }
            }
            Release(index) if state.store_up && state.attempts[index].orchestrator.has_grant() => {
                let attempt = &mut state.attempts[index];
                if !attempt.reservation.prepared {
                    attempt.reservation.deadline = None;
                    attempt.lease_ended = true;
                } else if self.fault == Fault::ReleasePrepared {
                    attempt.reservation.protected = false;
                }
            }
            JournalProof(index) if state.attempts[index].orchestrator.has_grant() => {
                state.attempts[index].orchestrator.journal = true;
            }
            RequestPrepare(index)
                if state.store_up
                    && state.attempts[index].orchestrator.up
                    && state.attempts[index].orchestrator.journal =>
            {
                state.attempts[index].reservation.pending_prepare = true;
            }
            CommitPrepare(index)
                if state.store_up && state.attempts[index].reservation.pending_prepare =>
            {
                let can_prepare = self.can_prepare(&state, index);
                let reservation = &mut state.attempts[index].reservation;
                reservation.pending_prepare = false;
                // A repeated request leaves the recorded preparation as it is.
                if !reservation.prepared && can_prepare {
                    reservation.prepared = true;
                    reservation.protected = true;
                    state.contents = state.contents.union(Self::outputs(index));
                }
            }
            SendAck(index)
                if state.store_up
                    && (state.attempts[index].reservation.prepared
                        || (self.fault == Fault::AcknowledgeBeforeCommit
                            && state.attempts[index].reservation.pending_prepare)) =>
            {
                state.attempts[index].ack_sent = true;
            }
            ReceiveAck(index)
                if state.attempts[index].orchestrator.up && state.attempts[index].ack_sent =>
            {
                state.attempts[index].orchestrator.ack_received = true;
            }
            Submit(index)
                if state.attempts[index].orchestrator.up
                    && state.attempts[index].orchestrator.ack_received =>
            {
                let attempt = &mut state.attempts[index];
                attempt.orchestrator.submitted = true;
                state.unsafe_submission |= !attempt.reservation.prepared
                    || !attempt.reservation.protected
                    || !attempt.orchestrator.journal
                    || !state.contents.contains(Self::outputs(index));
            }
            Tick if now < self.max_ticks => {
                state.now += 1;
                for attempt in &mut state.attempts {
                    let reservation = &mut attempt.reservation;
                    if reservation.deadline.is_none() || reservation.lease_valid(state.now) {
                        continue;
                    }
                    if !reservation.prepared {
                        attempt.lease_ended = true;
                    }
                    if self.fault == Fault::ExpirePrepared {
                        reservation.protected = false;
                    }
                }
            }
            CrashStore if self.crashes && state.store_up && !state.store_crashed => {
                state.store_up = false;
                state.store_crashed = true;
                for attempt in &mut state.attempts {
                    attempt.reservation.pending_prepare = false;
                    if self.fault == Fault::ForgetPreparationOnRestart {
                        attempt.reservation.prepared = false;
                        attempt.reservation.protected = false;
                    }
                }
                if self.fault == Fault::ForgetPreparationOnRestart {
                    state.contents = Objects::INITIAL;
                }
            }
            RestartStore if !state.store_up => state.store_up = true,
            CrashActor(index)
                if self.crashes
                    && state.attempts[index].orchestrator.up
                    && !state.attempts[index].orchestrator.crashed =>
            {
                state.attempts[index].orchestrator.crash();
            }
            RestartActor(index) if !state.attempts[index].orchestrator.up => {
                state.attempts[index].orchestrator.up = true;
            }
            _ => return None,
        }
        (state != *previous).then_some(state)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::<Self>::always("reservations are exclusive", |_, state| {
                let attempts = &state.attempts;
                (0..attempts.len()).all(|index| {
                    (0..index).all(|other| {
                        !(attempts[index].entitled_to_inputs()
                            && attempts[other].entitled_to_inputs()
                            && Self::inputs(index).overlaps(Self::inputs(other)))
                    })
                })
            }),
            Property::<Self>::always("acknowledgements have durable preparations", |_, state| {
                state.attempts.iter().enumerate().all(|(index, attempt)| {
                    !attempt.ack_sent
                        || (attempt.reservation.prepared
                            && state.contents.contains(Self::outputs(index)))
                })
            }),
            Property::<Self>::always("prepared inputs remain protected", |_, state| {
                state
                    .attempts
                    .iter()
                    .all(|attempt| !attempt.reservation.prepared || attempt.reservation.protected)
            }),
            Property::<Self>::always("stale reservations cannot prepare", |_, state| {
                state
                    .attempts
                    .iter()
                    .all(|attempt| !(attempt.reservation.prepared && attempt.lease_ended))
            }),
            Property::<Self>::always("submission has durable recovery data", |_, state| {
                !state.unsafe_submission
            }),
            Property::<Self>::sometimes("submission is reachable", |_, state| {
                state.attempts[0].orchestrator.submitted
            }),
            Property::<Self>::sometimes("prepared reservation outlives lease", |_, state| {
                let reservation = &state.attempts[0].reservation;
                reservation.prepared
                    && reservation
                        .deadline
                        .is_some_and(|deadline| deadline <= state.now)
            }),
        ]
        .into_iter()
        .chain(self.crashes.then(|| {
            Property::<Self>::sometimes("submission after both processes recover", |_, state| {
                let orchestrator = &state.attempts[0].orchestrator;
                state.store_crashed
                    && state.store_up
                    && orchestrator.crashed
                    && orchestrator.up
                    && orchestrator.submitted
            })
        }))
        .collect()
    }
}
