module publication {
  type SlotKind = Execution | WithoutExecution
  type Phase = Idle | Flushed | SlotInserted | IndexInserted
  type ReaderPhase = NoRead | SnapshotCaptured | ReadCompleted
  type Snapshot = { progress: int, stateSlot: int, created: List[int] }
  type State = { phase: Phase, committed: List[SlotKind], candidate: SlotKind, durable: bool, published: Snapshot,
    readerPhase: ReaderPhase, captured: Snapshot, result: Snapshot }
  type Command = Init({ genesis: bool }) | Start({ kind: SlotKind }) | Advance | Crash | Rollback
    | BeginRead | FinishRead | CloseRead
  pure val emptySnapshot: Snapshot = { progress: -1, stateSlot: -1, created: [] }
  pure def snapshot(slots: List[SlotKind]): Snapshot = {
    progress: slots.length() - 1,
    stateSlot: slots.indices().fold(-1, (latest, slot) =>
      if (slots[slot] == Execution and slot > latest) slot else latest),
    created: range(1, slots.length()).foldl([], (objects, slot) => if (slots[slot] == Execution) objects.append(slot + 10) else objects),
  }
  var state: State
  var command: Command

  action initWithGenesis(genesis: bool): bool = all {
    command' = Init({ genesis: genesis }),
    state' = { phase: Idle, committed: if (genesis) [Execution] else [], candidate: Execution,
      durable: false, published: snapshot(if (genesis) [Execution] else []), readerPhase: NoRead, captured: emptySnapshot, result: emptySnapshot },
  }
  action init = {
    nondet genesis = Set(false, true).oneOf()
    initWithGenesis(genesis)
  }
  action start(kind: SlotKind): bool = all {
    state.phase == Idle, state.committed.length() < 4,
    state.committed.length() > 0 or kind == Execution,
    command' = Start({ kind: kind }),
    state' = { ...state, phase: if (kind == Execution) Flushed else SlotInserted, candidate: kind, durable: kind == Execution },
  }
  action advance = all {
    state.phase != Idle,
    command' = Advance,
    state' = { ...state,
      phase: match state.phase {
        | Idle => Idle
        | Flushed => SlotInserted
        | SlotInserted => IndexInserted
        | IndexInserted => Idle
      },
      candidate: if (state.phase == IndexInserted) Execution else state.candidate,
      committed: if (state.phase == IndexInserted) state.committed.append(state.candidate) else state.committed,
      published: if (state.phase == IndexInserted) snapshot(state.committed.append(state.candidate)) else state.published,
    },
  }
  action crash = all {
    command' = Crash,
    state' = { ...state, phase: Idle, candidate: Execution, durable: false,
      readerPhase: NoRead, captured: emptySnapshot, result: emptySnapshot },
  }
  action rollback = all {
    state.phase == Idle, state.committed.length() > 1,
    command' = Rollback,
    state' = { ...state, committed: state.committed.slice(0, state.committed.length() - 1),
      published: snapshot(state.committed.slice(0, state.committed.length() - 1)) },
  }
  action beginRead = all {
    state.readerPhase == NoRead,
    command' = BeginRead,
    state' = { ...state, readerPhase: SnapshotCaptured, captured: snapshot(state.committed), result: emptySnapshot },
  }
  action finishRead = all {
    state.readerPhase == SnapshotCaptured,
    command' = FinishRead,
    state' = { ...state, readerPhase: ReadCompleted, result: state.captured },
  }
  action closeRead = all {
    state.readerPhase == ReadCompleted,
    command' = CloseRead,
    state' = { ...state, readerPhase: NoRead, captured: emptySnapshot, result: emptySnapshot },
  }
  action step = {
    nondet kind = Set(Execution, WithoutExecution).oneOf()
    any { start(kind), advance, crash, rollback, beginRead, finishRead, closeRead }
  }
  val safety = all {
    state.published == snapshot(state.committed),
    state.phase == Idle or state.candidate == WithoutExecution or state.durable,
    state.readerPhase != ReadCompleted or state.result == state.captured,
    state.committed.length() == 0 or state.committed[0] == Execution,
  }
  action publishExecution = start(Execution).then(advance).then(advance).then(advance)
  action publishWithoutExecution = start(WithoutExecution).then(advance).then(advance)

  run rollbackDuringReadTest = initWithGenesis(true).then(publishExecution)
    .then(beginRead).then(rollback).then(finishRead)
    .expect(all { state.committed == [Execution], state.result.created == [11], safety })

  run crashBeforeCommitTest = initWithGenesis(true).then(start(Execution))
    .then(advance).then(advance).then(crash)
    .expect(all { state.committed == [Execution], safety })

  run crashAfterCommitTest = initWithGenesis(true).then(start(Execution))
    .then(advance).then(advance).then(advance).then(crash)
    .expect(all { state.committed == [Execution, Execution], safety })

  run publishDuringReadTest = initWithGenesis(true).then(beginRead).then(publishExecution)
    .then(finishRead).expect(all { state.result.created == [], state.result.progress == 0, safety })

  run genesisCrashBeforeCommitTest = initWithGenesis(false).then(start(Execution))
    .then(advance).then(advance).then(crash)
    .expect(all { state.committed == [], safety }).then(publishExecution)
    .expect(all { snapshot(state.committed) == { progress: 0, stateSlot: 0, created: [] }, safety })

  run genesisCrashAfterCommitTest = initWithGenesis(false).then(start(Execution))
    .then(advance).then(advance).then(advance).then(crash)
    .expect(all { state.committed == [Execution], safety }).then(publishExecution)
    .expect(all { snapshot(state.committed).created == [11], safety })

  run genesisDuringReadTest = initWithGenesis(false).then(beginRead).then(publishExecution)
    .then(finishRead).expect(all { state.result == emptySnapshot, state.committed == [Execution], safety })

  run progressWithoutStateTest = initWithGenesis(true).then(beginRead).then(publishWithoutExecution)
    .then(finishRead).expect(all { state.result.progress == 0, snapshot(state.committed).progress == 1,
      snapshot(state.committed).stateSlot == 0, safety }).then(closeRead).then(crash)
    .then(publishExecution).then(beginRead).then(rollback).then(finishRead)
    .expect(all { state.result.created == [12], snapshot(state.committed).stateSlot == 0,
      snapshot(state.committed).progress == 1, safety })
}
