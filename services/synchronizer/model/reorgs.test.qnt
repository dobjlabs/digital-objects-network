module reorgs_tests {
  import reorgs.* from "./reorgs"

  run skippedSlotForkSwitchRewindsTest = initWithGap(true)
    .then(fetch(2)).then(process).then(process)
    .then(fetch(1)).then(process)
    .then(switchFork)
    .then(fetch(2)).then(process)
    .expect(all {
      nextSlot == 2,
      fetched == [],
      createdIndex == [{ commitment: 11, index: 0, slot: 1 }],
      headState.created == [11],
      headState.nullifiers == Set(),
      headState.history == [1, 11],
      safety,
    })
    .then(fetch(2)).then(process).then(process)
    .then(fetch(2)).then(process).then(process)
    .then(fetch(1)).then(process)
    .expect(all {
      headState.created == [11, 22, 24],
      headState.nullifiers == Set(22),
      headState.history == [1, 11, 22, 24, 26],
      safety,
    })

  run adjacentSlotForkSwitchRewindsTest = initWithGap(false)
    .then(fetch(2)).then(process).then(process)
    .then(switchFork)
    .then(fetch(1)).then(process)
    .expect(all { nextSlot == 2, safety })

  run staleBatchRemainsCoherentTest = initWithGap(true)
    .then(fetch(2))
    .then(switchFork)
    .then(process).then(process)
    .expect(all { headState.created == [11, 12], safety })
    .then(fetch(2)).then(process).then(process)
    .expect(all { nextSlot == 2, safety })

  run restartDropsFetchedSlotsTest = initWithGap(true)
    .then(fetch(2)).then(process)
    .then(restart)
    .expect(all {
      nextSlot == 2,
      fetched == [],
      headState.created == [11],
      safety,
    })
    .then(fetch(2)).then(process).then(process)
    .then(fetch(1)).then(process)
    .expect(all { headState.created == [11, 12, 14], safety })

  run mixedBatchRejectsDivergentParentTest = initWithGap(false)
    .then(fetch(1)).then(process)
    .then(fetchMixedBatch)
    .then(process)
    .expect(all { headState.created == [11, 12], safety })
    .then(process)
    .expect(all { nextSlot == 2, fetched == [], safety })

  run mixedBatchAcrossGapRewindsTest = initWithGap(true)
    .then(fetch(2)).then(process).then(process)
    .then(fetchMixedBatch)
    .then(process).then(process)
    .expect(all { nextSlot == 2, fetched == [], safety })

  run slotKindsAndSingleFetchTest = initWithGap(true)
    .then(fetch(1)).then(process)
    .then(fetch(1)).then(process)
    .then(fetch(1)).then(process)
    .expect(all { headState.history == [1, 11, 12], safety })
    .then(fetch(1)).then(process)
    .then(fetch(1)).then(process)
    .expect(all { headState.history == [1, 11, 12, 14], safety })
    .then(fetch(1)).then(process)
    .expect(all {
      headState.created == [11, 12, 14],
      headState.nullifiers == Set(12),
      headState.history == [1, 11, 12, 14, 16],
      safety,
    })

  run slotKindsAndBatchFetchTest = initWithGap(true)
    .then(fetch(2)).then(process).then(process)
    .then(fetch(2)).then(process).then(process)
    .then(fetch(2)).then(process).then(process)
    .expect(all {
      headState.created == [11, 12, 14],
      headState.nullifiers == Set(12),
      headState.history == [1, 11, 12, 14, 16],
      safety,
    })

  run rollbackDropsSpentStateTest = initWithGap(true)
    .then(fetch(2)).then(process).then(process)
    .then(fetch(2)).then(process).then(process)
    .expect(headState.nullifiers == Set(12))
    .then(switchFork)
    .then(fetch(1)).then(process)
    .expect(all {
      nextSlot == 2,
      headState.created == [11],
      headState.nullifiers == Set(),
      headState.history == [1, 11],
      safety,
    })
  run forkSpecificGapsTest = initLayout({ forkPoint: 2, firstGaps: Set(2, 3), secondGaps: Set(3) })
    .then(fetch(2)).then(process).then(process)
    .then(fetch(1)).then(process).then(switchFork)
    .then(fetch(1)).then(process)
    .expect(all { nextSlot == 2, safety })
    .then(fetch(2)).then(process).then(process)
    .then(fetch(2)).then(process).then(process)
    .expect(all { headState.created == [11, 22, 24], safety })

  run sharedPrefixRewindTest = initLayout({ forkPoint: 4, firstGaps: Set(3), secondGaps: Set(3) })
    .then(fetch(2)).then(process).then(process)
    .then(fetch(2)).then(process).then(process)
    .then(switchFork).then(fetch(1)).then(process)
    .expect(all { nextSlot == 3, headState.created == [11, 12], safety })

  run genesisRewindTest = initLayout({ forkPoint: 1, firstGaps: Set(2, 3), secondGaps: Set(2, 3) })
    .then(fetch(2)).then(process).then(process)
    .then(fetch(1)).then(process).then(switchFork)
    .then(fetch(1)).then(process)
    .expect(all { nextSlot == 1, headState.created == [], safety })
  run genesisRestartTest = initWithGap(true).then(restart)
    .expect(all { nextSlot == 1, headState.slot == 0, headState.history == [1], safety })

  run progressWithoutStateTest = initWithGap(true)
    .then(fetch(2)).then(process).then(process)
    .then(fetch(1)).then(process).then(restart)
    .expect(all { nextSlot == 4, headState.slot == 2, headState.history == [1, 11, 12], safety })
    .then(fetch(2)).then(process).then(process).then(restart)
    .expect(all { nextSlot == 6, headState.slot == 4, headState.history == [1, 11, 12, 14], safety })
}
