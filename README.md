# Digital Objects Network

The Digital Objects Network is a decentralized network that facilitates the
creation, execution, and exchange of Digital Objects. Digital Objects are fully
programmable state machines that are owned and operated by Internet users, and
that can be passed between mutually untrusting Internet users while maintaining their integrity and consistency, without relying on any central trusted authority.

## Getting started

- **Install (end user):** [INSTALL.md](INSTALL.md), or point an MCP-aware agent at [SKILL.md](SKILL.md).
- **Develop from source:** [CONTRIBUTING.md](CONTRIBUTING.md).
- **Self-host the services:** [deploy/](deploy/README.md).

## Repository map

Each directory has its own README with the details.

### Services

| Directory                                                 | Role                                                                                                                                   |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| [services/dobjd/](services/dobjd/README.md)               | The daemon. Owns all driver state; serves the REST/SSE API on `:7717` and an embedded MCP server on `:7718`. Every client talks to it. |
| [services/synchronizer/](services/synchronizer/README.md) | Reads blobs from the chain, maintains the Merkle state, and serves grounding/membership queries.                                       |
| [services/relayer/](services/relayer/README.md)           | Wraps proof payloads as EIP-4844 blob transactions and submits them to Ethereum.                                                       |
| [services/archiver/](services/archiver/README.md)         | Follows beacon blocks and archives blobs filtered by destination address, serving them over a beacon-compatible API.                   |

### Interfaces

| Directory                                   | Role                                                                                                     |
| ------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| [interfaces/cli/](interfaces/cli/README.md) | The `dobj` terminal CLI. A thin HTTP/SSE client of dobjd.                                                |
| [interfaces/gui/](interfaces/gui/README.md) | React frontend, runnable in a browser or wrapped in a Tauri desktop shell. Talks to dobjd over HTTP/SSE. |
| [interfaces/mcp/](interfaces/mcp/README.md) | MCP server library exposing the driver as tools to AI agents, plus the `dobj-mcp-proxy` stdio bridge.    |

### Libraries

| Directory                                       | Role                                                                                      |
| ----------------------------------------------- | ----------------------------------------------------------------------------------------- |
| [libs/driver/](libs/driver/README.md)           | The core orchestration library. Owns `~/.dobj/` and runs actions end-to-end.              |
| [libs/sdk/](libs/sdk/README.md)                 | Rhai engine and two-phase loader/executor that compiles plugin scripts into pod2 modules. |
| [libs/txlib/](libs/txlib/README.md)             | Transaction state machine: event hash chain, grounding, and the `TxFinalized` predicate.  |
| [libs/payload/](libs/payload)                   | Blob payload encoding and the plonky2 proof shrink wrapper.                               |
| [libs/pexe/](libs/pexe/README.md)               | The `.pexe` plugin archive format and its packaging CLI.                                  |
| [libs/pod2utils/](libs/pod2utils)               | Macros and helpers for loading podlang modules.                                           |
| [libs/wire-types/](libs/wire-types)             | Dependency-light data types crossing process boundaries (HTTP/MCP/SSE/CLI).               |
| [libs/eth-clients/](libs/eth-clients/README.md) | Partial Ethereum Beacon client API used by the chain-side services.                       |
| [libs/intro-pods/](libs/intro-pods)             | Intro pods for proof-of-work gating (`vdfpod`, `lt-eq-u256-pod`).                         |

### Examples & deploy

| Directory                   | Role                                                                                    |
| --------------------------- | --------------------------------------------------------------------------------------- |
| [examples/](examples)       | Example plugin sources: `craft-basics` (the bundled crafting demo) and `craft-rocket`.  |
| [deploy/](deploy/README.md) | Container images and Compose stack for running the synchronizer, relayer, and archiver. |

Built on [pod2](https://github.com/0xPARC/pod2) (0xPARC's predicate-of-data
system) over `plonky2`: proofs are constant-size regardless of input count, and
the chain sees only opaque commitments.
