# Store and orchestrator protocol model

This crate specifies and models the proposed `dobjd` store/orchestrator protocol
using Stateright. Production does not yet implement the split, leases, or finalized
observations specified here. The executable model and its limits are described in
the [model reference](#model-reference).

The **store**, which keeps the name `dobjd`, owns local object contents and status.
It follows the synchronizer and responds to reads and commands; it does not
initiate actions. The **orchestrator** executes actions, generates proofs, submits
transactions, and provides action/run/progress APIs. The **synchronizer** processes
chain data, checks transaction proofs and applicability, and reports object state.

The main sequence is: the orchestrator reserves inputs, executes and saves a
transaction locally, asks the store to prepare for the transaction, providing
the store with a full copy of the new objects created by the transaction, and
submits after acknowledgement. The store then reconciles observations and
eventually resolves input protection using finalized evidence. The sections
below follow that sequence.

## 1. Reserving the inputs

An **object version** is a particular object state, identified by a **commitment**.
Its creation adds that commitment to the chain's created set. Consuming the version
adds its associated **nullifier** to the nullifier set, preventing another
consumption. Creation and nullification are distinct kinds of transaction **effect**.
The chain records these effects; the store holds the actual object contents.

An object is **live** in a chain view when its creation is present and its nullifier
is absent. It is **available** locally when the store also holds its contents and
no reservation or prepared attempt protects it from another local action.
**External inputs** are the existing object versions an action will consume;
intermediate states created and consumed within the transaction are not external
inputs.

A **reservation** gives an orchestrator exclusive local use of its requested
external inputs. Its **lease** limits how long this protection lasts before
preparation. The store reserves every requested input atomically or none: each
must be locally held, live in its latest applied chain view, and free of conflicting
reservations or preparations.

The store grants a unique **reservation token**, identifying this reservation
rather than merely its orchestrator. Tokens are never reused after release or
expiry. Retrying an old request cannot acquire a new lease under the same token.
Another holder of the object data can still submit independently; reservations
coordinate cooperating orchestrators through one authoritative store.

The intended lease default is around 20 seconds, within a 10-30 second range.
The API accepts a requested duration and returns the granted duration and expiry,
subject to configurable minimum and maximum values whose exact bounds remain
unspecified. The orchestrator renews roughly every third of the granted duration,
independently of proof generation. These wall-clock durations are not modeled.

The store decides expiration and serializes renewal, release, and preparation
against the token's current state. Before preparation, explicit release or lease
expiry frees the inputs. A stale request cannot revive an expired lease or affect
a newer reservation. Clock behavior across restart must preserve these rules.

## 2. Executing and saving the transaction

The orchestrator obtains **grounding** information: a recent chain state against
which it proves the inputs existed. The **grounding root** identifies that state.
It executes the action and generates a **payload**, the transaction data and proof
that will be submitted to the network.

An **intent** is the desired set of effects. A **submission attempt** is one exact
payload, including its grounding. Different proofs or groundings for the same
intent are different attempts. The expected effects include every creation and
nullifier in the payload, including intermediate states created and consumed within
it, not just the external inputs and final outputs.

Before requesting preparation, the orchestrator saves the payload and its
identifiers in a **durable journal**. Durable means that a successful write survives
a process crash and restart; permanent disk loss is outside this contract. The
journal allows recovery and resubmission of the same attempt without re-proving.
The orchestrator retains it through observations, reorgs, and resolution.

## 3. Preparing the attempt in the store

To **prepare** an attempt is to durably record its intended effects and complete
new object contents in the store, while converting the input lease into protection
that no longer expires. Preparation does not submit the transaction or establish
that its effects have occurred on the chain.

The orchestrator sends its reservation token and immutable attempt data. The store
checks reservation ownership, the unexpired lease, current input eligibility,
object commitments, and consistency of the preparation. It atomically records:

- The external input versions and their nullifiers.
- The complete expected creation and nullifier sets.
- The complete contents of the new object states.
- The grounding root and sufficient chain context to establish its validity.
- An identifier or digest binding the exact saved submission payload.

The resulting **preparation record** binds the reservation to this attempt.
**Input protection** means those inputs remain unavailable to conflicting local
reservations until the attempt is conclusively resolved. The store keeps the
record and object contents; the orchestrator keeps the proof/payload. The store
does not execute the action, and proof verification remains the synchronizer's job.

Only after this atomic durable commit does the store send a **preparation
acknowledgement**: confirmation that it holds the data and protection needed for
this specific attempt to be submitted. The original lease can now expire without
releasing the inputs. Prepared attempts have no automatic timeout or abandonment.

## 4. Submitting and recovering interrupted work

The orchestrator submits its saved payload only after receiving the matching
preparation acknowledgement. It relies on that reply or a query recovering the
same durable result, not on direct access to the store's internal state.

An acknowledgement authorizes the exact attempt. `tx_final` alone is insufficient
as its identity: it commits to effects but not the grounding root. These are
separate in [Payload](../../libs/payload/src/payload.rs) and
[TxFinalBindings](../../libs/txlib/src/predicates/txlib.podlang). A newly grounded
proof requires a new preparation even when its effects are unchanged. Protection
for the old attempt remains until it is conclusively inapplicable.

Messages may be delayed, duplicated, reordered, or lost, and either process may
crash. A timeout therefore does not establish whether preparation succeeded:

- If the store crashed before committing, the orchestrator can retry preparation
  while its lease remains valid. No acknowledgement is justified yet.
- If the commit succeeded but the reply was lost, a retry or query recovers the
  durable result. Repeating the same identity and body returns the recorded result;
  changing the body under that identity is rejected.
- After an orchestrator restart, its journal supplies the saved payload. It can
  recover the store's acknowledgement and submit the same attempt.

Store restart preserves preparations and prevents token reuse. Queries expose the
durable result and current reconciliation state so recovery does not depend on
guessing whether a request completed.

## 5. Observing effects and handling reversals

An **observation** is a coherent synchronizer snapshot: membership of the relevant
creations and nullifiers at one chain state, together with finality information.
**Reconciliation** is the store updating object availability and attempt status
from that observation. The actual chain, synchronizer position, and store's latest
applied view can differ while updates are in flight.

**Effect coverage** describes which of an attempt's expected effects appear in one
observation. Creation commitments and nullifiers remain distinct when comparing
the sets.

| Coverage | Meaning |
| --- | --- |
| None | No expected effects are present; applicability still depends on grounding and other checks. |
| Partial | A nonempty proper subset is present. The attempt cannot apply on this branch because it would repeat an existing effect. |
| Full | All expected effects are present. The intent is satisfied, possibly by a superset or several other transactions. |

Accepted transactions apply atomically: partial coverage is overlap with other
transactions, not partial execution of the submitted payload. Repeating either a
creation or a nullifier rejects the whole payload, as in
[apply_payload](../../services/synchronizer/src/state_machine.rs).

Full coverage permits reporting **observed success** and stopping retries, without
proving which submission produced the effects. Exact commitment equality matters;
equivalent-looking object fields do not satisfy the intent. An expected output
may already be consumed, even within a superset transaction, so effect satisfaction
and current object availability are separate facts.

Observed outputs may be reserved and consumed immediately. For example, after
`A: x,y -> p` is observed, `B: p -> q` can be prepared and submitted before A is
finalized. Observation does not discard the contents, preparations, or journals
needed to recover either attempt.

A **reorg** replaces an unfinalized chain branch, potentially reversing observations.
If A disappears, p becomes unavailable when the store learns of the reversal.
An input whose consumption disappears remains protected by any unresolved
preparation. B's saved proof and preparation also remain; if its grounding is
absent from the replacement branch, the synchronizer rejects it there. Re-proving,
selecting new inputs, or changing the intent are orchestrator policy decisions
requiring a new preparation, not automatic reconciliation steps.

The store cannot react to a reorg it has not learned about. It must also reject
stale observations: freshness is not block height, since a newer view may have a
lower height or a different block at the same height. The model uses a monotonic
publication revision. The implementation needs equivalent ordering or
resynchronization, including across synchronizer restarts.

## 6. Resolving protection with finalized evidence

A block is **finalized** when it is assumed no longer subject to reorg. This model
takes Ethereum finality as an environmental assumption; it does not verify consensus.
Observed effects are reversible until their inclusion is finalized.

An attempt is **permanently inapplicable** when its exact payload cannot apply in
any continuation consistent with finalized history. This resolves its input
protection, whether or not all intended effects were achieved. Conclusive evidence
includes:

- At least one expected effect is finalized, so the payload would collide on replay.
- Its grounding block belongs to a branch excluded by finalized history.
- Finalized progress has reached the end of its grounding validity window, so its
  grounding is too old for every later block, and the finalized effect view is
  known. An attempt can therefore resolve with no effects at all.

The existing synchronizer accepts grounding ages up to 300 execution blocks and
rejects older roots. An error, current-head expiry, relayer receipt, or temporary
absence alone does not justify releasing protection. Once protection ends, each
input must still be present, unspent, and otherwise unreserved to be available.

**Cleanup eligibility** is permission under the finality rule to consider deleting
an object's local contents. The earliest eligibility is finality of its
**consumption**, not its creation. References or retention policy may require
keeping it longer. The model retains all contents and journals and checks only
eligibility, without deleting data.

Effect satisfaction, permanent inapplicability, and cleanup eligibility are thus
separate conclusions: a finalized conflict may resolve an attempt without satisfying
its intent, and releasing its protection does not imply deleting its data.

## Model reference

This development crate participates in the workspace's Rust tests. It does not
call production services, databases, or the proof generator.

### Run

From the repository root:

```sh
cargo test -p protocol-model --release --locked
```

The tests exhaust the finite scenarios below, failing on safety violations or
missing reachability witnesses. Pass `-- --nocapture` to see state counts.

To inspect states and transitions interactively:

```sh
cargo run -p protocol-model --release --locked -- explore reorg
```

Open `http://127.0.0.1:3001`. Pass another address as the last argument if needed.
Run counterexample tests with `--nocapture` to print mutation traces. Each trace
ends at the step that first breaks the property:

```sh
cargo test -p protocol-model --release --locked mutations -- --nocapture
cargo test -p protocol-model --release --locked expired_preparation -- --nocapture
```

### Model boundaries

These bounds limit what the checks establish about possible deployments.

| Scenario | State and interleavings | Bounds |
| --- | --- | --- |
| `leases` | Overlapping atomic reservations, replies, renewal, release, proof journaling, preparation commit, requests repeated or delayed past that commit, acknowledgements, submission | Two operation/token identities; three logical ticks; leases last two ticks; no crashes |
| `recovery` | The same protocol with volatile preparation requests and client knowledge cleared by crashes | One operation; two ticks; at most one crash/restart per process |
| `effects` | Current and finalized observations, partial inputs, output-only overlap, all-input overlap, supersets, and supersets consuming outputs | Genesis plus six alternative one-block branches; one local prepared intent; no reorg |
| `reorg` | Parent and dependent actions, a competing consumption, stale grounding, delayed/duplicate/out-of-order observations, rollback and finality | Two local intents; four blocks besides genesis; maximum height two; one rewind, followed by extension; no process crashes |
| `expiry` | Rejected old grounding, finalized failure without effects, and a newly grounded replacement attempt that needs its own preparation before it applies | One local intent with one replacement attempt; three blocks besides genesis; grounding age limit one; no reorg |

Objects use six symbolic identities, `x,y,z,p,q,r`. An object's creation and its
nullifier are distinct effects. Each object version has one associated nullifier.
`x,y,z` exist at genesis. Local attempts are the parent, its dependent, and the
parent's replacement; blocks are referred to by name.

The effect fixtures assume suitable valid proofs and guards; they do not assert
that a particular installed plugin permits every fixture.

The lifecycle starts with the parent's proof already journaled and its preparation
durable. Preparation replies and submission remain separate steps. The dependent and
replacement attempts each have separate proof journal, durable preparation, reply,
and submission steps. Their reservations and proof computation are abstracted, not
composed with all lease/crash interleavings. This deliberate decomposition is not a
composition proof.

### Synchronizer abstraction

`apply_payload` checks grounding ancestry, age, and proof input membership before
applying effects atomically. Payloads in a block are processed in order. Each block
has a distinct symbolic root, even when its effects match another block's effects.
The modeled grounding age limit is one instead of the production limit 300. Local
payloads use the grounding saved in their journal; external payloads are grounded
at the parent of the block that carries them.

The chain tip, synchronizer position, and store's applied observation are separate.
The synchronizer advances one block at a time, rewinds to a common ancestor, and
publishes snapshots. Published messages remain available for delayed/duplicate
delivery; the store applies the revision ordering described above.

Finite message flags represent repeatable delivery of the same immutable content;
indefinite delay also represents loss for these safety checks. Different request
bodies, unbounded retries, permanent disk loss, adversarial clients, hash collisions,
real Merkle proofs, and Ethereum consensus are not modeled. Correct preparations
are modeled as atomic durable commits; actual storage must establish that boundary.

### Properties and counterexamples

The executable properties are defined in [reservations.rs](src/reservations.rs)
and [lifecycle.rs](src/lifecycle.rs). Safety properties are `Property::always`;
`Property::sometimes` requires a witness for useful execution paths, without
establishing eventual progress or fairness.

Where a property would otherwise repeat the rule it checks, the model keeps a
separate record for it. The reservation model records when each lease ended, so a
later deadline change cannot hide it, and judges exclusive use from that record and
preparation instead of from the store's own check. The lifecycle model records
whether a block accepts an attempt after the store released its protection, which
tests the resolution rule against the chain instead of against itself.

The tests deliberately break thirteen rules and require specific counterexamples:

- Expire a prepared reservation with its old lease.
- Acknowledge before durable preparation.
- Accept a stale token because another reservation exists.
- Renew a lease after it has expired.
- Honor an explicit release of a prepared reservation.
- Reserve inputs that a prepared attempt holds once its lease expires.
- Lose acknowledged preparation data on store crash.
- Release input protection on an unfinalized observation.
- Measure grounding expiry from the observed head instead of finalized progress.
- Apply an older observation over a newer one.
- Keep an orphan output available after applying rollback.
- Allow cleanup on observed rather than finalized consumption.
- Authorize a freshly grounded replacement with the old preparation acknowledgement.

The regression traces also cover a retried request recovering the response after
both processes restart, release/expiry racing preparation, and a prepared local
attempt racing an independent spend in either order within a block. They check
finalized conflict releasing only still-live inputs, stale observations after
finalization, and dependent proofs rejected on a replacement branch while local
contents and journals remain retained.

### Connection to production

The model mirrors rules from
[`state_machine.rs`](../../services/synchronizer/src/state_machine.rs), especially
`apply_payload` and `derive_slot_head`. It does not call those functions: they
depend on RocksDB-backed Merkle containers and real payload types. The production
state machine and model have not yet been checked for conformance.
