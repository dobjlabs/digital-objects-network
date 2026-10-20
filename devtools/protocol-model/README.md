# Store and orchestrator handshake

This crate models the handshake between the **store**, which owns local objects, and the **orchestrator**, which runs actions: the two processes that today's daemon, `dobjd`, is to be split into. The name `dobjd` is retired with the split. It is a [Stateright](https://github.com/stateright/stateright) model written as communicating actors. The model is the design for the split: the implementation follows it, and nothing in production implements it yet.

This phase covers the store and the orchestrator only. What happens after submission (the chain, the synchronizer, observations, finality, and how a pending transaction's protection is eventually released) belongs to the next phase. An earlier model of that side is in this directory's history.

## The two processes

The **store** owns object contents and their status. It answers the orchestrator's requests and follows the synchronizer, and it never starts an action. It applies updates one at a time, never in parallel. An update is an orchestrator's request, a lease expiring, or information received from the synchronizer, and requests wait in a queue until the store processes them. The store does not hold anything locked while it waits for the synchronizer: it fetches first, then applies what it received as one update. Every decision about a reservation or a pending transaction is therefore atomic with respect to every other. This phase models the orchestrator's requests and lease expiry; updates from the synchronizer come in the next phase.

The **orchestrator** runs actions. For each one, it reserves the inputs, generates the proof, keeps the resulting payload in a durable journal, asks the store to mark the transaction pending, and submits it to the relayer. There is expected to be one orchestrator, managing its own resources and running many actions in parallel, such as several proofs and transactions at once. Nothing relies on that: the store tells requests apart by token, not by who sent them, so a second orchestrator would not break the protocol. In the model, each orchestrator actor runs one action, so two competing actors can equally be two actions of the same orchestrator.

## The handshake

**Reserve.** The orchestrator asks for exclusive use of the action's inputs. If none of them is already reserved or held by a pending transaction, the store grants a token with a lease; otherwise it refuses. A token names one reservation rather than one orchestrator, so a delayed request from an earlier reservation cannot be mistaken for a later one.

**Prove and journal.** While it holds the lease, the orchestrator proves the action and renews the lease. Before asking the store to mark the transaction pending, it saves the payload, and the token it holds, in its journal, so a restarted orchestrator resumes from there instead of proving again.

**Mark pending.** The orchestrator sends its token and the contents of the objects the transaction creates. The store first checks that the reservation is still active, so that the inputs are still held for this transaction. It then saves the transaction as pending together with the new objects, in one step, and ends the lease. From then on the pending transaction keeps the inputs reserved, with no lease left to expire, and the store tracks the commitments of the new objects. This is the same pending status the daemon already gives an action's outputs while it waits for confirmation. Only after saving does the store acknowledge.

The store must hold a copy of the new objects before the transaction is submitted, because losing object data is catastrophic: there must never be a live object for which nobody holds a full copy of its state. Once submitted, the transaction can only succeed on chain, and the store learns that it has succeeded only when it sees the new objects' commitments through the synchronizer.

**Submit.** The orchestrator submits its payload to the relayer only after receiving the acknowledgement.

**Give up.** Until it receives the acknowledgement, an orchestrator may abandon the action and release its token. If it has already sent the request to mark the transaction pending, the release and that request can reach the store in either order. If the release arrives first, the reservation ends and the inputs are free at once. If the request arrives first, the store refuses the release, and the inputs stay reserved until the transaction either lands or times out, which happens once its grounding is too old for the chain to accept it. Observing either outcome belongs to the next phase.

## Failures and recovery

Messages can be delayed, reordered, or lost, and either process can crash. When the orchestrator sends the store a request and gets no reply within its timeout, for example an HTTP request that times out, it cannot tell whether the store never received the request, received it but crashed before processing it, or processed it and the reply was lost. Recovery never relies on guessing which. This is a different kind of timeout from a lease expiring, which the store alone decides.

**Idempotent requests.** Marking a transaction pending and releasing a reservation are idempotent: sending the same request again gets the same answer and changes nothing further. An orchestrator that did not hear back therefore simply sends the request again, which is also how it learns the outcome. If the store already saved the transaction as pending, it acknowledges again. Renewing is safe to repeat as well. Reserving is not idempotent: repeating it asks for a new reservation, and a reservation the orchestrator does not use simply expires.

**Store restart.** The store saves pending transactions but not reservations. A restart ends every reservation whose transaction was not yet pending, and tokens issued before the restart are never issued again. An orchestrator whose reservation disappeared has its request rejected, reserves again, and asks again to mark pending the payload it already saved. This costs some repeated work after a restart, but the only durable state is the pending transactions, and nothing has to reason about clocks across restarts. It is safe because a token from before the restart can no longer mark a transaction pending: another action may take the inputs in the meantime, but the two transactions are never both acknowledged.

**Orchestrator restart.** The journal supplies the saved payload and its token. The orchestrator repeats its outstanding request: a reservation if it had not yet saved a proof, otherwise the request to mark its transaction pending.

## What the model checks

The protocol and its guarantees are in [src/handshake.rs](src/handshake.rs), with no assumptions about how many participants there are or what they request. The store's and the orchestrator's rules are traits whose default methods are the protocol. The guarantees are its safety properties, each a documented function. They are judged from a record of the messages exchanged, not from the store's own rules, so a wrong rule shows up as a violation instead of being checked against itself. Each scenario must also reach a submission, so a store that refused everything would not pass.

The tests in [tests/handshake.rs](tests/handshake.rs) also break one rule at a time, as a store or orchestrator with that rule replaced, and require a counterexample to the guarantee the rule protects. This shows that each guarantee can actually fail. A few hand-written traces walk through the recovery cases described above.

## What the bounds cover

Checking every interleaving of two competing actions together with crashes of every participant is too large, so the model is checked in three scenarios, defined separately in [src/scenarios.rs](src/scenarios.rs): competition for a shared input with renewals and giving up, crash and restart of the store and one orchestrator, and competition across a store restart. Each participant restarts at most once, and the store grants a bounded number of leases between restarts.

Some things are deliberately outside the model:

- Time is abstract. Lease expiry, renewal, and proving finish in any order, so the checks hold for any durations, but nothing is said about which durations work well.
- The network delays and reorders messages without bound but never drops them. A lost message is one that is never delivered, and the participants cannot tell that from one still in flight, so dropping would add states without adding behaviour the guarantees can see.
- The checks show that nothing bad happens and that submission is possible. They do not show that an action always eventually completes.
- A real store issues random tokens; the model gives each restart a new epoch instead. Either way a token is never issued twice, and that is one of the guarantees checked.
- A request that reuses a token with a different payload must be rejected. The model never sends one, so this is left to tests of the implementation.
- Everything after submission, as described at the top.

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

The implementation follows the model's shape. The messages are the store's API, the store applying one update at a time is a single-threaded writer, and what the store saves rather than keeps in memory is the persistence boundary. The model assumes each step is atomic and saved before the reply goes out; the implementation has to provide that. The two have not yet been checked against each other.
