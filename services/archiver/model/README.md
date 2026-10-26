# Acquisition and recovery contract

[acquisition.qnt](acquisition.qnt) covers parent payload acquisition, withheld payloads, partial filesystem publication, completion markers, fork changes, reorg cleanup and restart. In the Gloas-style fixture, a child's bid can reveal or skip its parent's payload. Required blobs belong under the parent's committing root; the child's header marks completion. A processed parent header does not promise acquisition of that parent's own payload. The [Connect adapter](../src/node/model_tests.rs) uses production HTTP clients, transaction filtering and filesystem operations, checking ancestry, markers and stored blob contents. Recovery must retain partial acquisition under an already processed parent so retry can finish it, while rewind preserves unrelated roots. See the [shared tooling instructions](../../../devtools/models/README.md) for commands.

## Assumptions and exclusions

The fixture assumes one writer, bounded retained history and a valid supplied consensus graph. Successful file and directory synchronization and atomic rename are trusted primitives; aborting work and recovering the store exercises process-crash recovery without establishing power-loss behavior.

SSE, remote head polling, arbitrary filesystem corruption, cryptographic validity and HTTP read races are outside the contract. Signatures and KZG commitments are fixture data. Cached blobs exercise production loading; garbage collection is limited to reorg cleanup of the stored branch.
