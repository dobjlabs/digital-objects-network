module reorgs {
  type Block = {
    slot: int,
    root: int,
    parent: int,
    hasPayload: bool,
    creates: List[int],
    spends: Set[int],
  }
  type DonState = { slot: int, created: List[int], nullifiers: Set[int], history: List[int] }
  type IndexEntry = { commitment: int, index: int, slot: int }
  type Layout = { forkPoint: int, firstGaps: Set[int], secondGaps: Set[int] }
  type Command = Init({ config: Layout }) | SwitchFork
    | Fetch({ count: int, change_fork: bool }) | Process | Restart

  pure val lastSlot = 6
  pure val emptyState: DonState = { slot: -1, created: [], nullifiers: Set(), history: [] }
  pure val genesis: Block = {
    slot: 0, root: 1, parent: 0, hasPayload: true, creates: [], spends: Set(),
  }

  pure def gapsFor(fork: int, config: Layout): Set[int] =
    if (fork == 1) config.firstGaps else config.secondGaps

  pure def rootAt(fork: int, config: Layout, slot: int): int =
    if (slot == 0) 1
    else if (gapsFor(fork, config).contains(slot)) 0
    else (if (slot < config.forkPoint) 10 else fork * 10) + slot

  pure def blockAt(fork: int, config: Layout, slot: int): Block = {
    val root = rootAt(fork, config, slot)
    val parentSlot = range(0, slot).select(previous => rootAt(fork, config, previous) != 0)
      .foldl(0, (_, previous) => previous)
    if (slot == 0) genesis
    else if (root == 0) {
      slot: slot, root: 0, parent: 0, hasPayload: false, creates: [], spends: Set(),
    } else {
      slot: slot,
      root: root,
      parent: rootAt(fork, config, parentSlot),
      hasPayload: slot != 5,
      creates: if (slot <= 4) [root] else [],
      spends: if (slot == 4 and rootAt(fork, config, 2) != 0)
        Set(rootAt(fork, config, 2)) else Set(),
    }
  }

  pure def applyBlock(headState: DonState, block: Block): DonState =
    if (block.hasPayload) {
      slot: block.slot,
      created: headState.created.concat(block.creates),
      nullifiers: headState.nullifiers.union(block.spends),
      history: headState.history.append(block.root),
    } else headState

  pure def replay(blocks: List[Block]): DonState =
    blocks.foldl(emptyState, (headState, block) => applyBlock(headState, block))

  var remoteFork: int
  var layout: Layout
  var committed: List[Block]
  var states: List[DonState]
  var createdIndex: List[IndexEntry]
  var fetched: List[Block]
  var command: Command

  val nextSlot = committed.length()
  val headState = states[states.length() - 1]

  action initLayout(config: Layout): bool = all {
    command' = Init({ config: config }),
    remoteFork' = 1,
    layout' = config,
    committed' = [genesis],
    states' = [applyBlock(emptyState, genesis)],
    createdIndex' = [],
    fetched' = [],
  }

  action initWithGap(hasGap: bool): bool = initLayout({
    forkPoint: 2, firstGaps: if (hasGap) Set(3) else Set(),
    secondGaps: if (hasGap) Set(3) else Set(),
  })

  action init = {
    nondet forkPoint = Set(1, 2, 4).oneOf()
    nondet firstGaps = Set(Set(), Set(2), Set(3), Set(2, 3)).oneOf()
    nondet secondGaps = Set(Set(), Set(2), Set(3), Set(2, 3))
      .filter(gaps => gaps.filter(slot => slot < forkPoint) ==
        firstGaps.filter(slot => slot < forkPoint)).oneOf()
    initLayout({ forkPoint: forkPoint, firstGaps: firstGaps, secondGaps: secondGaps })
  }

  action switchFork = all {
    command' = SwitchFork,
    remoteFork' = 3 - remoteFork,
    layout' = layout,
    committed' = committed,
    states' = states,
    createdIndex' = createdIndex,
    fetched' = fetched,
  }

  action fetchSlots(count: int, changeFork: bool): bool = {
    val finalFork = if (changeFork) 3 - remoteFork else remoteFork
    all {
      command' = Fetch({ count: count, change_fork: changeFork }),
      fetched.length() == 0,
      count > 0,
      nextSlot + count - 1 <= lastSlot,
      remoteFork' = finalFork,
      layout' = layout,
      committed' = committed,
      states' = states,
      createdIndex' = createdIndex,
      fetched' = range(nextSlot, nextSlot + count).foldl([], (blocks, slot) =>
        blocks.append(blockAt(if (slot == nextSlot) remoteFork else finalFork, layout, slot))),
    }
  }

  action fetch(count: int): bool = fetchSlots(count, false)
  action fetchMixedBatch = fetchSlots(2, true)

  // Only actual matching block roots anchor a rewind; matching gaps do not.
  val commonSlots = committed.indices().filter(slot =>
    committed[slot].root != 0 and
      committed[slot].root == blockAt(remoteFork, layout, slot).root)
  val keepSlot = commonSlots.fold(0, (latest, slot) => if (slot > latest) slot else latest)

  action rewind = all {
    remoteFork' = remoteFork,
    layout' = layout,
    committed' = committed.slice(0, keepSlot + 1),
    states' = states.select(state => state.slot <= keepSlot),
    createdIndex' = createdIndex.select(entry => entry.slot <= keepSlot),
    fetched' = [],
  }

  action publish(block: Block): bool = all {
    block.slot == nextSlot,
    remoteFork' = remoteFork,
    layout' = layout,
    committed' = committed.append(block),
    states' = if (block.hasPayload) states.append(applyBlock(headState, block)) else states,
    createdIndex' = createdIndex.concat(
      range(0, block.creates.length()).foldl([], (entries, offset) => entries.append({
        commitment: block.creates[offset],
        index: headState.created.length() + offset,
        slot: block.slot,
      }))),
    fetched' = fetched.tail(),
  }

  val nearestBlock = committed.select(block => block.root != 0)
    .foldl(genesis, (_, block) => block)

  action process = all {
    command' = Process,
    fetched.length() > 0,
    {
      val block = fetched.head()
      val parent = nearestBlock
      if (block.root != 0 and parent.root != 0 and block.parent != parent.root)
        rewind
      else publish(block)
    },
  }

  // Restart discards speculative fetches and resumes after the highest committed slot.
  action restart = all {
    command' = Restart,
    remoteFork' = remoteFork,
    layout' = layout,
    committed' = committed,
    states' = states,
    createdIndex' = createdIndex,
    fetched' = [],
  }

  action step = any {
    switchFork,
    fetch(1),
    fetch(2),
    fetchMixedBatch,
    process,
    restart,
  }

  val contiguousSlots = committed.indices().forall(slot => committed[slot].slot == slot)

  val producedBlocks = committed.select(block => block.root != 0)
  val coherentBranch = producedBlocks.indices().forall(index =>
    index == 0 or producedBlocks[index].parent == producedBlocks[index - 1].root)

  val replayMatchesStates = all {
    states.length() == committed.select(block => block.hasPayload).length(),
    states.indices().forall(index => {
      val block = committed.select(block => block.hasPayload)[index]
      states[index] == replay(committed.slice(0, block.slot + 1))
    }),
  }

  val indexMatchesDonState = all {
    createdIndex.length() == headState.created.length(),
    createdIndex.indices().forall(index => all {
      createdIndex[index].index == index,
      createdIndex[index].commitment == headState.created[index],
      createdIndex[index].slot < nextSlot,
      committed[createdIndex[index].slot].creates
        .foldl(false, (found, object) => found or object == createdIndex[index].commitment),
    }),
  }

  val uniqueCreations = headState.created.length() == headState.created
    .foldl(Set(), (objects, object) => objects.union(Set(object))).size()
  val safety = all {
    contiguousSlots,
    coherentBranch,
    replayMatchesStates,
    indexMatchesDonState,
    uniqueCreations,
  }
}
