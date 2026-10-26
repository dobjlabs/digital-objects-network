module delivery {
  type RpcBehavior = RespondNormally | RejectSendOrFailReceipt | LoseSendResponse
    | InterruptAcceptedSend | FailNonceOrFeeLookup | RefuseBroadcast | TransportUnavailable | InterruptBeforeSend
  type TransactionOutcome = Prepared | Pending | Succeeded | Reverted | Dropped
  type Job = { exists: bool, status: str, attempts: int, nonce: int, current: int,
    known: Set[int], submittedAt: int, bumps: int, nextAttemptAt: int, pending: bool }
  type Transaction = { hash: int, job: int, nonce: int, outcome: TransactionOutcome }
  type State = { timeoutEnabled: bool, now: int, jobs: List[Job], transactions: List[Transaction] }
  type Command = Init({ timeout_enabled: bool }) | Enqueue({ job: int }) | Process({ job: int, behavior: RpcBehavior })
    | PollIdle({ behavior: RpcBehavior }) | Mine({ hash: int, success: bool }) | Drop({ hash: int }) | Tick | Restart
  pure val emptyJob: Job = { exists: false, status: "absent", attempts: 0,
    nonce: -1, current: 0, known: Set(), submittedAt: -1, bumps: 0, nextAttemptAt: -1, pending: false }
  var state: State
  var command: Command

  pure def terminal(job: Job): bool = Set("confirmed", "failed").contains(job.status)
  pure def due(job: Job, now: int): bool = job.exists and not(terminal(job)) and job.nextAttemptAt <= now
  pure def transaction(transactions: List[Transaction], hash: int): Transaction = transactions[hash - 1]
  pure def nextNonce(transactions: List[Transaction]): int =
    0.to(10).filter(nonce => not(transactions.indices().exists(index =>
      transactions[index].nonce == nonce and not(Set(Prepared, Dropped).contains(transactions[index].outcome)))))
      .fold(11, (lowest, nonce) => if (nonce < lowest) nonce else lowest)
  pure def prepare(transactions: List[Transaction], job: int, nonce: int): List[Transaction] =
    transactions.append({ hash: transactions.length() + 1, job: job, nonce: nonce, outcome: Prepared })
  pure def knownReceipt(job: Job, transactions: List[Transaction]): int =
    job.known.filter(hash => Set(Succeeded, Reverted).contains(transaction(transactions, hash).outcome))
      .fold(0, (found, hash) => if (hash > found) hash else found)
  pure def inMempool(job: Job, transactions: List[Transaction]): bool =
    transaction(transactions, job.current).outcome == Pending and
      not(transactions.indices().exists(index => transactions[index].nonce == job.nonce and
        transactions[index].hash > job.current and transactions[index].outcome == Pending))

  action initWith(timeoutEnabled: bool): bool = all {
    command' = Init({ timeout_enabled: timeoutEnabled }),
    state' = { timeoutEnabled: timeoutEnabled, now: 0, jobs: [emptyJob, emptyJob], transactions: [] },
  }
  action init = {
    nondet timeoutEnabled = Set(false, true).oneOf()
    initWith(timeoutEnabled)
  }
  action enqueue(job: int): bool = all {
    command' = Enqueue({ job: job }),
    state' = if (state.jobs[job].exists) state else { ...state,
      jobs: state.jobs.replaceAt(job, { ...emptyJob, exists: true, status: "queued", nextAttemptAt: state.now }) },
  }
  action tick = all {
    state.now < 10, command' = Tick, state' = { ...state, now: state.now + 1 },
  }
  action restart = all {
    command' = Restart,
    state' = { ...state, jobs: state.jobs.foldl([], (jobs, job) => jobs.append(
      if (job.status == "sending") { ...job, status: "queued", nextAttemptAt: state.now } else job)) },
  }
  action mine(hash: int, success: bool): bool = all {
    hash > 0, hash <= state.transactions.length(),
    transaction(state.transactions, hash).outcome != Prepared,
    not(state.transactions.indices().exists(index =>
      state.transactions[index].nonce == transaction(state.transactions, hash).nonce and
      Set(Succeeded, Reverted).contains(state.transactions[index].outcome))),
    0.to(transaction(state.transactions, hash).nonce - 1).forall(nonce =>
      state.transactions.indices().exists(index => state.transactions[index].nonce == nonce and
        Set(Succeeded, Reverted).contains(state.transactions[index].outcome))),
    command' = Mine({ hash: hash, success: success }),
    state' = { ...state, transactions: state.transactions.foldl([], (transactions, tx) =>
      transactions.append(if (tx.hash == hash) { ...tx, outcome: if (success) Succeeded else Reverted }
        else if (tx.nonce == transaction(state.transactions, hash).nonce and tx.outcome != Prepared) { ...tx, outcome: Dropped }
        else tx)) },
  }
  action drop(hash: int): bool = all {
    hash > 0, hash <= state.transactions.length(),
    transaction(state.transactions, hash).outcome == Pending,
    command' = Drop({ hash: hash }),
    state' = { ...state, transactions: state.transactions.replaceAt(hash - 1,
      { ...transaction(state.transactions, hash), outcome: Dropped }) },
  }

  pure def send(job: Job, behavior: RpcBehavior, nonce: int, hash: int, now: int): Job = {
    val attempts = job.attempts + 1
    if (Set(RejectSendOrFailReceipt, FailNonceOrFeeLookup, RefuseBroadcast).contains(behavior)) { ...job, attempts: attempts,
      status: if (attempts >= 2) "failed" else "queued",
      nextAttemptAt: if (attempts >= 2) -1 else now + 1 }
    else { ...job, status: "submitted", attempts: attempts, nonce: nonce,
      current: hash, known: Set(hash), submittedAt: now, bumps: 0,
      pending: behavior != RespondNormally,
      nextAttemptAt: if (Set(InterruptBeforeSend, InterruptAcceptedSend).contains(behavior)) job.nextAttemptAt else now + 1 }
  }

  action process(job: int, behavior: RpcBehavior): bool = {
    val current = state.jobs[job]
    val receipt = if (current.current > 0) knownReceipt(current, state.transactions) else 0
    val expired = state.timeoutEnabled and current.current > 0 and state.now - current.submittedAt >= 8
    val bump = current.current > 0 and not(current.pending) and not(expired) and receipt == 0 and
      behavior != RejectSendOrFailReceipt and behavior != FailNonceOrFeeLookup and
      state.now - current.submittedAt >= 2 * (current.bumps + 1) and current.bumps < 2
    val pending = if (current.current > 0) inMempool(current, state.transactions) else false
    val sending = current.current == 0
    val retry = current.pending and not(expired) and receipt == 0 and behavior != RejectSendOrFailReceipt
    val consumed = state.transactions.indices().exists(index =>
      state.transactions[index].nonce == current.nonce and state.transactions[index].hash != current.current and
      Set(Succeeded, Reverted).contains(state.transactions[index].outcome))
    val preparing = (sending and not(Set(RejectSendOrFailReceipt, FailNonceOrFeeLookup).contains(behavior))) or
      (bump and pending)
    val accepted = not(Set(RefuseBroadcast, TransportUnavailable, InterruptBeforeSend).contains(behavior)) and
      ((retry and not(consumed)) or (preparing and not(consumed)))
    val reserved = state.jobs.foldl(0, (floor, job) =>
      if (not(terminal(job)) and job.pending and job.nonce >= floor) job.nonce + 1 else floor)
    val remoteNonce = nextNonce(state.transactions)
    val nonce = if (sending) (if (reserved > remoteNonce) reserved else remoteNonce) else current.nonce
    val hash = state.transactions.length() + 1
    val preparedTransactions = if (preparing) prepare(state.transactions, job, nonce) else state.transactions
    val updated = if (sending) send(current, behavior, nonce, hash, state.now)
      else if (behavior == RejectSendOrFailReceipt) { ...current, status: "submitted" }
      else if (receipt > 0) { ...current,
        status: if (transaction(state.transactions, receipt).outcome == Succeeded) "confirmed" else "failed",
        current: receipt, pending: false }
      else if (expired) { ...current, status: "failed", pending: false }
      else if (retry) { ...current, pending: consumed or not(Set(RespondNormally, FailNonceOrFeeLookup).contains(behavior)) }
      else if (bump and pending and behavior != RefuseBroadcast) { ...current,
        current: hash, known: current.known.union(Set(hash)), bumps: current.bumps + 1,
        pending: behavior != RespondNormally }
      else if (bump and not(pending)) { ...current, bumps: 2 }
      else current
    all {
      due(current, state.now),
      state.transactions.length() < 10,
      command' = Process({ job: job, behavior: behavior }),
      state' = { ...state, jobs: state.jobs.replaceAt(job, if (sending or (behavior == InterruptBeforeSend and (preparing or retry)) or (behavior == InterruptAcceptedSend and accepted)) updated else {
        ...updated, nextAttemptAt: if (terminal(updated)) -1 else state.now + 1 }),
        transactions: if (accepted) {
          val acceptedHash = if (retry) current.current else hash
          preparedTransactions.replaceAt(acceptedHash - 1,
            { ...transaction(preparedTransactions, acceptedHash), outcome: Pending })
        } else preparedTransactions },
    }
  }

  action pollIdle(behavior: RpcBehavior): bool = all {
    state.jobs.indices().forall(job => not(due(state.jobs[job], state.now))),
    command' = PollIdle({ behavior: behavior }),
    state' = state,
  }

  action step = {
    nondet job = Set(0, 1).oneOf()
    nondet behavior = Set(RespondNormally, RejectSendOrFailReceipt, LoseSendResponse,
      InterruptAcceptedSend, FailNonceOrFeeLookup, RefuseBroadcast, TransportUnavailable, InterruptBeforeSend).oneOf()
    nondet hash = 1.to(10).oneOf()
    nondet success = Set(false, true).oneOf()
    any { enqueue(job), process(job, behavior), pollIdle(behavior), tick, restart, mine(hash, success), drop(hash) }
  }

  val safety = all {
    state.transactions.indices().forall(index =>
      state.transactions[index].outcome == Prepared or
      state.jobs[state.transactions[index].job].known.contains(state.transactions[index].hash)),
    state.jobs.indices().forall(job => state.jobs[job].status != "confirmed" or
      transaction(state.transactions, state.jobs[job].current).outcome == Succeeded),
    state.transactions.indices().forall(first => state.transactions.indices().forall(second =>
      state.transactions[first].nonce != state.transactions[second].nonce or
      state.transactions[first].job == state.transactions[second].job or
      Set(Prepared, Dropped).contains(state.transactions[first].outcome) or
      Set(Prepared, Dropped).contains(state.transactions[second].outcome))),
  }

  run receiptRpcFailureTest = init.then(enqueue(0)).then(process(0, RejectSendOrFailReceipt))
    .then(tick).then(process(0, RespondNormally)).then(tick).then(process(0, RejectSendOrFailReceipt))
    .expect(state.jobs[0].status == "submitted")

  run oldReplacementMinedTest = init.then(enqueue(0)).then(process(0, RespondNormally))
    .then(tick).then(tick).then(process(0, RespondNormally))
    .then(tick).then(tick).then(process(0, RespondNormally))
    .then(mine(1, true)).then(tick).then(process(0, RespondNormally))
    .expect(state.jobs[0].status == "confirmed")

  run ambiguousSendRestartTest = init.then(enqueue(0)).then(process(0, InterruptAcceptedSend))
    .then(restart).then(enqueue(1)).then(process(1, RespondNormally)).then(process(0, RespondNormally))
    .then(mine(1, true)).then(mine(2, true))
    .then(tick).then(process(0, RespondNormally)).expect(safety)

  run nonceFailureBackoffTest = init.then(enqueue(0))
    .then(process(0, FailNonceOrFeeLookup))
    .expect(all { state.jobs[0].status == "queued", state.jobs[0].attempts == 1,
      state.jobs[0].nextAttemptAt == 1 })
    .then(pollIdle(FailNonceOrFeeLookup)).then(pollIdle(RespondNormally))
    .then(tick).then(process(0, FailNonceOrFeeLookup))
    .expect(all { state.jobs[0].status == "failed", state.jobs[0].attempts == 2 })
    .then(pollIdle(FailNonceOrFeeLookup))

  run nonceFailureRecoveryTest = init.then(enqueue(0))
    .then(process(0, FailNonceOrFeeLookup)).then(pollIdle(RespondNormally))
    .then(tick).then(process(0, RespondNormally)).then(mine(1, true))
    .then(tick).then(process(0, RespondNormally))
    .expect(state.jobs[0].status == "confirmed")

  run receiptAfterDeadlineTest = init.then(enqueue(0)).then(process(0, RespondNormally))
    .then(mine(1, true)).then(tick).then(tick).then(tick).then(tick)
    .then(tick).then(tick).then(tick).then(tick).then(restart)
    .then(process(0, RespondNormally))
    .expect(state.jobs[0].status == "confirmed")

  run receiptOutagePastDeadlineTest = init.then(enqueue(0)).then(process(0, RespondNormally))
    .then(tick).then(tick).then(tick).then(tick).then(tick).then(tick).then(tick).then(tick)
    .then(process(0, RejectSendOrFailReceipt))
    .expect(state.jobs[0].status == "submitted")
    .then(mine(1, false)).then(tick).then(process(0, RespondNormally))
    .expect(state.jobs[0].status == "failed")

  run replacementReceiptAfterDeadlineTest = init.then(enqueue(0)).then(process(0, RespondNormally))
    .then(tick).then(tick).then(process(0, RespondNormally)).then(mine(1, true))
    .then(tick).then(tick).then(tick).then(tick).then(tick).then(tick)
    .then(process(0, RespondNormally))
    .expect(all { state.jobs[0].status == "confirmed", state.jobs[0].current == 1 })

  run pendingNonceReservationTest = init.then(enqueue(0)).then(process(0, LoseSendResponse))
    .then(drop(1)).then(enqueue(1)).then(process(1, RespondNormally))
    .expect(state.jobs[1].nonce == 1)
    .then(tick).then(process(0, RespondNormally))
    .then(mine(1, true)).then(mine(2, true)).then(tick).then(process(0, RespondNormally))
    .expect(all { state.jobs[0].status == "confirmed", state.jobs[0].attempts == 1 })

  run nonceConsumedBeforeRebroadcastTest = init.then(enqueue(0)).then(process(0, RespondNormally))
    .then(drop(1)).then(enqueue(1)).then(process(1, LoseSendResponse)).then(mine(1, true))
    .then(tick).then(process(1, RespondNormally))
    .expect(all { state.jobs[1].status == "submitted", state.jobs[1].pending,
      transaction(state.transactions, 2).outcome == Dropped })

  run initialRefusalWithoutTimeoutTest = initWith(false).then(enqueue(0))
    .then(process(0, RefuseBroadcast)).then(pollIdle(RespondNormally))
    .expect(all { state.jobs[0].status == "queued", state.jobs[0].current == 0,
      state.jobs[0].nonce == -1, not(state.jobs[0].pending) })
    .then(tick).then(process(0, RefuseBroadcast))
    .expect(all { state.jobs[0].status == "failed", state.jobs[0].attempts == 2 })

  run refusalReleasesNonceTest = initWith(false).then(enqueue(0)).then(process(0, RefuseBroadcast))
    .then(enqueue(1)).then(process(1, RespondNormally))
    .expect(state.jobs[1].nonce == 0)
    .then(mine(2, true)).then(tick).then(process(1, RespondNormally))
    .expect(state.jobs[1].status == "confirmed")

  run replacementRefusalTest = initWith(false).then(enqueue(0)).then(process(0, RespondNormally))
    .then(tick).then(tick).then(process(0, RefuseBroadcast))
    .expect(all { state.jobs[0].current == 1, state.jobs[0].known == Set(1),
      not(state.jobs[0].pending), state.jobs[0].bumps == 0 })
    .then(mine(1, true)).then(tick).then(process(0, RespondNormally))
    .expect(state.jobs[0].status == "confirmed")

  run resendRefusalRemainsAmbiguousTest = initWith(false).then(enqueue(0))
    .then(process(0, LoseSendResponse)).then(restart).then(tick)
    .then(process(0, RefuseBroadcast))
    .expect(all { state.jobs[0].current == 1, state.jobs[0].pending,
      state.jobs[0].status == "submitted" })
    .then(mine(1, true)).then(tick).then(process(0, RespondNormally))
    .expect(state.jobs[0].status == "confirmed")

  run transportFailureRemainsAmbiguousTest = initWith(false).then(enqueue(0))
    .then(process(0, TransportUnavailable)).then(tick).then(process(0, RefuseBroadcast))
    .expect(all { state.jobs[0].pending, state.jobs[0].current == 1 })
    .then(tick).then(process(0, RespondNormally)).then(mine(1, true))
    .then(tick).then(process(0, RespondNormally)).expect(state.jobs[0].status == "confirmed")

  run crashBeforeInitialSendTest = initWith(false).then(enqueue(0))
    .then(process(0, InterruptBeforeSend)).then(restart)
    .expect(all { state.jobs[0].current == 1, state.jobs[0].pending,
      transaction(state.transactions, 1).outcome == Prepared })
    .then(process(0, RespondNormally)).then(mine(1, true)).then(tick)
    .then(process(0, RespondNormally)).expect(state.jobs[0].status == "confirmed")

  run crashBeforeReplacementSendTest = initWith(false).then(enqueue(0)).then(process(0, RespondNormally))
    .then(tick).then(tick).then(process(0, InterruptBeforeSend)).then(restart)
    .expect(all { state.jobs[0].current == 2, state.jobs[0].known == Set(1, 2), state.jobs[0].pending })
    .then(process(0, RespondNormally)).then(mine(2, true)).then(tick)
    .then(process(0, RespondNormally)).expect(all { state.jobs[0].status == "confirmed", state.jobs[0].bumps == 1 })

  run consumedNoncePreventsInterruptionTest = initWith(false).then(enqueue(0)).then(process(0, RespondNormally))
    .then(drop(1)).then(enqueue(1)).then(process(1, LoseSendResponse)).then(mine(1, true))
    .then(tick).then(process(1, InterruptAcceptedSend))
    .expect(all { state.jobs[1].status == "submitted", state.jobs[1].pending,
      state.jobs[1].nextAttemptAt == state.now + 1,
      transaction(state.transactions, 2).outcome == Dropped })
}
