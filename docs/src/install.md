# Installing the driver

The driver is a service that manages the digital objects themselves. Installing
it puts four binaries under `~/.dobj/bin/`:

| Binary           | What it is                                                                          |
| ---------------- | ----------------------------------------------------------------------------------- |
| `dobjd`          | Driver daemon with bundled browser UI (REST/SSE on `:7717`, optional MCP on `:7718`) |
| `dobj`           | Terminal CLI                                                                        |
| `dobj-mcp-proxy` | Stdio bridge for agents that only speak stdio (e.g. Claude Desktop)                 |
| `pexe`           | Packaging tool for `.pexe` plugin archives                                          |

The driver installs with an **empty action catalog**: applications ship
separately as plugins (`.pexe` files). The steps below install the binaries,
open the browser UI with `dobj ui`, and add an example plugin such as
[craft-basics](./examples/craft-basics.md) or
[craft-rocket](./examples/craft-rocket.md).

Release binaries are configured with the
[hosted synchronizer and relayer endpoints](./testnet.md#hosted-services), so a
fresh install needs no configuration.

## 1. Install the binaries

The install script downloads the latest release into `~/.dobj/bin` and prints a
PATH hint. To pin a version, set `DOBJ_VERSION` (bash) / `$env:DOBJ_VERSION`
(PowerShell) to a release tag first.

On macOS or Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/install.sh | sh
```

On Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/install.ps1 | iex
```

> **Windows:** the binaries aren't codesigned yet, so SmartScreen may show
> "Windows protected your PC". Click **More info → Run anyway**.

To install without the script, see [Manual install](#manual-install) below.

## 2. Open the browser UI

On macOS or Linux:

```bash
~/.dobj/bin/dobj ui
```

On Windows:

```powershell
& "$env:USERPROFILE\.dobj\bin\dobj.exe" ui
```

`dobj ui` starts the daemon if needed and opens its default UI at
`http://127.0.0.1:7717/ui/`. Run it again whenever you want to return to the UI.
The release daemon bundles the frontend, so no Node.js or Vite setup is needed.
The action catalog is empty until you install a plugin in the next step.

For CLI or agent use without opening a browser, use `dobj start` instead.
`dobj ui --no-open` starts the daemon if needed and prints the URL.

> **Note:** the first start builds ZK circuits and can take a few minutes.
> Later starts take seconds.

## 3. Install a plugin

`dobj install` takes a local `.pexe` path or an http(s) URL, validates it, and
hot-reloads the running daemon's catalog, so the daemon does not need to
restart. Two example plugins ship with each release:

- [craft-basics](./examples/craft-basics.md), a small crafting demo.
- [craft-rocket](./examples/craft-rocket.md), a larger factory tech tree.

Install either, or both. On macOS or Linux:

```bash
~/.dobj/bin/dobj install https://github.com/dobjlabs/digital-objects-network/releases/latest/download/craft-basics.pexe
~/.dobj/bin/dobj install https://github.com/dobjlabs/digital-objects-network/releases/latest/download/craft-rocket.pexe
```

On Windows:

```powershell
& "$env:USERPROFILE\.dobj\bin\dobj.exe" install https://github.com/dobjlabs/digital-objects-network/releases/latest/download/craft-basics.pexe
& "$env:USERPROFILE\.dobj\bin\dobj.exe" install https://github.com/dobjlabs/digital-objects-network/releases/latest/download/craft-rocket.pexe
```

After installing a plugin, reload the browser tab opened by `dobj ui` to load
its actions.

## 4. Verify

On macOS or Linux:

```bash
~/.dobj/bin/dobj status      # pid + HTTP healthcheck
~/.dobj/bin/dobj actions     # installed plugin actions
~/.dobj/bin/dobj state-root  # 64-hex root: the hosted synchronizer is reachable
```

On Windows:

```powershell
& "$env:USERPROFILE\.dobj\bin\dobj.exe" status
& "$env:USERPROFILE\.dobj\bin\dobj.exe" actions
& "$env:USERPROFILE\.dobj\bin\dobj.exe" state-root
```

## 5. Connect a client

Every client talks to the same daemon:

- **Browser UI**: `dobj ui` (starts the daemon if needed and opens your browser)
- **CLI**: `~/.dobj/bin/dobj` (add `~/.dobj/bin` to your PATH)
- **Claude Code / MCP agents**: `claude mcp add --transport http dobj http://127.0.0.1:7718/mcp`
- **Stdio-only agents (e.g. Claude Desktop)**: point them at `~/.dobj/bin/dobj-mcp-proxy`

MCP is off by default. Before connecting an agent, enable it with
`dobj settings set --mcp on` (use the full binary path if it is not on your
PATH yet). See
[INSTALL.md](https://github.com/dobjlabs/digital-objects-network/blob/main/INSTALL.md#connect-an-agent-mcp)
for agent-specific setup.

After connecting MCP and starting a fresh agent session, you can optionally
open the Digital Objects command menu. In Claude Code:

```text
/mcp__dobj__start
```

In Codex:

```text
Call dobj invoke_prompt with: {"name":"start","arguments":{"command":"help"}} Then follow the returned messages.
```

All MCP tools remain available without starting the command interface.

## Installing with an agent

An MCP-aware agent (Claude Code, Cursor, ...) can perform the steps above. Give
it this prompt:

> Read https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/INSTALL.md, install and start the Digital Objects driver, install the craft-rocket plugin, and configure MCP for this agent if supported.

[INSTALL.md](https://github.com/dobjlabs/digital-objects-network/blob/main/INSTALL.md)
walks the agent through each step, including plugin install and MCP
connection.

## Manual install

To fetch the archives yourself, detect your platform's target triple, then
download the three tarballs from the
[releases page](https://github.com/dobjlabs/digital-objects-network/releases)
(the `dobjd` tarball bundles `dobj-mcp-proxy`).

On macOS or Linux:

```bash
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)   TARGET=aarch64-apple-darwin ;;
  Darwin-x86_64)  TARGET=x86_64-apple-darwin ;;
  Linux-x86_64)   TARGET=x86_64-unknown-linux-gnu ;;
  *) echo "unsupported platform: $(uname -sm)"; exit 1 ;;
esac

RELEASE=https://github.com/dobjlabs/digital-objects-network/releases/latest/download
mkdir -p ~/.dobj/bin
curl -fsSL "$RELEASE/dobjd-$TARGET.tar.gz" | tar -xz -C ~/.dobj/bin
curl -fsSL "$RELEASE/dobj-$TARGET.tar.gz"  | tar -xz -C ~/.dobj/bin
curl -fsSL "$RELEASE/pexe-$TARGET.tar.gz"  | tar -xz -C ~/.dobj/bin
chmod +x ~/.dobj/bin/dobjd ~/.dobj/bin/dobj ~/.dobj/bin/dobj-mcp-proxy ~/.dobj/bin/pexe
```

On Windows (PowerShell):

```powershell
$TARGET  = "x86_64-pc-windows-msvc"     # the only Windows target built
$RELEASE = "https://github.com/dobjlabs/digital-objects-network/releases/latest/download"
$DOBJ    = "$env:USERPROFILE\.dobj"
New-Item -ItemType Directory -Force -Path "$DOBJ\bin" | Out-Null
foreach ($name in @("dobjd", "dobj", "pexe")) {
    $tmp = "$DOBJ\$name-$TARGET.tar.gz"
    curl.exe -fsSL "$RELEASE/$name-$TARGET.tar.gz" -o $tmp
    tar -xzf $tmp -C "$DOBJ\bin"
    Remove-Item $tmp
}
```

Then continue from [2. Open the browser UI](#2-open-the-browser-ui).

## Other infrastructure

The hosted endpoints are defaults, not requirements. See
[Choosing services](./testnet.md#choosing-services) for how to point the driver
at any synchronizer and relayer, including your own.
