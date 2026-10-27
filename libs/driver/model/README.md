# Transaction record contract

[transactions.qnt](transactions.qnt) is the driver's contract for the life of a transaction once proving is split from recording, submitting and settling. It covers taking and releasing input holds, recording outputs and payload, deleting a recorded transaction, submitting to the relayer, relayer broadcast and failure, the chain landing or outrunning a transaction, the settlement pass, and crashes at each write inside record and inside submit. [transactions.test.qnt](transactions.test.qnt) pins the scenarios that decided the design; `safety` is checked by random simulation over the same commands.

The state is the driver as it sits on disk and in memory: object files (status and the txFinal that wrote them), the transactions directory (payloads and records), the hold table and the proof jobs. The environment is the chain (head block, created set, nullifier set) and the relayer (one job per txFinal, with broadcast and failed flags). Basenames double as commitments and nullifiers, so one small integer names an object everywhere. A record's txFinal is the id of the proof job that produced it.

## What the contract says

Open records (`Recorded`, `Submitted`) have pairwise disjoint inputs, and no held basename is an input of an open record. `Landed` and `Expired` are terminal and agree with the chain: a landed record's transaction is in the chain's landed set and an expired record's never is. A landed record's outputs exist, carry its txFinal and are no longer `Unknown`; its inputs are `Nullified`. An expired record owns no `Unknown` output file. Every `Unknown` file is owned by an open record, except a file left by a crash before the record file was written. `Live` means the commitment is in the created set and `Nullified` means the nullifier is in the nullifier set, so object status is never ahead of the chain. Status moves only forward, and a file's txFinal never changes. A deleted transaction is never one the relayer holds.

Three decisions in the plan fall out of these invariants, and each is a one-line change to the model that `quint run` refutes within a second:

- Relayer failure is an annotation, not a terminal state. A job can fail after its send went out, so a `failed` job may still land. If settle treated it as terminal and removed the outputs, the later landing would leave a landed transaction with no output files. `relayerFailureIsAnAnnotationTest` is the trace.
- Settle reads the head block and the membership answers from one snapshot. The expiry rule is `head - grounding > maxAge` against the same head the membership answer was taken at. Reading the head after the membership answer is the same as expiring one block early, and `quint run` then finds a transaction marked `Expired` that lands. `landedAtTheAgeLimitStaysLandedTest` is the boundary trace.
- Deletion must exclude transactions whose payload may be at the relayer. If the process dies after the relayer accepted the payload but before the record was marked `Submitted`, the record still reads `Recorded` while the payload can land. Deleting it would remove the outputs of a transaction that lands. `Crash(RelayerAccepted)` is that window; `deleteRefusedAfterRelayerAcceptTest` and `crashAfterRelayerAcceptStillSettlesTest` show the refusal and that settle lands the record anyway, since membership covers every open record and the relayer is idempotent on txFinal.

## Assumptions and exclusions

The chain lands a payload only while `head - grounding <= maxAge` and only if none of its commitments or nullifiers collide with committed state; this is the synchronizer's rule and the contract's one assumption about the environment. The real rule compares the inclusion block with the grounding block, so the model's guard admits landings the synchronizer would refuse, which is the safe direction for the invariants. The output-file batch, payload write, and record rename are modeled as atomic steps. Imports, the discard escape hatch, transactions proved outside the driver, explicit reservations and relayer fee bumps are outside the contract; so is any liveness claim, since the environment may stop advancing.

Bounds: four object ids, two present and live at the start, at most four proofs, a head of at most eight blocks and a grounding age limit of two. Random simulation reaches landings, expiries, deletes, collisions and every crash phase often; chained spends and two concurrent landings are rare, so the test module pins them.

## Run

Requires Node.js 22 and pnpm 10.34.8. From the repository root:

```sh
just models
```

For individual checks, install the pinned tools once:

```sh
pnpm --dir devtools/models install --frozen-lockfile --ignore-scripts
pnpm --dir devtools/models typecheck:driver
pnpm --dir devtools/models test:driver
pnpm --dir devtools/models simulate:driver
```

`check:driver` runs all three. The [shared tooling](../../../devtools/models/README.md)
pins Quint 0.33.0, the dependency lockfile, and the simulation seed and bounds.
The [Quint workflow](../../../.github/workflows/models.yml) runs these checks in CI.

## Rust replay adapter

The workspace includes the `quint-connect` dependency pin, but the driver adapter
is still to be implemented. It will replay model transitions against a `Driver`
on a temporary home, using mock synchronizer and relayer state, gates at crash
boundaries, and restart by reopening the driver. It will compare object files,
transaction records, and holds against `node`; auxiliary variables such as
`deleted`, `strays`, and `previousObjects` are used only by the specification.
