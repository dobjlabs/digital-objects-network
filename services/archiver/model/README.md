# Acquisition and recovery contract

[acquisition.qnt](acquisition.qnt) covers parent payload acquisition, withheld payloads, partial publication, completion markers, fork changes, reorg cleanup and restart. A child can reveal or skip its parent's payload. Required blobs belong under the parent's committing root; the child's header marks completion. Recovery retains partial acquisition under an already processed parent so retry can finish it, and rewind preserves unrelated roots. See the [shared tooling instructions](../../../devtools/models/README.md) for commands.

## Assumptions and exclusions

The contract assumes one writer, bounded retained history and a valid supplied consensus graph. Successful synchronization and atomic rename are trusted primitives; process recovery does not establish power-loss behavior. SSE, remote head polling, arbitrary filesystem corruption, cryptographic validity and HTTP read races are excluded. Garbage collection is limited to reorg cleanup of the stored branch.
