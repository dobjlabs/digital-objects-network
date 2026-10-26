module acquisition {
  type AcquisitionPhase = Idle | Linked | HeaderPrepared | BlobStored | PayloadReady | Published
  type State = { fork: int, withheld: Set[int], baseSlot: int, committed: List[int],
    phase: AcquisitionPhase, pending: int, blobs: int -> Set[int] }
  type Command = Init({ withheld: Set[int], base_slot: int }) | Start | Advance | Crash | SwitchFork
  pure val roots = Set(11, 12, 13, 14, 22, 23, 24)
  var state: State
  var command: Command
  pure def rootAt(fork: int, position: int): int = if (position == 1) 11 else fork * 10 + position
  pure def parent(root: int): int = if (root == 11) 1 else rootAt(root / 10, root % 10 - 1)
  pure def target(root: int, withheld: Set[int]): int =
    if (parent(root) == 1 or withheld.contains(root)) 0 else parent(root)
  pure val emptyBlobs: int -> Set[int] = roots.mapBy(_ => Set())

  action initWith(withheld: Set[int], baseSlot: int): bool = all {
    command' = Init({ withheld: withheld, base_slot: baseSlot }),
    state' = { fork: 1, withheld: withheld, baseSlot: baseSlot, committed: [],
      phase: Idle, pending: 0, blobs: emptyBlobs },
  }
  action init = {
    nondet withheld = Set(Set(), Set(12), Set(22), Set(12, 23)).oneOf()
    nondet baseSlot = Set(1, 998, 999998).oneOf()
    initWith(withheld, baseSlot)
  }
  action switchFork = all { command' = SwitchFork, state' = { ...state, fork: 3 - state.fork } }
  action start = {
    val root = rootAt(state.fork, state.committed.length() + 1)
    val previous = state.committed.foldl(1, (_, current) => current)
    all {
      state.phase == Idle, state.committed.length() < 4,
      command' = Start,
      state' = if (parent(root) != previous) {
        ...state,
        committed: state.committed.slice(0, state.committed.length() - 1),
        blobs: state.blobs.set(previous, Set()),
      } else { ...state, phase: Linked, pending: root },
    }
  }
  action advance = {
    val owner = if (state.pending == 0) 0 else target(state.pending, state.withheld)
    val missing = if (owner == 0) Set() else Set(0, 1).exclude(state.blobs.get(owner))
    val firstMissing = missing.fold(2, (first, index) => if (index < first) index else first)
    all {
      state.phase != Idle,
      command' = Advance,
      state' = if (state.phase == Linked) { ...state, phase: HeaderPrepared }
        else if (state.phase == HeaderPrepared or state.phase == BlobStored) {
          ...state,
          phase: if (missing.size() > 0) BlobStored else PayloadReady,
          blobs: if (missing.size() > 0) state.blobs.set(owner,
            state.blobs.get(owner).union(Set(firstMissing))) else state.blobs,
        }
        else if (state.phase == PayloadReady) { ...state, phase: Published, committed: state.committed.append(state.pending) }
        else { ...state, phase: Idle, pending: 0 },
    }
  }
  action crash = all {
    command' = Crash,
    state' = { ...state, phase: Idle, pending: 0 },
  }
  action step = any { start, advance, crash, switchFork }
  val safety = state.committed.indices().forall(index => all {
    parent(state.committed[index]) == (if (index == 0) 1 else state.committed[index - 1]),
    target(state.committed[index], state.withheld) == 0 or
      state.blobs.get(target(state.committed[index], state.withheld)) == Set(0, 1),
  })

  run partialParentPayloadRestartTest = initWith(Set(), 998)
    .then(start).then(advance).then(advance).then(advance).then(advance)
    .then(start).then(advance).then(advance).then(crash)
    .expect(all { state.committed == [11], state.blobs.get(11) == Set(0), safety })
    .then(start).then(advance).then(advance).then(advance).then(advance).then(advance)
    .expect(all { state.committed == [11, 12], state.blobs.get(11) == Set(0, 1), safety })

  run publishedRestartTest = initWith(Set(), 999998)
    .then(start).then(advance).then(advance).then(advance).then(crash)
    .expect(all { state.committed == [11], safety })

  run withheldPayloadTest = initWith(Set(12), 1)
    .then(start).then(advance).then(advance).then(advance).then(advance)
    .then(start).then(advance).then(advance).then(advance).then(advance)
    .expect(all { state.committed == [11, 12], state.blobs.get(11) == Set(), safety })
  run reorgRetainsAncestorPayloadTest = initWith(Set(), 998)
    .then(start).then(advance).then(advance).then(advance).then(advance)
    .then(start).then(advance).then(advance).then(advance).then(advance).then(advance).then(advance)
    .then(start).then(advance).then(advance).then(advance).then(advance).then(advance).then(advance)
    .then(switchFork).then(start).then(start)
    .expect(all { state.committed == [11], state.blobs.get(11) == Set(0, 1),
      state.blobs.get(12) == Set(), safety })
    .then(start).then(advance).then(advance).then(advance).then(advance)
    .expect(all { state.committed == [11, 22], safety })
}
