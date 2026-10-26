# Synchronizer ancestry contract

[reorgs.qnt](reorgs.qnt) covers fetching individual slots and batches, stale and mixed fetches, skipped slots, fork changes, rollback, state replay and restart. Safety requires connected ancestry between produced blocks, derived DON states matching replay, and indexes matching the head's ordered objects. The [Connect adapter](../src/node/model_tests.rs) uses production fetch clients, validation, Merkle derivation and storage; it reopens persisted states and checks their contents. See the [shared tooling instructions](../../../devtools/models/README.md) for commands.

The model starts with a finalized genesis execution block and includes its state in history. Slot progress advances independently of DON state: skipped slots and blocks without canonical execution payloads produce no state entry. The adapter validates and processes genesis through the production slot-processing path before replaying subsequent events. It checks the sparse state log, the selected head and progress, and reopened Merkle contents. Model slots are offsets from the configured genesis slot.

## Assumptions and exclusions

The contract assumes one writer, atomic slot/head/index publication and rollback, immutable backing data, a retained finalized genesis and an available common ancestor, and a stable remote view during ancestor search. Missing-slot observations remain provisional until a produced block validates its ancestry. Per-block deltas are supplied as already accepted; the Rust fixtures exercise validation without establishing proof correctness, arbitrary grounding ages or collision rejection.

The adapter exercises catch-up's shared reorg check; the full scheduling loop, head tracker, SSE, blob selection, wire encoding and cryptographic verification are outside its scope. Restart reopens stores. The [publication contract](PUBLICATION.md) covers interrupted genesis and state publication, progress without derivation and concurrent snapshots. Failures during ancestor search and eventual progress are not modeled.
