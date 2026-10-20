# Store and orchestrator handshake

This crate models the handshake between the **store**, which owns local objects, and the **orchestrator**, which runs actions: the two processes that today's daemon, `dobjd`, is to be split into. The name `dobjd` is retired with the split. It is a [Stateright](https://github.com/stateright/stateright) model written as communicating actors. The model is the design for the split: the implementation follows it, and nothing in production implements it yet.

This phase covers the store, the orchestrator, and the ending of a pending transaction on a verdict the model assumes to be settled. How the synchronizer arrives at that verdict (the chain, observations, finality, and reorgs) belongs to the next phase. An earlier model of that side is in this directory's history.

## The two processes

The **store** owns object contents and their status. It answers the orchestrator's requests and follows the synchronizer, and it never starts an action. It applies updates one at a time, never in parallel. An update is an orchestrator's request, a lease expiring, or information received from the synchronizer, and requests wait in a queue until the store processes them. The store does not hold anything locked while it waits for the synchronizer: it fetches first, then applies what it received as one update. Every decision about a reservation or a pending transaction is therefore atomic with respect to every other. This phase models the orchestrator's requests, lease expiry, and one kind of information from the synchronizer: the settlement of a pending transaction.

The **orchestrator** runs actions. For each one, it reserves the inputs, generates the proof, keeps the resulting payload in a durable journal, asks the store to mark the transaction pending, and submits it to the relayer. There is expected to be one orchestrator, managing its own resources and running many actions in parallel, such as several proofs and transactions at once. Nothing relies on that: the store tells requests apart by reservation, not by who sent them, so a second orchestrator would not break the protocol. In the model, each orchestrator actor runs one action, so two competing actors can equally be two actions of the same orchestrator.

## The handshake

**Reserve.** The orchestrator asks for exclusive use of the action's inputs. If none of them is already reserved, held by a pending transaction, or consumed by a settled one, the store grants a reservation with a lease; otherwise it refuses. Each reservation is identified on its own rather than by its orchestrator, so a delayed request from an earlier reservation cannot be mistaken for a later one.

**Prove and journal.** While it holds the lease, the orchestrator proves the action and renews the lease. Before asking the store to mark the transaction pending, it saves the payload, and the reservation it holds, in its journal, so a restarted orchestrator resumes from there instead of proving again.

**Mark pending.** The orchestrator sends its reservation and the contents of the objects the transaction creates. The store first checks that the reservation is still active, so that the inputs are still held for this transaction. It then saves the transaction as pending together with the new objects, in one step, and ends the lease. From then on the pending transaction keeps the inputs reserved, with no lease left to expire, and the store tracks the commitments of the new objects. This is the same pending status the daemon already gives an action's outputs while it waits for confirmation. Only after saving does the store acknowledge.

The store must hold a copy of the new objects before the transaction is submitted, because losing object data is catastrophic: there must never be a live object for which nobody holds a full copy of its state. Once submitted, the transaction can only succeed on chain, and the store learns that it has succeeded only when it sees the new objects' commitments through the synchronizer.

**Submit.** The orchestrator submits its payload to the relayer only after receiving the acknowledgement.

**Give up.** Until it receives the acknowledgement, an orchestrator may abandon the action and release its reservation. If it has already sent the request to mark the transaction pending, the release and that request can reach the store in either order. If the release arrives first, the reservation ends and the inputs are free at once. If the request arrives first, the store refuses the release, and the inputs stay reserved until the synchronizer settles the transaction, as described next.

## The ending

**Settle.** Once a transaction is pending, the store watches the chain for its effects through the synchronizer: the nullifiers of its inputs and the commitments of its outputs. The chain applies a transaction whole or not at all, so what the store eventually sees is one of three verdicts. All of the effects are present: the transaction landed. Some but not all are present: another transaction consumed one of the inputs or created one of the outputs first, and this one can never land, because applying it would now collide. None are present once the chain has moved past the transaction's grounding window: it timed out and can never land either. The model abstracts this to a single settlement from the synchronizer, carrying the inputs consumed and the outputs created, and assumes it is right and final. What it takes for the real synchronizer to deliver such a verdict is the next phase.

The store applies a settlement as one update, saved before anything else happens, and keeps the transaction's record with its verdict. Consumed inputs are nullified: no reservation will ever cover them again. Unconsumed inputs are free, and the outputs are live if they were created and dead otherwise. A request on a settled transaction, whether to mark it pending again or to release it, is answered `Ended`. An orchestrator that never heard the acknowledgement and asks again therefore learns that the transaction is over and does not submit its payload. An acknowledgement already in flight when the settlement arrives still reaches the orchestrator, which then submits; the chain will not accept that payload, which is harmless exactly because the verdict is settled.

## Failures and recovery

Messages can be delayed, reordered, or lost, and either process can crash. When the orchestrator sends the store a request and gets no reply within its timeout, for example an HTTP request that times out, it cannot tell whether the store never received the request, received it but crashed before processing it, or processed it and the reply was lost. Recovery never relies on guessing which. This is a different kind of timeout from a lease expiring, which the store alone decides.

**Idempotent requests.** Marking a transaction pending and releasing a reservation are idempotent: sending the same request again gets the same answer and changes nothing further. An orchestrator that did not hear back therefore simply sends the request again, which is also how it learns the outcome. If the store already saved the transaction as pending, it acknowledges again, or answers `Ended` once the transaction is settled. Renewing is safe to repeat as well. Reserving is not idempotent: repeating it asks for a new reservation, and a reservation the orchestrator does not use simply expires.

**Store restart.** The store saves pending transactions and their settlements but not leases. A restart ends every reservation whose transaction was not yet pending, and a reservation granted before the restart is never granted again. An orchestrator whose reservation disappeared has its request rejected, reserves again, and asks again to mark pending the payload it already saved. This costs some repeated work after a restart, but the only durable state is the pending transactions with their verdicts, and nothing has to reason about clocks across restarts. The store does not have to ask the synchronizer again after a restart: in the model the stand-in keeps watching, which stands for the store resuming its polling of unsettled transactions. It is safe because a reservation from before the restart can no longer mark a transaction pending: another action may take the inputs in the meantime, but the two transactions are never both acknowledged.

**Orchestrator restart.** The journal supplies the saved payload and its reservation. The orchestrator repeats its outstanding request: a reservation if it had not yet saved a proof, otherwise the request to mark its transaction pending.

## What the model checks

The protocol and its guarantees are in [src/handshake.rs](src/handshake.rs), with no assumptions about how many participants there are or what they request. The store's and the orchestrator's rules are traits whose default methods are the protocol. The guarantees are its safety properties, each a documented function. They are judged from a record of the messages exchanged, not from the store's own rules, so a wrong rule shows up as a violation instead of being checked against itself. Each scenario must also reach a submission and a landing, so a store that refused everything would not pass.

The tests in [tests/handshake.rs](tests/handshake.rs) also break one rule at a time, as a store or orchestrator with that rule replaced, and require a counterexample to the guarantee the rule protects. This shows that each guarantee can actually fail. A few hand-written traces walk through the recovery cases described above.

## What the bounds cover

Checking every interleaving of two competing actions together with crashes of every participant is too large, so the model is checked in three scenarios, defined separately in [src/scenarios.rs](src/scenarios.rs): competition for a shared input with renewals and giving up, crash and restart of the store and one orchestrator, and competition across a store restart. Each participant restarts at most once, and the store grants a bounded number of leases between restarts. The synchronizer stand-in settles each transaction with every verdict its effects allow, at any point relative to everything else.

Some things are deliberately outside the model:

- Time is abstract. Lease expiry, renewal, and proving finish in any order, so the checks hold for any durations, but nothing is said about which durations work well.
- The network delays and reorders messages without bound but never drops them. A lost message is one that is never delivered, and the participants cannot tell that from one still in flight, so dropping would add states without adding behaviour the guarantees can see.
- The checks show that nothing bad happens and that submission is possible. They do not show that an action always eventually completes.
- A real store identifies reservations at random; the model gives each restart a new epoch instead. Either way a reservation is never granted twice, and that is one of the guarantees checked.
- A request that reuses a reservation with a different payload must be rejected. The model never sends one, so this is left to tests of the implementation.
- Each settlement is assumed right and final, and the model never revises one. The real synchronizer answers at its head, which a reorg or a re-index can revise, so deriving a verdict that will not change is the next phase's job. An implementation that settles on head observations in the meantime is exposed to exactly that.
- A settlement can arrive whether or not the orchestrator submitted, since the store cannot tell. The model over-approximates the environment here, which is safe for these checks, and no guarantee says that a landing implies a submission.
- The synchronizer stand-in never crashes.
- How a verdict is reached, as described at the top.

## Running

```sh
cargo test -p protocol-model --release --locked
```

`just test-protocol` runs the same. Add `-- --nocapture` to print each scenario's state count and the shortest counterexample for each broken rule.

To step through a scenario interactively, with each run drawn as a sequence diagram:

```sh
cargo run -p protocol-model --release --locked -- explore contention
```

Then open `http://127.0.0.1:3001`. The other scenarios are `recovery` and `store-restart`.

## Relation to the implementation

The implementation follows the model's shape. The messages are the store's API, the store applying one update at a time is a single-threaded writer, and what the store saves rather than keeps in memory is the persistence boundary. The model assumes each step is atomic and saved before the reply goes out; the implementation has to provide that. The settlement is the store's one input from the synchronizer: the implementation derives it from a single membership snapshot per transaction, never revises it, and keeps per-object reconciliation alongside so that a verdict a reorg overturns loses no object contents until finalized evidence replaces head observations. The two have not yet been checked against each other.
