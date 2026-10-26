# Publication and snapshot contract

[publication.qnt](publication.qnt) covers publication of genesis and later DON states, slot progress without a new state, rollback, interrupted writes, restart and concurrent snapshot reads. The [Connect adapter](../src/node/model_tests/publication.rs) pauses production operations around container flush, state-log publication and reading a captured snapshot's indexes. Uncommitted writes must stay invisible, committed state must reopen after restart, and readers must retain their captured progress, state and membership across concurrent publication or rollback. See the [shared tooling instructions](../../../devtools/models/README.md) for commands.

## Assumptions and exclusions

The contract assumes a single writer, immutable backing data and a finalized genesis execution block. It exercises publication from an empty store and recovery on either side of the first commit; waiting for finalization and rejecting unsuitable genesis choices are outside its scope. Successful RocksDB WAL flush and Postgres commit are trusted durable primitives. Process interruption does not establish storage-engine correctness or power-loss behavior. Multiple writers, reader cancellation beyond discarding a request, and backing-store garbage collection are excluded.
