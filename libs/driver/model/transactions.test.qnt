// Deterministic traces for the transaction model's boundary cases.
// Each run starts from init. then applies an action; expect checks the resulting
// state. fail means the attempted action is disabled by its guards, rather than
// that an exception occurred. safety checks all invariants at the marked points.
//
// Proof IDs also identify their recorded transactions. Objects 1 and 2 start
// Live; objects 3 and 4 are available as new output names.
module transactions_tests {
  import transactions.* from "./transactions"

  // Normal lifecycle
  //
  // The proof first holds input 1, then its record owns that input. Submission
  // and broadcast leave output 3 Unknown; only landing and settlement make it Live.
  run happyPathTest = init
    .then(startProofWith(1, Set(1), Set(3)))
    .expect(all { node.holds == Map(1 -> 1), freeLiveObjects == Set(2), safety })
    .then(finishProof(1))
    .expect(all {
      node.holds == Map(),
      node.records.get(1).state == Recorded,
      node.objects.get(3) == { status: Unknown, tx: 1 },
      freeLiveObjects == Set(2),
      safety,
    })
    .then(submit(1)).then(relayerBroadcast(1)).then(settle)
    .expect(all { node.records.get(1).state == Submitted, node.records.get(1).broadcast, safety })
    .then(chainLand(1)).then(settle)
    .expect(all {
      node.records.get(1).state == Landed,
      node.objects.get(3).status == Live,
      node.objects.get(1).status == Nullified,
      freeLiveObjects == Set(2, 3),
      safety,
    })

  // A relayer failure leaves the transaction open and its output unconfirmed.
  // The same payload can still land, so settlement must later promote the output.
  run relayerFailureIsAnAnnotationTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(relayerFail(1)).then(settle)
    .expect(all {
      node.records.get(1).state == Submitted,
      node.records.get(1).relayerFailed,
      node.objects.get(3).status == Unknown,
      safety,
    })
    .then(chainLand(1)).then(settle)
    .expect(all { node.records.get(1).state == Landed, node.objects.get(3).status == Live, safety })

  // Land exactly at the age limit, advance beyond it, then settle. Inclusion
  // must win over age so the driver never expires a transaction already included.
  run landedAtTheAgeLimitStaysLandedTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(chainAdvance).then(chainAdvance)
    // Landing is allowed at age == maxAge; the next block makes an absent tx expire.
    .then(chainLand(1))
    .then(chainAdvance)
    .expect(chain.head - node.records.get(1).grounding > maxAge)
    .then(settle)
    .expect(all { node.records.get(1).state == Landed, node.objects.get(3).status == Live, safety })

  // A proof the chain refused expires as soon as settlement sees the rejection,
  // and the chain can never land it afterwards.
  run rejectedProofExpiresAndCannotLandTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(chainAdvance).then(chainAdvance).then(chainAdvance)
    .then(chainReject(1))
    .then(settle)
    .expect(all {
      node.records.get(1).state == Expired,
      not(node.objects.keys().contains(3)),
      freeLiveObjects == Set(1, 2),
      safety,
    })
    .then(chainLand(1)).fail()

  // The chain cannot reject a payload it would still accept.
  run acceptableProofIsNotRejectedTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(chainReject(1)).fail()

  // Expiry and ownership
  //
  // Once grounding expires without landing, remove the unknown output and free
  // its input. A new proof can reuse both that input and the removed output name.
  run expiryFreesInputsAndRemovesOutputsTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(chainAdvance).then(chainAdvance).then(chainAdvance).then(settle)
    .expect(all {
      node.records.get(1).state == Expired,
      not(node.objects.keys().contains(3)),
      node.objects.get(1).status == Live,
      freeLiveObjects == Set(1, 2),
      safety,
    })
    .then(startProofWith(2, Set(1), Set(3))).then(finishProof(2))
    .expect(all { node.records.get(2).state == Recorded, node.objects.get(3) == { status: Unknown, tx: 2 }, safety })

  // Expiry also applies to records that were never submitted. Their unknown
  // outputs must be removed once the grounding becomes stale.
  run recordedExpiresWithoutSubmitTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(chainAdvance).then(chainAdvance).then(chainAdvance).then(settle)
    .expect(all { node.records.get(1).state == Expired, not(node.objects.keys().contains(3)), safety })

  // A stale proof is rejected before the relayer gets a job.
  run submitRefusesAgedGroundingTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(chainAdvance).then(chainAdvance).then(chainAdvance)
    .then(submit(1))
    .fail()

  // Two simultaneous proofs cannot claim input 1.
  run heldInputRefusedTest = init
    .then(startProofWith(1, Set(1), Set(3)))
    .then(startProofWith(2, Set(1), Set(4)))
    .fail()

  // Recording releases the hold but retains ownership of input 1. Another
  // proof must still be refused, even before the first transaction is submitted.
  run openRecordInputRefusedTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(startProofWith(2, Set(1), Set(4)))
    .fail()

  // Distinct inputs allow two proofs to propose the same output name. Only the
  // first records successfully; the refused job releases its input and its hold.
  run outputCollisionRefusedAtRecordTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(startProofWith(2, Set(2), Set(3)))
    .expect(all { node.holds == Map(1 -> 1, 2 -> 2), freeLiveObjects == Set(), safety })
    // The second finish is enabled, but takes the recording-refusal branch.
    .then(finishProof(1)).then(finishProof(2))
    .expect(all {
      node.records.keys() == Set(1),
      node.holds == Map(),
      node.objects.get(3).tx == 1,
      freeLiveObjects == Set(2),
      safety,
    })

  // Recording crashes
  //
  // An output written before the record is a crash leftover, with no payload or
  // transaction ownership. Its input is free for a new proof after memory is lost.
  run crashBeforeRecordLeavesInertOutputsTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(crash(1, OutputsWritten))
    .expect(all {
      node.holds == Map(),
      node.jobs == Map(),
      node.records == Map(),
      node.payloads == Set(),
      node.objects.get(3) == { status: Unknown, tx: 1 },
      freeLiveObjects == Set(1, 2),
      safety,
    })
    .then(startProofWith(2, Set(1), Set(4))).then(finishProof(2))
    .expect(all { node.records.get(2).state == Recorded, safety })

  // A payload alone is not a committed transaction. The missing record means
  // no durable input ownership, even though the payload write completed.
  run crashAfterPayloadLeavesNoRecordTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(crash(1, PayloadWritten))
    .expect(all { node.payloads == Set(1), node.records == Map(), freeLiveObjects == Set(1, 2), safety })

  // Once the record is written, the transaction survives a crash and can still
  // be submitted and settled. The process's lost holds no longer matter.
  run crashAfterRecordIsCommittedTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(crash(1, RecordWritten))
    .expect(all { node.records.get(1).state == Recorded, node.holds == Map(), freeLiveObjects == Set(2), safety })
    .then(submit(1)).then(chainLand(1)).then(settle)
    .expect(all { node.records.get(1).state == Landed, safety })

  // Submission crashes
  //
  // The relayer accepts the payload, but a crash loses its job ID locally.
  // Retrying attaches the existing relayer job rather than creating a second one.
  run crashAfterRelayerAcceptResubmitsIdempotentlyTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(crash(1, RelayerAccepted))
    .expect(all { node.records.get(1).state == Recorded, node.records.get(1).attempted, relayer.keys() == Set(1), safety })
    .then(retry(1))
    .expect(all { node.records.get(1).state == Submitted, relayer.keys() == Set(1), safety })
    .then(chainLand(1)).then(settle)
    .expect(all { node.records.get(1).state == Landed, safety })

  // The accepted payload can land before any retry. Settlement must query the
  // status of a Recorded transaction even without a stored relayer job ID.
  run crashAfterRelayerAcceptStillSettlesTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(crash(1, RelayerAccepted))
    .then(chainLand(1)).then(settle)
    .expect(all { node.records.get(1).state == Landed, node.objects.get(3).status == Live, safety })

  // Safe abandonment
  //
  // A local Recorded state does not prove deletion is safe: the relayer may
  // already hold the payload after a submission crash.
  run deleteRefusedAfterRelayerAcceptTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(crash(1, RelayerAccepted))
    .then(delete(1))
    .fail()

  // Known relayer acceptance also prevents abandonment.
  run deleteRefusedOnceSubmittedTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(delete(1))
    .fail()

  // A record with no relayer acceptance can be deleted. Its payload and output
  // disappear, and its unspent input becomes available again.
  run deleteRecordedTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(delete(1))
    .expect(all {
      node.records == Map(),
      node.payloads == Set(),
      node.objects.keys() == Set(1, 2),
      freeLiveObjects == Set(1, 2),
      deleted == Set(1),
      safety,
    })

  // Restart drops an unfinished proof and its holds. No transaction was
  // recorded, so both genesis inputs remain available.
  run restartReleasesHoldsTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(restart)
    .expect(all { node.holds == Map(), node.jobs == Map(), node.objects.keys() == Set(1, 2), freeLiveObjects == Set(1, 2), safety })

  // Multiple transactions
  //
  // A landed output can become the next transaction's input. Settling that
  // spend must leave the earlier transaction Landed while nullifying its output.
  run chainedSpendTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1)).then(chainLand(1)).then(settle)
    .then(startProofWith(2, Set(3), Set(4))).then(finishProof(2)).then(submit(2)).then(chainLand(2)).then(settle)
    .expect(all {
      node.records.get(1).state == Landed,
      node.records.get(2).state == Landed,
      node.objects.get(1).status == Nullified,
      node.objects.get(3).status == Nullified,
      node.objects.get(4).status == Live,
      freeLiveObjects == Set(2, 4),
      safety,
    })

  // Independently grounded transactions can have different outcomes in one
  // pass: the older one expires while the newer one lands. Only the expired
  // transaction's input is freed; the landed transaction's input is spent.
  run oneExpiresWhileTheOtherLandsTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(chainAdvance)
    .then(startProofWith(2, Set(2), Set(4))).then(finishProof(2)).then(submit(2))
    .expect(all { openRecords == Set(1, 2), freeLiveObjects == Set(), safety })
    .then(chainLand(2))
    .then(chainAdvance).then(chainAdvance).then(settle)
    .expect(all {
      node.records.get(1).state == Expired,
      node.records.get(2).state == Landed,
      not(node.objects.keys().contains(3)),
      node.objects.get(4).status == Live,
      node.objects.get(1).status == Live,
      node.objects.get(2).status == Nullified,
      freeLiveObjects == Set(1, 4),
      safety,
    })

  run failedSubmissionSurvivesRestartAndRetriesTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(submitWith(1, false, Unavailable))
    .expect(all {
      node.records.get(1).state == Recorded,
      node.records.get(1).attempted,
      relayer == Map(),
      freeLiveObjects == Set(2),
      safety,
    })
    .then(restart)
    .expect(all { node.records.get(1).attempted, safety })
    .then(submitWith(1, true, Unavailable))
    .expect(all { node.records.get(1).state == Recorded, relayer == Map(), safety })
    .then(retry(1)).then(chainLand(1)).then(settle)
    .expect(all { node.records.get(1).state == Landed, safety })

  run failedSubmissionCannotBeDeletedEvenWhenRelayerHasNothingTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(submitWith(1, false, Unavailable))
    .expect(all { relayer == Map(), node.records.get(1).attempted, safety })
    .then(delete(1)).fail()

  run automaticRetryRefusesNeverSubmittedRecordTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(restart)
    .expect(all { not(node.records.get(1).attempted), relayer == Map(), safety })
    .then(retry(1)).fail()

  run lostResponsesReuseExistingRelayerJobTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(submitWith(1, false, ResponseLost)).then(relayerBroadcast(1))
    .then(submitWith(1, true, ResponseLost))
    .expect(all {
      node.records.get(1).state == Recorded,
      relayer.keys() == Set(1),
      relayer.get(1).broadcast,
      safety,
    })
    .then(restart).then(retry(1))
    .expect(all { node.records.get(1).state == Submitted, relayer.get(1).broadcast, safety })
    .then(chainLand(1)).then(settle)
    .expect(all { node.records.get(1).state == Landed, safety })

  run automaticRetryRefusesExpiredGroundingTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(submitWith(1, false, Unavailable))
    .then(chainAdvance).then(chainAdvance).then(chainAdvance)
    .then(retry(1)).fail()

  run expiryStopsAutomaticRetryTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(submitWith(1, false, Unavailable))
    .then(chainAdvance).then(chainAdvance).then(chainAdvance).then(settle)
    .expect(all { node.records.get(1).state == Expired, freeLiveObjects == Set(1, 2), safety })
    .then(retry(1)).fail()

  run landingStopsAutomaticRetryTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(submitWith(1, false, ResponseLost)).then(chainLand(1)).then(settle)
    .expect(all { node.records.get(1).state == Landed, safety })
    .then(retry(1)).fail()

  run partialOutputFailureRollsBackOnlyItsOwnWorkTest = init
    .then(startProofWith(1, Set(1), Set(3, 4)))
    .then(startProofWith(2, Set(2), Set(3)))
    .then(recordFailed(1, OutputWrite, Set(3)))
    .expect(all {
      node.objects.keys() == Set(1, 2),
      node.records == Map(),
      node.payloads == Set(),
      node.jobs.keys() == Set(2),
      node.holds == Map(2 -> 2),
      strays == Set(),
      freeLiveObjects == Set(1),
      safety,
    })
    .then(finishProof(2))
    .expect(all { node.objects.get(3).tx == 2, safety })

  run payloadFailureRollsBackAndReleasesInputTest = init
    .then(startProofWith(1, Set(1), Set(3)))
    .then(recordFailed(1, PayloadWrite, Set(3)))
    .expect(all {
      node.objects.keys() == Set(1, 2),
      node.records == Map(),
      node.payloads == Set(),
      node.jobs == Map(),
      node.holds == Map(),
      strays == Set(),
      freeLiveObjects == Set(1, 2),
      safety,
    })
    .then(startProofWith(2, Set(1), Set(3))).then(finishProof(2))
    .expect(all { node.objects.get(3).tx == 2, safety })

  run recordFailureRollsBackPayloadAndOutputsTest = init
    .then(startProofWith(1, Set(1), Set(3)))
    .then(recordFailed(1, RecordWrite, Set(3)))
    .expect(all {
      node.objects.keys() == Set(1, 2),
      node.records == Map(),
      node.payloads == Set(),
      node.jobs == Map(),
      node.holds == Map(),
      strays == Set(),
      safety,
    })

  run settlementSkipsRecordsCommittedAfterQueryTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(startProofWith(2, Set(2), Set(4)))
    .then(chainAdvance).then(chainAdvance).then(chainAdvance)
    .then(querySettlement)
    .expect(node.settlement.transactions == Set(1))
    .then(finishProof(2)).then(applySettlement)
    .expect(all {
      node.records.get(1).state == Expired,
      node.records.get(2).state == Recorded,
      node.records.get(2) == previousRecords.get(2),
      node.objects.get(4) == { status: Unknown, tx: 2 },
      not(node.objects.keys().contains(3)),
      freeLiveObjects == Set(1),
      safety,
    })
    .then(settle)
    .expect(all { node.records.get(2).state == Expired, not(node.objects.keys().contains(4)), safety })

  run settlementKeepsStatusAndExpiryAtSameHeadTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(chainAdvance).then(chainAdvance).then(querySettlement)
    .then(chainLand(1)).then(chainAdvance).then(applySettlement)
    .expect(all {
      node.records.get(1).state == Submitted,
      node.objects.get(3).status == Unknown,
      safety,
    })
    .then(settle)
    .expect(all { node.records.get(1).state == Landed, node.objects.get(3).status == Live, safety })

  run settlementDoesNotExpireReplacementAfterDeletionTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(chainAdvance).then(chainAdvance).then(chainAdvance).then(querySettlement)
    .then(delete(1)).then(startProofWith(2, Set(1), Set(3))).then(finishProof(2))
    .then(applySettlement)
    .expect(all {
      node.records.keys() == Set(2),
      node.records.get(2).state == Recorded,
      node.objects.get(3) == { status: Unknown, tx: 2 },
      safety,
    })

  run restartDiscardsPendingSettlementTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(submitWith(1, false, Unavailable)).then(querySettlement).then(restart)
    .expect(all {
      not(node.settlement.pending),
      node.records.get(1).state == Recorded,
      node.records.get(1).attempted,
      safety,
    })
    .then(retry(1)).then(chainLand(1)).then(querySettlement).then(applySettlement)
    .expect(all { node.records.get(1).state == Landed, safety })

  run concurrentSettlementQueriesAreRefusedTest = init
    .then(querySettlement).then(querySettlement).fail()


  run landedReceiptPruningPreservesObjectsAcrossRestartTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(chainLand(1)).then(settle).then(prune(1)).then(restart)
    .expect(all {
      node.records == Map(),
      node.payloads == Set(),
      node.objects.get(3) == { status: Live, tx: 1 },
      node.objects.get(1).status == Nullified,
      prunedRecords.get(1).state == Landed,
      safety,
    })
    .then(startProofWith(2, Set(3), Set(4))).then(finishProof(2)).then(submit(2))
    .then(chainLand(2)).then(settle)
    .expect(all { node.objects.get(3).status == Nullified, safety })

  run expiredReceiptPruningKeepsInputAvailableTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(chainAdvance).then(chainAdvance).then(chainAdvance).then(settle).then(prune(1))
    .expect(all {
      node.records == Map(),
      node.payloads == Set(),
      node.objects.keys() == Set(1, 2),
      freeLiveObjects == Set(1, 2),
      prunedRecords.get(1).state == Expired,
      safety,
    })
    .then(startProofWith(2, Set(1), Set(3))).then(finishProof(2))
    .expect(all { node.records.get(2).state == Recorded, safety })

  run pruningRefusesRecordedTransactionTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(prune(1)).fail()

  run pruningRefusesSubmittedTransactionTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(prune(1)).fail()


  run mixedQueriesRequestStatusOnlyForAttemptedRecordsTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(submit(1))
    .then(startProofWith(2, Set(2), Set(4))).then(finishProof(2)).then(querySettlement)
    .expect(all {
      node.settlement.transactions == Set(1, 2),
      node.settlement.statusTransactions == Set(1),
      safety,
    })
    .then(applySettlement)
    .expect(all { node.records.get(1).state == Submitted, node.records.get(2).state == Recorded, safety })

  run headOnlySnapshotWaitsForStatusAfterConcurrentSubmissionTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1)).then(querySettlement)
    .expect(node.settlement.statusTransactions == Set())
    .then(submitWith(1, false, ResponseLost)).then(chainLand(1))
    .then(chainAdvance).then(chainAdvance).then(chainAdvance).then(applySettlement)
    .expect(all {
      node.records.get(1).state == Recorded,
      node.records.get(1).attempted,
      node.objects.get(3).status == Unknown,
      safety,
    })
    .then(settle)
    .expect(all { node.records.get(1).state == Landed, node.objects.get(3).status == Live, safety })

  run headOnlyQueryExpiresAnUntouchedRecordTest = init
    .then(startProofWith(1, Set(1), Set(3))).then(finishProof(1))
    .then(chainAdvance).then(chainAdvance).then(chainAdvance).then(querySettlement)
    .expect(node.settlement.statusTransactions == Set())
    .then(applySettlement)
    .expect(all { node.records.get(1).state == Expired, node.objects.keys() == Set(1, 2), safety })

}
