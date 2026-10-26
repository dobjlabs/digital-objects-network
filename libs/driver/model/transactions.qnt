// Safety model for the driver's transaction lifecycle. It follows local files,
// proof jobs, relayer jobs, and chain state through normal operation and crashes.
// Cryptographic proofs and HTTP requests are abstracted into guarded actions.
//
// Reading the model:
// - A pure definition computes from its arguments; a val may read current state.
// - An action describes one transition. Unprimed names are current values;
//   primed names, such as node', are values after that transition.
// - all requires every guard and update to hold together. any permits one of
//   its enabled actions. A false guard disables a transition.
// - nondet explores choices of inputs, outputs, commands, and crash points.
module transactions {
  // State representation
  //
  // Local object files record chain knowledge: Unknown has not been confirmed,
  // Live has been created, and Nullified has been spent. Pending submission is
  // tracked on the transaction record rather than stored as an object status.
  type ObjectStatus = Unknown | Live | Nullified
  // tx is the producing transaction's ID; 0 denotes a genesis object.
  type ObjectFile = { status: ObjectStatus, tx: int }
  // Recorded and Submitted keep their inputs unavailable to other proofs.
  // Landed and Expired end that ownership and cannot change to another state.
  type RecordState = Recorded | Submitted | Landed | Expired
  type Record = {
    inputs: Set[int],
    outputs: Set[int],
    grounding: int,       // Block of the state root used to prove the transaction.
    state: RecordState,
    broadcast: bool,      // Latest broadcast information copied from the relayer.
    relayerFailed: bool,  // A reported failure; it does not decide chain inclusion.
  }
  // A proof job fixes its inputs, proposed outputs, and grounding at startup.
  // Its ID becomes the transaction ID if recording succeeds.
  type Job = { inputs: Set[int], outputs: Set[int], grounding: int }

  // Everything owned by one driver. Objects, payloads, and records survive a
  // restart; holds and jobs exist only in memory. Payload bytes are represented
  // by the set of transaction IDs whose payloads have been written.
  type Node = {
    objects: int -> ObjectFile,
    payloads: Set[int],
    records: int -> Record,
    holds: int -> int,    // Input object ID -> proof job holding that input.
    jobs: int -> Job,
    nextId: int,
  }
  // The chain and relayer are independent of the driver. The chain remembers
  // created objects and spent inputs even while local files are stale.
  type Chain = { head: int, created: Set[int], nullifiers: Set[int], landed: Set[int] }
  type RelayerJob = {
    inputs: Set[int],
    outputs: Set[int],
    grounding: int,
    broadcast: bool,
    failed: bool,
  }
  // The output batch, payload write, and record write are modeled as atomic.
  // Crashes can occur between them; torn files and partial output batches are
  // outside the model.
  type CrashPhase = OutputsWritten | PayloadWritten | RecordWritten | RelayerAccepted

  // Save the selected command in each state so counterexample traces show what
  // happened, including the IDs and crash phase involved.
  type Command = Init
    | StartProof({ id: int, inputs: Set[int], outputs: Set[int] })
    | FinishProof({ id: int })
    | Crash({ id: int, phase: CrashPhase })
    | Delete({ tx: int })
    | Submit({ tx: int })
    | RelayerBroadcast({ tx: int })
    | RelayerFail({ tx: int })
    | ChainLand({ tx: int })
    | ChainAdvance
    | Settle
    | Restart

  // Exploration bounds
  //
  // Small finite sets let short traces cover collisions, expiry, and crashes.
  // The age limit stands in for the production grounding-age rule. A grounding
  // exactly maxAge blocks old is still valid; only a greater age expires it.
  pure val maxAge = 2
  pure val lastBlock = 8
  pure val maxProofs = 4  // Total proof starts allowed in a trace, not a concurrency limit.
  // One integer represents an object's filename, commitment, and nullifier.
  // This removes hashing from the model while preserving ownership conflicts.
  pure val objectIds = 1.to(4)
  pure val genesisObjects = Set(1, 2)

  // Mutable state and history used by safety checks
  //
  // Driver actions may leave local knowledge behind the chain. Settlement is
  // the action that brings those views together.
  var node: Node
  var chain: Chain
  var relayer: int -> RelayerJob
  var command: Command
  // These history variables are for checking properties, not driver storage.
  var deleted: Set[int]                     // Transactions explicitly abandoned.
  var strays: Set[int]                      // Output names left by a crash before a record exists.
  var previousObjects: int -> ObjectFile    // File view before the most recent action.

  // Derived views and updates
  //
  // These definitions express the ownership rules used by the actions below.

  pure def isOpen(state: RecordState): bool = state == Recorded or state == Submitted

  // Removing keys creates a new map; entries outside the removal set survive.
  pure def without(entries: int -> a, removed: Set[int]): int -> a =
    entries.keys().exclude(removed).mapBy(key => entries.get(key))

  pure def writeOutputs(objects: int -> ObjectFile, outputs: Set[int], tx: int): int -> ObjectFile =
    objects.keys().union(outputs).mapBy(basename =>
      if (outputs.contains(basename)) { status: Unknown, tx: tx } else objects.get(basename))

  // Delete only unconfirmed files still owned by this transaction. An output
  // name may later be reused, so matching the filename alone would be unsafe.
  pure def ownedUnknownOutputs(objects: int -> ObjectFile, record: Record, tx: int): Set[int] =
    record.outputs.filter(basename =>
      objects.keys().contains(basename) and objects.get(basename) == { status: Unknown, tx: tx })

  // Recording transfers ownership from the in-memory job into a durable record.
  // Relayer annotations start empty because no acceptance has been recorded.
  pure def newRecord(job: Job): Record = {
    inputs: job.inputs,
    outputs: job.outputs,
    grounding: job.grounding,
    state: Recorded,
    broadcast: false,
    relayerFailed: false,
  }

  // Give file statuses an order so we can detect local knowledge moving back.
  pure def rank(status: ObjectStatus): int = match status {
    | Unknown => 0
    | Live => 1
    | Nullified => 2
  }

  // An input is available only if its local file is Live and neither a proof
  // nor an open transaction owns it. Submission does not release an input.
  val openRecords = node.records.keys().filter(tx => isOpen(node.records.get(tx).state))
  val lockedInputs = openRecords.map(tx => node.records.get(tx).inputs).flatten()
  val freeLiveObjects = node.objects.keys()
    .filter(basename => node.objects.get(basename).status == Live)
    .exclude(lockedInputs)
    .exclude(node.holds.keys())

  // Recording rechecks collisions after proving: another job may have written
  // one of the proposed output names in the meantime. These checks and the
  // successful writes form one atomic step, representing the store lock.
  def recordRefused(id: int): bool = {
    val job = node.jobs.get(id)
    or {
      job.outputs.exists(basename => node.objects.keys().contains(basename)),
      job.inputs.intersect(lockedInputs).size() > 0,
    }
  }

  // A job releases its holds whether recording succeeds or is refused.
  pure def released(n: Node, id: int): Node = {
    val job = n.jobs.get(id)
    { ...n, holds: without(n.holds, job.inputs), jobs: without(n.jobs, Set(id)) }
  }

  // Each action must assign every state variable. These helpers preserve the
  // environment and deletion/crash history. Actions save the previous file view
  // separately so status changes can be checked after every step.
  action unchangedEnvironment = all { chain' = chain, relayer' = relayer }
  action unchangedGhosts = all { deleted' = deleted, strays' = strays }

  // Initial state
  //
  // Start with two confirmed, unspent objects and no proof or transaction jobs.
  // The genesis files have producing transaction ID 0 and need no local record.
  action init = all {
    command' = Init,
    node' = {
      objects: genesisObjects.mapBy(basename => { status: Live, tx: 0 }),
      payloads: Set(),
      records: Map(),
      holds: Map(),
      jobs: Map(),
      nextId: 1,
    },
    chain' = { head: 0, created: genesisObjects, nullifiers: Set(), landed: Set() },
    relayer' = Map(),
    deleted' = Set(),
    strays' = Set(),
    previousObjects' = genesisObjects.mapBy(basename => { status: Live, tx: 0 }),
  }

  // Proving and recording
  //
  // Starting a proof claims available inputs and fixes the grounding at the
  // current head. Witness retrieval is folded into this step. Proposed output
  // names are not reserved, so separate jobs can later collide when recording.
  action startProofWith(id: int, inputs: Set[int], outputs: Set[int]): bool = all {
    id == node.nextId,
    id <= maxProofs,
    inputs.size() > 0,
    inputs.subseteq(freeLiveObjects),
    outputs.size() > 0,
    outputs.intersect(inputs).size() == 0,
    command' = StartProof({ id: id, inputs: inputs, outputs: outputs }),
    node' = { ...node,
      holds: node.holds.keys().union(inputs).mapBy(basename =>
        if (inputs.contains(basename)) id else node.holds.get(basename)),
      jobs: node.jobs.put(id, { inputs: inputs, outputs: outputs, grounding: chain.head }),
      nextId: id + 1,
    },
    unchangedEnvironment,
    unchangedGhosts,
    previousObjects' = node.objects,
  }

  // Simulation chooses input and output subsets. Empty or otherwise invalid
  // choices are disabled by startProofWith's guards. Tests supply explicit sets.
  action startProof = {
    nondet inputs = freeLiveObjects.powerset().oneOf()
    nondet outputs = objectIds.exclude(inputs).powerset().oneOf()
    startProofWith(node.nextId, inputs, outputs)
  }

  // A completed proof either records its outputs, payload, and record together,
  // or loses a recording race and writes nothing. Both outcomes release holds.
  // Crashes between the individual writes are modeled separately below.
  // Proving may outlast the grounding's validity; recording still succeeds,
  // leaving submission or settlement to reject or expire the stale transaction.
  action finishProof(id: int): bool = all {
    node.jobs.keys().contains(id),
    command' = FinishProof({ id: id }),
    node' = {
      val job = node.jobs.get(id)
      val after = released(node, id)
      if (recordRefused(id)) after
      else { ...after,
        objects: writeOutputs(node.objects, job.outputs, id),
        payloads: node.payloads.union(Set(id)),
        records: node.records.put(id, newRecord(job)),
      }
    },
    unchangedEnvironment,
    unchangedGhosts,
    previousObjects' = node.objects,
  }

  // Crashes and recovery
  //
  // Build the disk state left after each completed write. The record is the
  // final commit point: outputs and payload alone do not create a transaction.
  // Losing the process clears all jobs and holds, including unrelated jobs.
  pure def afterCrash(n: Node, id: int, phase: CrashPhase): Node = {
    val job = n.jobs.get(id)
    val outputsWritten = { ...n,
      objects: writeOutputs(n.objects, job.outputs, id),
      holds: Map(),
      jobs: Map(),
    }
    val payloadWritten = { ...outputsWritten, payloads: n.payloads.union(Set(id)) }
    match phase {
      | OutputsWritten => outputsWritten
      | PayloadWritten => payloadWritten
      | RecordWritten => { ...payloadWritten, records: n.records.put(id, newRecord(job)) }
      | RelayerAccepted => n  // Handled by crash's separate submission branch.
    }
  }

  // Explore recording crashes and the uncertain submission window. In that
  // window the relayer has the payload while the local record still says Recorded;
  // later settlement must allow it to land without a locally stored job ID.
  action crash(id: int, phase: CrashPhase): bool = all {
    command' = Crash({ id: id, phase: phase }),
    match phase {
      | RelayerAccepted => all {
          node.records.keys().contains(id),
          node.records.get(id).state == Recorded,
          not(relayer.keys().contains(id)),
          chain.head - node.records.get(id).grounding <= maxAge,
          node' = { ...node, holds: Map(), jobs: Map() },
          relayer' = relayer.put(id, {
            inputs: node.records.get(id).inputs,
            outputs: node.records.get(id).outputs,
            grounding: node.records.get(id).grounding,
            broadcast: false,
            failed: false,
          }),
          strays' = strays,
        }
      | _ => all {
          node.jobs.keys().contains(id),
          not(recordRefused(id)),
          node' = afterCrash(node, id, phase),
          relayer' = relayer,
          // Only a crash before the record write leaves outputs without an owner.
          strays' = if (phase == RecordWritten) strays
            else strays.union(node.jobs.get(id).outputs),
        }
    },
    chain' = chain,
    deleted' = deleted,
    previousObjects' = node.objects,
  }

  // Deletion and submission
  //
  // Deletion frees the inputs and removes the transaction's unconfirmed outputs.
  // It is safe only if the relayer has no payload that could later land. The model
  // can inspect the relayer directly; the driver must conservatively track whether
  // a submission attempt could have reached it.
  action delete(tx: int): bool = all {
    node.records.keys().contains(tx),
    node.records.get(tx).state == Recorded,
    not(relayer.keys().contains(tx)),
    command' = Delete({ tx: tx }),
    node' = { ...node,
      objects: without(node.objects, ownedUnknownOutputs(node.objects, node.records.get(tx), tx)),
      payloads: node.payloads.exclude(Set(tx)),
      records: without(node.records, Set(tx)),
    },
    unchangedEnvironment,
    deleted' = deleted.union(Set(tx)),
    strays' = strays,
    previousObjects' = node.objects,
  }

  // Submission accepts only a still-valid Recorded transaction. Repeating a
  // request after a crash reuses the relayer's existing job, assuming submissions
  // are idempotent by transaction ID. This closes the uncertain submission window.
  action submit(tx: int): bool = all {
    node.records.keys().contains(tx),
    node.records.get(tx).state == Recorded,
    chain.head - node.records.get(tx).grounding <= maxAge,
    command' = Submit({ tx: tx }),
    node' = { ...node,
      records: node.records.set(tx, { ...node.records.get(tx), state: Submitted }),
    },
    relayer' = if (relayer.keys().contains(tx)) relayer else relayer.put(tx, {
      inputs: node.records.get(tx).inputs,
      outputs: node.records.get(tx).outputs,
      grounding: node.records.get(tx).grounding,
      broadcast: false,
      failed: false,
    }),
    chain' = chain,
    unchangedGhosts,
    previousObjects' = node.objects,
  }

  // Relayer and chain progress
  //
  // Relayer status changes independently of driver records. The driver observes
  // these flags only when it settles a Submitted record.
  action relayerBroadcast(tx: int): bool = all {
    relayer.keys().contains(tx),
    not(relayer.get(tx).broadcast),
    command' = RelayerBroadcast({ tx: tx }),
    relayer' = relayer.set(tx, { ...relayer.get(tx), broadcast: true }),
    node' = node,
    chain' = chain,
    unchangedGhosts,
    previousObjects' = node.objects,
  }

  // Failure records a relayer report without withdrawing the payload. A send
  // may already have happened, so a failed job remains eligible for landing.
  action relayerFail(tx: int): bool = all {
    relayer.keys().contains(tx),
    not(relayer.get(tx).failed),
    command' = RelayerFail({ tx: tx }),
    relayer' = relayer.set(tx, { ...relayer.get(tx), failed: true }),
    node' = node,
    chain' = chain,
    unchangedGhosts,
    previousObjects' = node.objects,
  }

  // Chain acceptance is the environmental safety assumption: grounding must
  // still be valid, inputs must not be spent, and outputs must not already exist.
  // Landing does not depend on the relayer's broadcast or failure flags; local
  // status must never be sufficient evidence that a payload cannot land.
  action chainLand(tx: int): bool = all {
    relayer.keys().contains(tx),
    not(chain.landed.contains(tx)),
    chain.head - relayer.get(tx).grounding <= maxAge,
    relayer.get(tx).inputs.intersect(chain.nullifiers).size() == 0,
    relayer.get(tx).outputs.intersect(chain.created).size() == 0,
    command' = ChainLand({ tx: tx }),
    chain' = { ...chain,
      created: chain.created.union(relayer.get(tx).outputs),
      nullifiers: chain.nullifiers.union(relayer.get(tx).inputs),
      landed: chain.landed.union(Set(tx)),
    },
    node' = node,
    relayer' = relayer,
    unchangedGhosts,
    previousObjects' = node.objects,
  }

  // Time can advance without a transaction landing. Stop at a bounded head
  // height so exploration stays small; early groundings can expire along the way.
  action chainAdvance = all {
    chain.head < lastBlock,
    command' = ChainAdvance,
    chain' = { ...chain, head: chain.head + 1 },
    node' = node,
    relayer' = relayer,
    unchangedGhosts,
    previousObjects' = node.objects,
  }

  // Settlement
  //
  // Terminal records stay terminal. For an open record, check chain inclusion
  // before age: a transaction that landed at the age limit remains Landed even
  // when settlement happens at a later head. Relayer flags are annotations only.
  pure def settleRecord(record: Record, c: Chain, broadcast: bool, failed: bool): Record =
    if (not(isOpen(record.state))) record
    else {
      val annotated = { ...record, broadcast: broadcast, relayerFailed: failed }
      if (record.outputs.subseteq(c.created) and record.inputs.subseteq(c.nullifiers))
        { ...annotated, state: Landed }
      else if (c.head - record.grounding > maxAge)
        { ...annotated, state: Expired }
      else annotated
    }

  // One pass uses a single chain snapshot for membership and age checks, then
  // updates records and local files together. Splitting those reads could falsely
  // expire a transaction that landed between them.
  action settle = all {
    command' = Settle,
    node' = {
      val settledRecords = node.records.keys().mapBy(tx => {
        val record = node.records.get(tx)
        // Recorded transactions still receive membership checks, including after
        // a crash that lost the relayer job ID. Relayer polling is Submitted-only.
        val atRelayer = record.state == Submitted and relayer.keys().contains(tx)
        settleRecord(record, chain,
          if (atRelayer) relayer.get(tx).broadcast else record.broadcast,
          if (atRelayer) relayer.get(tx).failed else record.relayerFailed)
      })
      // Remove expired outputs only while their files still belong to that record.
      val expiredOutputs = settledRecords.keys()
        .filter(tx => settledRecords.get(tx).state == Expired)
        .map(tx => ownedUnknownOutputs(node.objects, settledRecords.get(tx), tx))
        .flatten()
      val reconciled = node.objects.keys().exclude(expiredOutputs).mapBy(basename => {
        val file = node.objects.get(basename)
        // A spent object is also in the created set, so spending takes precedence.
        if (chain.nullifiers.contains(basename)) { ...file, status: Nullified }
        else if (chain.created.contains(basename)) { ...file, status: Live }
        else file
      })
      { ...node, objects: reconciled, records: settledRecords }
    },
    unchangedEnvironment,
    unchangedGhosts,
    previousObjects' = node.objects,
  }

  // Restart preserves disk and the environment but discards proof jobs and
  // holds. Persisted transactions remain available for submission and settlement.
  action restart = all {
    command' = Restart,
    node' = { ...node, holds: Map(), jobs: Map() },
    unchangedEnvironment,
    unchangedGhosts,
    previousObjects' = node.objects,
  }

  // Exploration
  //
  // Pick one enabled driver or environment action at each step. There is no
  // fairness requirement: the relayer or chain may stop making progress. The
  // properties below therefore check safety, not eventual completion.
  action step = {
    nondet id = 1.to(maxProofs).oneOf()
    nondet phase = Set(OutputsWritten, PayloadWritten, RecordWritten, RelayerAccepted).oneOf()
    any {
      startProof,
      finishProof(id),
      crash(id, phase),
      delete(id),
      submit(id),
      relayerBroadcast(id),
      relayerFail(id),
      chainLand(id),
      chainAdvance,
      settle,
      restart,
    }
  }

  // Safety properties
  //
  // Each property must hold after initialization and every enabled action.
  // Together they check exclusive ownership, persistence, chain agreement,
  // cleanup, and the allowed exception for files left by an interrupted record.

  // Two open transactions can never own the same input, even if one has not
  // been submitted yet or has received a relayer failure report.
  val disjointOpenInputs = openRecords.forall(first => openRecords.forall(second =>
    first == second or
      node.records.get(first).inputs.intersect(node.records.get(second).inputs).size() == 0))

  // Ownership must transfer from the proof's holds to its record without
  // allowing another proof to acquire an input owned by that record.
  val holdsExcludeOpenInputs = node.holds.keys().intersect(lockedInputs).size() == 0

  // Every held input must still have a local file marked Live.
  val holdsAreLive = node.holds.keys().forall(basename =>
    node.objects.keys().contains(basename) and node.objects.get(basename).status == Live)

  // A committed record always has its payload. The converse is deliberately
  // false: a crash can leave a payload without completing the record write.
  val recordsHavePayloads = node.records.keys().subseteq(node.payloads)

  // A terminal decision must agree with actual inclusion. Open records may
  // lag the chain until settlement observes them, but Expired can never land.
  val recordsMatchChain = node.records.keys().forall(tx => {
    val record = node.records.get(tx)
    match record.state {
      | Landed => chain.landed.contains(tx)
      | Expired => not(chain.landed.contains(tx))
      | _ => true
    }
  })

  // Declaring a transaction Landed must also reconcile its files. Its outputs
  // may later be spent, so they can be Live or Nullified; its inputs are spent.
  val landedRecordsSettled = node.records.keys().forall(tx => {
    val record = node.records.get(tx)
    record.state != Landed or all {
      record.outputs.forall(basename =>
        node.objects.keys().contains(basename) and
          node.objects.get(basename).tx == tx and
          node.objects.get(basename).status != Unknown),
      record.inputs.forall(basename =>
        node.objects.keys().contains(basename) and
          node.objects.get(basename).status == Nullified),
    }
  })

  // An expired transaction retains no unconfirmed output file of its own.
  // A later transaction may reuse a removed filename without violating this rule.
  val expiredOutputsRemoved = node.records.keys().forall(tx =>
    node.records.get(tx).state != Expired or
      ownedUnknownOutputs(node.objects, node.records.get(tx), tx).size() == 0)

  // Normally every Unknown file has an open producing record. Crash leftovers
  // are the explicit exception; they confer no input ownership.
  val unknownFilesAreOwned = node.objects.keys().forall(basename =>
    node.objects.get(basename).status != Unknown or
      openRecords.contains(node.objects.get(basename).tx) or
      strays.contains(basename))

  // Local knowledge may lag the chain, but a positive claim needs evidence:
  // Live requires creation and Nullified requires a recorded spend on the chain.
  val statusesFollowChain = node.objects.keys().forall(basename =>
    match node.objects.get(basename).status {
      | Unknown => true
      | Live => chain.created.contains(basename)
      | Nullified => chain.nullifiers.contains(basename)
    })

  // A file present across an action cannot move backward in status or change
  // producer. Removal followed by later recreation is checked as separate steps.
  val objectsMonotone = node.objects.keys().intersect(previousObjects.keys()).forall(basename => all {
    rank(previousObjects.get(basename).status) <= rank(node.objects.get(basename).status),
    previousObjects.get(basename).tx == node.objects.get(basename).tx,
  })

  // Explicit abandonment must never delete a payload the relayer can still land.
  val deletedNeverAtRelayer = deleted.intersect(relayer.keys()).size() == 0

  // Single entry point for the simulator's invariant checks and scenario tests.
  val safety = all {
    disjointOpenInputs,
    holdsExcludeOpenInputs,
    holdsAreLive,
    recordsHavePayloads,
    recordsMatchChain,
    landedRecordsSettled,
    expiredOutputsRemoved,
    unknownFilesAreOwned,
    statusesFollowChain,
    objectsMonotone,
    deletedNeverAtRelayer,
  }
}
