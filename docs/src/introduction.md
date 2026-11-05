# Introduction

The **Digital Objects Network** is a decentralized network that facilitates
the creation, execution, and exchange of **Digital Objects**. Digital Objects
are fully programmable state machines that are owned and operated by Internet
users, and that can be passed between mutually untrusting Internet users (for
example via email) while maintaining their _integrity_ and _consistency_,
without relying on any central trusted authority.

The Digital Objects Network is a very general platform that many applications
could be built on. Concretely, Digital Objects can be used to represent many
different things: a record in a distributed database; an entire database; a
unit of monetary value; an access permission; an attestation; the status of or
a lock on another Digital Object or group of Objects; and more. Because Digital
Objects are fully programmable and maintain their integrity and consistency
across trust boundaries, applications whose data and procedures must execute
across multiple untrusting entities, and that were previously prohibitively
expensive to run (in trust, operational overhead, or other dimensions), can be
operated on top of the Digital Objects Network. For example, transferring
residency records between multiple countries or states with heterogeneous
systems of record can be implemented as a single atomic action if Digital
Objects exist to represent "ports" into each individual system. Likewise with
money moving between different financial networks, or records on multiple
different ERPs that a business would like to ensure update atomically.

Objects are files held privately by their owners. Each state transition is
proved in zero knowledge, and the network publishes only opaque commitments to
Ethereum blob data, so the chain sees neither an object's contents nor its
code.

## Stack

The concepts required to realize the Digital Objects Network, from higher to
lower level, next to how they are implemented today:

| Concept                          | Implementation                                                |
| -------------------------------- | ------------------------------------------------------------- |
| global state derivation          | permissionlessly operable [synchronizer](./architecture.md#synchronizer) |
| global bulletin board            | [Ethereum blobspace](./architecture.md#ethereum-blobs)        |
| Class-Action SDK                 | pexe/Rhai SDK                                                 |
| Class-Action protocol            | podlang txlib                                                 |
| private statement proving system | [POD2](https://0xparc.github.io/pod2/)                        |

POD2 is itself layered:

| Concept                     | Implementation          |
| --------------------------- | ----------------------- |
| predicate language          | podlang                 |
| statement derivation kernel | MainPod plonky2 circuit |
| zk proving system           | plonky2                 |

## Contents

- [Concepts](./concepts.md) describes Digital Objects, the operations users can
  perform on them, and what the network needs to stay live.
- [Architecture](./architecture.md) describes the components that make up the
  network and the client configurations users assemble from them.
- [Installing the driver](./install.md) covers installing the driver, opening
  its browser UI, and adding a plugin.
- [Testnet](./testnet.md) lists the parameters of the current testnet and the
  hosted services that serve it.
- [Running the services](./deployment.md) covers running your own archiver,
  synchronizer, and relayer.
- [Examples](./examples/index.md) describes how applications are packaged as
  plugins, and the example applications in the repository.

The source for the network and these docs is at
[github.com/0xPARC/digital-objects-network](https://github.com/0xPARC/digital-objects-network).
