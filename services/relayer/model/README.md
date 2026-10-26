# Delivery and recovery contract

[delivery.qnt](delivery.qnt) covers idempotent enqueue, due-job scheduling, bounded retries, signed transaction persistence, node refusals, transport errors, lost replies, interruption before or after network acceptance, fee replacements, receipts, dropped transactions, optional tracking timeout and restart. The [Connect adapter](../src/worker/model_tests.rs) runs the real worker and Postgres with a controlled clock and an independent gateway ledger. It checks persisted transaction bytes and identity, nonce reservations, replacement history and job state. Safety requires successful receipts for confirmation and prevents unrelated jobs from sharing accepted nonces. See the [shared tooling instructions](../../../devtools/models/README.md) for commands.

## Assumptions and exclusions

This is single-worker, at-least-once delivery. A JSON-RPC refusal on the first send means the provider did not forward that transaction; a provider that forwards it and then reports refusal violates this assumption. Transport errors and later resend errors remain ambiguous; recovery must preserve signed bytes and identity. A tracking timeout, when configured, ends tracking only after receipt reconciliation. A timed-out job can still appear on-chain later.

Ethereum fee amounts and signing internals are abstracted, and the gateway supplies stable fees. Chain finality, receipt reorgs, external signers sharing the account, and RPC ambiguity during database recovery are outside the model. Restart reconnects to Postgres; interrupting a worker future exercises process-state loss without simulating an OS power failure.
