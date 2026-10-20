# Store and orchestrator handshake

This crate models the handshake between the two processes that `dobjd` is to be split into: a **store**, which keeps the name `dobjd` and owns local objects, and an **orchestrator**, which runs actions. It is a [Stateright](https://github.com/stateright/stateright) model written as communicating actors. The model is the design for the split: the implementation follows it, and nothing in production implements it yet.

This phase covers the store and the orchestrator only. What happens after submission (the chain, the synchronizer, observations, finality, and how a preparation's protection is eventually released) belongs to the next phase. An earlier model of that side is in this directory's history.

## The two processes

The **store** owns object contents and their status. It answers requests and never starts an action. It handles one request at a time, so every decision about a reservation or a preparation is atomic with respect to every other.

The **orchestrator** runs an action: it reserves the inputs, generates the proof, keeps the resulting payload in a durable journal, asks the store to prepare, and submits. Several orchestrators can share one store.

## The handshake

**Reserve.** The orchestrator asks for exclusive use of the action's inputs. If none of them is reserved or prepared, the store grants a token with a lease; otherwise it refuses. A token names one reservation rather than one orchestrator, so a delayed request from an earlier reservation cannot be mistaken for a later one.

**Prove and journal.** While it holds the lease, the orchestrator proves the action and renews the lease. Before asking for preparation it saves the payload, and the token it will prepare under, in its journal, so a restarted orchestrator resumes from there instead of proving again.

**Prepare.** The orchestrator sends its token and the contents of the objects the action creates. If the lease is still active, the store saves the preparation and those contents in one step and ends the lease. From then on the inputs are protected by the preparation itself, with no lease left to expire. Only after saving does the store acknowledge.

**Submit.** The orchestrator submits its payload only after receiving the acknowledgement.

**Give up.** Before the acknowledgement, an orchestrator may abandon the action and release its token. If the release reaches the store first, the inputs are free at once. If the store has already prepared the attempt, it refuses the release, and the inputs stay protected by an attempt nobody will submit until that attempt is resolved in the next phase.

## Failures and recovery

Messages can be delayed, reordered, or lost, and either process can crash. A timeout therefore says nothing about whether a request succeeded, and recovery never relies on guessing.

**Repeated preparation.** Repeating a preparation request returns the recorded result. An orchestrator that did not hear back asks again: if the store saved the preparation, it acknowledges again; if not, it decides afresh. A retry is also how an orchestrator learns the outcome.

**Store restart.** The store saves preparations but not reservations. A restart ends every lease that had not been prepared, and tokens issued before the restart are never issued again. An orchestrator whose lease disappeared has its preparation rejected, reserves again, and prepares the payload it already saved. This costs some repeated work after a restart, but the only durable state is the preparations, and nothing has to reason about clocks across restarts. It is safe because a token from before the restart can no longer prepare: another orchestrator may take the inputs in the meantime, but the two attempts are never both acknowledged.

**Orchestrator restart.** The journal supplies the saved payload and its token. The orchestrator repeats its outstanding request: a reservation if it had not yet saved a proof, otherwise a preparation.

## What the model checks

The guarantees are the safety properties in [src/handshake.rs](src/handshake.rs), each a documented function. They are judged from a record of the messages exchanged, not from the store's own rules, so a wrong rule shows up as a violation instead of being checked against itself. Each scenario must also reach a submission, so a store that refused everything would not pass.

The tests in [tests/handshake.rs](tests/handshake.rs) also break one rule at a time in one participant and require a counterexample to the guarantee that rule protects, which shows that each guarantee can actually fail. A few hand-written traces walk through the recovery cases described above.

## What the bounds cover

Checking every interleaving of two competing orchestrators together with crashes of every participant is too large, so the model has three scenarios, described on the `Scenario` type: competition for a shared input with renewals and giving up, crash and restart of the store and one orchestrator, and competition across a store restart. Each participant restarts at most once, and the store grants a bounded number of leases between restarts.

Some things are deliberately outside the model:

- Time is abstract. Lease expiry, renewal, and proving finish in any order, so the checks hold for any durations, but nothing is said about which durations work well.
- The checks show that nothing bad happens and that submission is possible. They do not show that an action always eventually completes.
- A real store issues random tokens; the model gives each restart a new epoch instead, which provides the same uniqueness.
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

The implementation follows the model's shape. The messages are the store's API, the store handling one message at a time is a single-threaded writer, and what the store saves rather than keeps in memory is the persistence boundary. The model assumes each step is atomic and saved before the reply goes out; the implementation has to provide that. The two have not yet been checked against each other.
