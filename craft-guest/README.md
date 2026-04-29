# craft-guest

risc0 zkVM guest binary for the craft-basics action set.

Not a workspace member of the parent `zk-craft` repo — depends on the risc0
toolchain (`riscv32im-risc0-zkvm-elf` target). Build it standalone:

```bash
# One-time setup
curl -L https://risczero.com/install | bash
rzup install

# Build the guest ELF
cd craft-guest
cargo build --release
# → target/riscv32im-risc0-zkvm-elf/release/craft-guest
```

The driver (Phase 4) will load this ELF via the `craft-methods` build script,
which invokes `risc0-build::embed_methods` and exposes `CRAFT_GUEST_ELF` and
`CRAFT_GUEST_ID` constants.

## What it does

```
host (driver) → borsh(GuestInput) → env::read()
                                    │
                                    ▼
                         craft_actions::validate
                          ├─ grounding::verify_all  (Merkle inclusion proofs)
                          ├─ tx_build::nullifiers_for
                          ├─ dispatch by action_id  (per-action predicate)
                          └─ tx_build::build_journal (live_root, tx_final)
                                    │
                                    ▼
                         env::commit_slice(borsh(GuestJournal))
                                    │
                                    ▼
synchronizer → Receipt::verify(IMAGE_ID) → decode journal
```

Panics — bad grounding, predicate violation, malformed input — abort proving.
The host treats no-receipt as "this action wasn't possible".
