# Protocol models

Shared Quint tooling for the repository's executable specifications.

| Contract | Scope | Rust replay |
| --- | --- | --- |
| [Driver transactions](../../libs/driver/model/README.md) | Input ownership, recording, submission, settlement, and crash recovery | Planned |

## Requirements

Use Node.js 22 and pnpm 10.34.8, pinned by `packageManager`. The Quint CLI is
pinned to 0.33.0 and its dependency tree is recorded in `pnpm-lock.yaml`.
These model checks need no Rust build, database, or running services. The
Rust evaluator downloads on first use, so the first simulation needs network
access.

## Run

From the repository root:

```sh
just models
```

Or install once and invoke the scripts directly:

```sh
pnpm --dir devtools/models install --frozen-lockfile --ignore-scripts
pnpm --dir devtools/models check
```

`check:driver` runs typechecking, all deterministic scenarios, and 10,000
bounded simulations of up to 60 steps, using seed `20261026`. Run
`typecheck:driver`, `test:driver`, or `simulate:driver` separately when needed.
The [Quint workflow](../../.github/workflows/models.yml) runs `check` in CI
with the same pinned tools and lockfile.

These are sampled safety checks, not exhaustive verification or liveness
proofs. Each model documents its bounds and assumptions. Keep new contracts
beside the implementation and register their checks in `package.json`.

## Rust replay

Adapters that replay model transitions against production Rust code are
planned. The driver adapter is not implemented yet; passing these scripts
checks the specification itself, without comparing it to the driver
implementation. Add the Rust replay dependency when an adapter uses it.
