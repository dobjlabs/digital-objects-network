# Bitcraft

Bitcraft is a resource-production game built on the Digital Objects Network.
Every resource, tool, and production facility in it is a Digital Object, and
every gather, craft, mine, and upgrade is a proved action. 27 classes and 134
actions.

Unlike the other examples, Bitcraft ships as a desktop application rather than
a plugin alone. The app bundles its own driver daemon, the `bitcraft` plugin,
and the game UI, so it does not need a separate [driver install](../install.md).

## Installing

Installers are attached to each
[release](https://github.com/dobjlabs/digital-objects-network/releases):

| Platform              | Installer                                              |
| --------------------- | ------------------------------------------------------ |
| macOS (Apple Silicon) | `bitcraft-<version>-aarch64-apple-darwin.dmg`          |
| macOS (Intel)         | `bitcraft-<version>-x86_64-apple-darwin.dmg`           |
| Windows (x86_64)      | `bitcraft-<version>-x86_64-pc-windows-msvc.exe`        |
| Linux (x86_64)        | `bitcraft-<version>-x86_64-unknown-linux-gnu.AppImage` |

The app keeps its objects, settings, and installed plugin in `~/.bitcraft/`,
separate from a standalone driver's `~/.dobj/`, and runs its daemon on
`127.0.0.1:7727`. It uses the same
[hosted synchronizer and relayer](../testnet.md#hosted-services) as the
standalone driver. On first launch it builds its ZK circuits and creates a
starter pocketknife.

## Gameplay

- A **pocketknife** is created from no inputs and gathers wood, stone, and iron
  slowly, with a small proof of work per gather (about 15, 30, and 60 seconds
  on a modern CPU).
- **Sticks** are made from wood. **Axes** (wood, then stone, then iron) chop 2,
  3, or 4 wood per use and break after 100 uses.
- Three production facilities can each be built and then upgraded from level 1
  to level 10, with costs doubling at every level:
  - a **quarry** mines stone, more per action at higher levels;
  - a **mining drill** mines iron, coal, and geodes, consuming 5 wood per
    action;
  - a **pumpjack** pumps oil into a barrel for 5 coal. The player chooses the
    number of steps, and each step is serial (non-parallelizable) work.
- A **geode** cracks open into one of six gems: quartz, amethyst, sapphire,
  emerald, ruby, or diamond.
- **Buckets** hold a count of one resource, and a **barrel** holds oil.
  Facility upgrades spend buckets rather than individual objects.
- **Bits** are the game's currency. Wood, stone, and iron convert to 1, 2, and
  5 bits; gems convert into a bucket of bits, from 100 for a quartz to
  320,000,000 for a diamond.

## Classes

| Group      | Classes                                                                                          |
| ---------- | ------------------------------------------------------------------------------------------------ |
| Resources  | `Wood`, `Stone`, `Iron`, `Coal`, `Stick`, `Geode`                                                |
| Gems       | `Quartz`, `Amethyst`, `Sapphire`, `Emerald`, `Ruby`, `Diamond`                                   |
| Containers | `BucketOfWood`, `BucketOfStone`, `BucketOfIron`, `BucketOfCoal`, `BucketOfQuartz`, `BucketOfBits`, `BarrelOfOil` |
| Tools      | `Pocketknife`, `WoodAxe`, `StoneAxe`, `IronAxe`                                                  |
| Facilities | `Quarry`, `MiningDrill`, `Pumpjack`                                                              |
| Currency   | `Bit`                                                                                            |

## What it demonstrates

- **Proof of work bound to the tool.** Every proof-of-work action grinds the
  `nonce` of the tool it uses, after every other update to that tool, so the
  work bounds the tool's new commitment. A tool's first nonce is the grounding
  block's hash. Work toward a tool's next use can only start once its previous
  use is mined, and a nonce mined for one tool state proves nothing for any
  other, so resources cannot be premined.
- **Randomness from the work itself.** A geode records the drill's new
  commitment, and cracking it compares that value against the geode's gem
  thresholds, so which gem a geode holds is decided by the mining work.
- **Serial work.** Each pumpjack step is a computation that cannot be
  parallelized, so oil output cannot be sped up by adding cores.
- **Quantities in one object.** Buckets and barrels carry a count, so a recipe
  that needs hundreds of a resource spends one object rather than hundreds.

## Agents

The app installs `bitcraft-dobj` and `bitcraft-dobj-mcp-proxy`, wrappers
around the CLI and the MCP proxy that connect to the running app's daemon. The
game's toolbar can open Claude or Codex with the Bitcraft data folder selected
and a player prompt filled in. For agents without shell access, enable MCP
with `bitcraft-dobj settings set --mcp on` and connect the agent to
`http://127.0.0.1:7728/mcp`.

## Source

- [`examples/bitcraft/`](https://github.com/dobjlabs/digital-objects-network/tree/main/examples/bitcraft):
  the plugin. `plugin.rhai` and `manifest.toml` are generated by `src/main.rs`.
- [`apps/bitcraft/`](https://github.com/dobjlabs/digital-objects-network/tree/main/apps/bitcraft):
  the desktop app (Tauri host and game UI). Its README covers building and
  running it from a checkout, starting with `just bitcraft-dev`.
