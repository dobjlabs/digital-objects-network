# Bitcraft desktop

Bitcraft is one desktop product containing a Tauri host, the game UI, Digital Objects daemon and tools, the Bitcraft plugin, and initial network configuration. All components are built from this repository and delivered together. Digital Objects also remains available as a standalone product.

The host launches its own daemon on `127.0.0.1:7727` (MCP uses `7728` when enabled), verifies its launch identity, then displays the daemon's `/bitcraft/` page in a webview. The game page has no Tauri IPC permissions. Native menus and runtime management stay in Rust. The UI remains a static HTML/JavaScript client with no frontend build step.

## Development

Install the repository's Rust toolchain, Python 3, and Node.js 22+ for the Tauri CLI. On Ubuntu, install `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf file`. macOS requires Xcode command line tools; Windows requires MSVC build tools and WebView2. See [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

From the repository root:

```sh
# Fast desktop shell iteration, without creating game state or starting dobjd.
just bitcraft-shell

# Build the daemon, tools, and plugin, then launch the full application.
just bitcraft-dev

# Launch against an already-running local backend stack with separate data.
just bitcraft-dev-local

# Build an unsigned development installer containing the prepared runtime.
just bitcraft-bundle

# Regenerate the bitcraft presentation pack and copy it into the UI.
just bitcraft-presentation
```

Icons, summaries and descriptions for classes and actions come from presentation packs, which only the app reads; see [PRESENTATION_PACKS.md](PRESENTATION_PACKS.md). Preparation copies them into `ui/presentation/`.

`just bitcraft-dev-local` uses `<checkout>/.dev/bitcraft-local` as its home, overriding `BITCRAFT_HOME`. On first use it creates `settings.json` with synchronizer `http://127.0.0.1:3000`, relayer `http://127.0.0.1:3200`, and MCP disabled. Later launches preserve existing settings, objects, and actions. This directory is ignored by Git. The local backend stack must already be running; for example, run `just dev-local` in another terminal. Its standalone daemon uses port `7717`; Bitcraft continues to use `7727` (MCP `7728` when enabled).

When using Git worktrees, keep the backend stack in the checkout that owns
its existing `data/` directory, and run `just bitcraft-dev-local` from the
UI worktree. `just dev-local` uses shared Postgres database names
(`synchronizer_local` and `relayer_local`) but checkout-relative RocksDB,
chain, and blob files. Starting that stack in a fresh worktree can pair old
Postgres roots with an empty RocksDB store, producing "node with hash ...
not found" errors. Stop the misplaced stack and restart it in the original
checkout to use the matching stores together.

Quit any running Bitcraft instance before switching homes: they share the application's single-instance identity and ports. To use the wrappers with the local development instance, run them from `.dev/bitcraft-local` or pass `--home "$PWD/.dev/bitcraft-local"` from the repository root. Edit that home's `settings.json` while the app is stopped, or use the wrapper's `settings set` command while it is running, to customize the endpoints. The local recipe preserves those changes.

Quit Bitcraft and stop the local backend stack before running `just reset`. Reset removes `.dev/bitcraft-local` along with the devnet state, since its objects are grounded on that chain. The next `just bitcraft-dev-local` recreates the initial local settings; custom settings in that home are lost. If you wipe the devnet manually, also remove this home (or the custom home used with that devnet). The normal `~/.bitcraft` home is unaffected by `just reset`.

`scripts/prepare.py` builds the exact binaries, checks the plugin manifest with `pexe build --check`, and stages them under `src-tauri/binaries` and `src-tauri/runtime`. It verifies that the daemon embeds this checkout's complete UI before staging it, using a SHA-256 fingerprint of every bundled asset's relative path and contents. The build appends that fingerprint to the embedded index as an HTML comment. Added, changed, renamed, and removed assets are detected automatically, including presentation packs. If the compiled UI is stale, it cleans and rebuilds only the daemon package; preparation fails if the rebuilt UI still differs. It generates `src-tauri/tauri.bundle.conf.json`; those outputs are ignored by Git. Run it with `--version VERSION` for a release and `--target TRIPLE` for cross-compilation. The plugin builder runs on the native host even for a cross-compiled package.

`just bitcraft-dev` and `just bitcraft-dev-local` use `prepare.py --dev`. Development preparation selects binaries from the root workspace, matching the feature unification used by `just dev-local`, and omits `--target` to reuse native release artifacts. The launcher binaries use their desktop workspace's native debug artifacts, matching the desktop dev profile. Staged filenames keep the native target suffix. The first build with these settings may compile dependencies; later launches reuse them. Different versions, profiles and host/build dependencies can still require separate compilations. Installer preparation keeps its target-specific release builds; `--dev` supports only the native host target.

The daemon bundles runtime Markdown files alongside other UI assets. Only the named contributor docs in `services/dobjd/ui-excluded-files.txt` are excluded; the bundler and preparation check share that list.

The startup screen covers cryptography preparation, starter pocketknife
creation, and network confirmation. Inventory opens once a live pocketknife
is available. Automatic setup jobs are hidden; manual jobs remain visible.
Setup failures offer retry; the inventory stays closed until a live pocketknife exists.

To inspect the splash without waiting for a new pocketknife or resetting
inventory, fully quit Bitcraft and run `just bitcraft-shell`. It keeps the
desktop startup page open without launching a daemon. Existing live knives
skip the crafting splash during a normal launch, and warm cryptography
startup can finish before the desktop page is noticeable. Closing a window
to the tray does not quit the application; a second launch reopens the
existing instance.

For browser behavior tests, run `pnpm --dir apps/bitcraft/tests install --frozen-lockfile`
then `pnpm --dir apps/bitcraft/tests test`. For a visual preview without building
the desktop or generating proofs, run `pnpm --dir apps/bitcraft/tests preview`
and open `http://127.0.0.1:4171`. Its links demonstrate
first launch, existing inventory, and setup failure against a simulated daemon.
Preview data stays in memory and never reaches the network services.

The desktop Cargo workspace is separate from the headless workspace to keep GTK/WebKit out of service-only builds. It contains the Tauri host and the `crates/launchers` package, which has no GUI dependencies. It uses the repository toolchain and commits its own lockfile. Both workspaces participate in the same product build.

```sh
cargo fmt --manifest-path apps/bitcraft/src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path apps/bitcraft/src-tauri/Cargo.toml --workspace --locked --tests --examples -- -D warnings
cargo test --manifest-path apps/bitcraft/src-tauri/Cargo.toml --workspace --locked --release
cargo test --release --locked -p dobjd --features bundled-bitcraft-ui ui::tests
```

The host accepts `--background`, `--no-tray`, `--home PATH`, and `--no-runtime`. Closing the window or pressing Cmd-W hides it when a tray is available. The tray's Open action and a second foreground launch restore it. On macOS, hiding switches to accessory activation policy so the app leaves Cmd-Tab. Linux checks for a registered StatusNotifier host and restores the window if the host disappears. With no tray, closing the window exits normally. Explicit Quit exits the host and stops its child daemon. On macOS the menu bar also has an Edit menu, since a webview gets Cmd-C, Cmd-V and the other editing shortcuts only from menu items that carry them; the game's import sheet relies on it for pasting.

The host launches the daemon with `--exit-on-stdin-close` and retains its stdin pipe. If the host crashes or is force-quit, EOF makes the daemon exit even during initialization or circuit warm-up. Launch waits up to three seconds for the HTTP port to become available; this wait runs off the UI thread and never terminates another process. A stale connection file fails verification until a new daemon becomes ready and replaces it. This is immediate termination, with the same in-flight-job limitation as explicit Quit.

Linux lifecycle verification uses `cargo build --manifest-path apps/bitcraft/src-tauri/Cargo.toml --release --locked` followed by `xvfb-run -a dbus-run-session /usr/bin/python3 apps/bitcraft/scripts/smoke_linux.py`. Install `xvfb dbus-x11 xdotool x11-utils python3-dbus python3-gi` first. The test exercises background launch, native tray actions, close-to-tray, second-launch forwarding, tray-host loss, and ordinary close without a host. It runs with `--no-runtime` and checks that no game state was created.

The Bitcraft CI workflow also runs a Linux packaging job: it prepares the full runtime, builds an unsigned AppImage, and runs `smoke_bundle_linux.py` against the extracted artifact without requiring FUSE. The smoke check verifies the packaged plugin and executable tools, starts the managed daemon in a temporary home with loopback-only backend settings, checks the UI and CLI wrapper, and runs the tray/window lifecycle checks using the packaged app. It does not generate proofs or submit transactions. The AppImage is retained as a CI artifact for seven days. macOS and Windows have native desktop checks on pull requests; their installer checks run in the release workflow described below.

## State and compatibility

Bitcraft stores writable data in a stable directory across app releases:

```text
~/.bitcraft/
  actions/bitcraft.pexe
  actions/archive/
  objects/
  settings.json
  connection.json
  dobjd.log
```

`--home` overrides the default root; the host and wrappers also accept `BITCRAFT_HOME`, with an explicit `--home` taking precedence. This directory is also the managed daemon's `DOBJ_HOME`. Changing the app version or bundled binaries does not select a new data directory. Objects, settings, and other installed actions remain in place across updates. The release manifest records version, source commit, and artifact hashes; it does not determine the data location. Generic `~/.dobj` state is untouched.

The app manages `actions/bitcraft.pexe`. Before launch it verifies the bundled plugin against the release manifest and atomically installs it if its contents differ from the installed copy. Replaced copies are retained as `actions/archive/bitcraft-<sha256>.pexe`; the action loader does not scan this subdirectory. Local edits to the managed file are archived and replaced too. Other installed actions are preserved. The archive retains previous plugin bytes; loading multiple versions or making older objects compatible with changed actions still requires explicit compatibility support.

Bundled settings are copied only when `settings.json` does not exist. The initial configuration uses the same hosted synchronizer and relayer as the existing Digital Objects releases; MCP starts disabled. Changes to bundled defaults do not overwrite existing settings. For local-network development, use a separate `--home` directory and configure its settings. Backend compatibility still needs release validation; these URLs alone do not establish a versioned backend contract.

The installer includes `bitcraft-dobjd`, `bitcraft-dobj`, `bitcraft-dobj-mcp-proxy`, and `bitcraft-pexe`. The names avoid collisions with a standalone Digital Objects installation. Bundling does not add commands to PATH or configure AI clients.

`bitcraft-dobj` and `bitcraft-dobj-mcp-proxy` are Rust wrappers around private `-inner` binaries beside them in the app bundle. Once the daemon is ready, the host atomically publishes `<home>/connection.json` with its HTTP and MCP URLs, active state directory, and launch identity. The wrappers verify that identity, supply `--url` and `DOBJ_HOME`, and preserve arguments, stdin/stdout/stderr, and the child exit status. They use explicit sibling paths and never search PATH or fall back to the standalone Digital Objects endpoint. Normal shutdown removes the connection file; a stale file fails the identity/readiness check.

Run these commands using the installed wrappers' full paths until PATH integration is added:

```sh
bitcraft-dobj objects
bitcraft-dobj settings set --mcp on
bitcraft-dobj-mcp-proxy

# Custom homes are discovered from the working directory (or its ancestors).
cd /path/to/bitcraft-data
bitcraft-dobj objects

# Select a different home explicitly, from any directory.
bitcraft-dobj --home /path/to/bitcraft-data objects
bitcraft-dobj-mcp-proxy --home /path/to/bitcraft-data
```

Choose **Claude** or **Codex** in the game's toolbar to open the installed desktop app with the Bitcraft home directory selected and a player prompt prefilled. Claude opens its Code session using `claude://code/new` with `folder` and `q`; Codex opens a local workspace using `codex://new` with `path` and `prompt`. The user sends the prefilled prompt; Claude also asks them to confirm the folder. These links require an installed desktop app that supports them. The buttons appear only inside the Bitcraft desktop app.

The Codex button uses the familiar ChatGPT/OpenAI logo. Before handing off, Bitcraft checks the registered URL handler using NSWorkspace on macOS, Windows Launcher URI support on Windows, and GIO application associations on Linux. Launching runs off the UI thread. If no app is registered or the launch fails, Bitcraft opens **Play with agents...** with the error and a copyable prompt. The same guide is available from the app and tray menus for other desktop agents or CLI tools. It uses the game's fonts, colors and form styling, and works before the game daemon is ready. A registered handler does not establish that an older app supports every deep-link parameter.

The prompt's text lives in `AGENT_PROMPT.md` in this folder; edit it there. It is built into the desktop app, so rebuild (for example with `just bitcraft-dev`) to see a change. `{skill}`, `{cli}` and `{home}` in it are replaced with the installed player skill, the installed CLI and the game's home directory. The prompt points to the bundled `runtime/skills/bitcraft-play/SKILL.md` and installed CLI. The selected directory is the app's actual home, including custom homes and `.dev/bitcraft-local`. The wrappers search the working directory and its ancestors for `connection.json`, then fall back to `~/.bitcraft`; explicit `--home` and `BITCRAFT_HOME` take precedence. Invalid or stale discovered connections fail rather than silently choosing another game. Commands run from the selected home need no `--home` argument. No checkout or permanent agent configuration is needed. Packaging includes the player skill alongside the runtime resources.

The skill leads with CLI discovery and long-running shell commands. The CLI exposes inventory fields, recipe outputs and arguments, and `runs list/show/wait` for recovering an interrupted connection without another submission. `run --json` and `runs wait --json` return the final run state on stdout, progress on stderr, and nonzero exit status for a failed run. For agents without local shell access, enable MCP explicitly with `bitcraft-dobj settings set --mcp on` and configure the agent to connect to `http://127.0.0.1:7728/mcp`. The toolbar buttons do not enable MCP or change the agent's configuration.

MCP clients can launch the proxy with no port arguments once MCP is enabled in the running app. `--home PATH` must precede tool arguments. Help/version work without a running app. Endpoint overrides are rejected, as are the CLI's standalone `start`, `stop`, `status`, `logs`, `ui`, and `update` commands, whose pid-file and installation assumptions do not match the desktop host. Manage the daemon through Bitcraft. `bitcraft-pexe` remains the packaging tool; it is not a connection-aware wrapper.

## Discord Rich Presence

The native host can share Bitcraft activity with the Discord desktop client over local IPC. Sharing is off by default. Turn it on with **Share my Bitcraft scores on Discord** in the Settings window (the gear in the game's top bar). The preference is stored in `<home>/desktop.json`, separate from daemon settings, and follows `--home` and `BITCRAFT_HOME`. Turning it off clears presence immediately; quitting clears presence before stopping the daemon. Closing to the tray keeps presence active. Shell-only launches never connect to Discord.

Presence shows the current Bitcraft action (or a summary for concurrent jobs), Total Inventory Value in BIT, the five-minute production rate in BIT/sec, active job count, and elapsed session time, with links to the website and community Discord. The total includes the value of resources and tools, rather than just counting Bit objects. Both numeric values use the same calculation as the main window's toolbar, and appear once the native host has loaded its inventory. Presence includes zero and negative production rates. Individual objects, file paths, and errors are not published. Discord's own activity-sharing settings control who can see it. The [Bitcraft Discord bot](discord-bot/README.md) reads Total Inventory Value from this presence and publishes each player's highest on a public leaderboard. The trophy in the game's top bar opens that leaderboard.

To activate this integration for development or a release:

1. Create a **Bitcraft** application in the [Discord Developer Portal](https://discord.com/developers/applications). No bot user, bot token, client secret, or OAuth redirect is needed.
2. Copy its numeric **Application ID** from General Information.
3. In the application's Rich Presence art assets, upload `src-tauri/icons/icon.png` with the asset key **bitcraft**. Set the application icon as well.
4. Set `BITCRAFT_DISCORD_APPLICATION_ID` to that ID when building the Tauri host, including release builds. For a local launch, for example:

   ```sh
   BITCRAFT_DISCORD_APPLICATION_ID=YOUR_NUMERIC_APPLICATION_ID just bitcraft-dev
   ```

   For GitHub release builds, create a repository Actions variable named `BITCRAFT_DISCORD_APPLICATION_ID` with that value under **Settings > Secrets and variables > Actions > Variables**. The `Release Bitcraft` workflow supplies it to every installer build, including manual build-only runs. The public Application ID does not need to be a secret.

   The ID is public and is embedded at compile time. The same environment variable can override it at runtime. Blank runtime values fall back to the compiled ID; blank compiled values leave the integration unconfigured. The sharing preference can always be changed. Without a valid ID, no Discord connection is attempted; the saved preference takes effect when an ID is configured.

5. Run the Discord desktop app and enable sharing in Bitcraft's Settings. Check the activity card on your Discord profile and verify disabling sharing removes it. A browser-only Discord session cannot receive local IPC.

The native host keeps inventory metrics whether or not a window is open or sharing is on. It follows the daemon's inventory, values it with `ui/inventory-values.json` (the table the game pages use), and keeps five minutes of history, so BIT/sec stays current while the window is hidden. The main game window takes BIT/sec from the host through the `inventory_metrics` command and computes everything else itself. When the host loses track of the inventory, it starts a fresh rate history rather than counting changes it did not observe as production.

Presence follows the daemon's active runs, sends only changed payloads, and limits how often it updates. A brief daemon outage keeps the last activity; a sustained one clears it. Losing Discord does not affect the game, and presence returns when Discord does, without toggling sharing. No Discord account linking or hosted service is required.

The transport uses Discord's documented [RPC over IPC protocol](https://docs.discord.com/developers/topics/rpc), with Unix sockets on macOS/Linux and named pipes on Windows. The tests use a simulated Discord, so a real profile display still needs the application setup above and a native smoke check on each supported OS.

## Releases and updates

The repository's `Release` workflow builds Bitcraft installers alongside the standalone Digital Objects artifacts for each `vMAJOR.MINOR.PATCH[-rc.N | -alpha.N | -beta.N]` tag. It calls `Release Bitcraft`, which builds macOS Apple Silicon and Intel DMGs, a Windows x86_64 per-user NSIS installer, and a Linux x86_64 AppImage. Product versions are derived from the tag; packaged runtime manifests are checked against that version, target, and source commit. Manual releases check out the requested tag rather than the workflow's selected branch. All packages upload to the same draft GitHub release, so standalone installers and update checks continue to find their expected artifacts through the repository's latest release.

Release macOS builds reuse the existing Apple certificate and App Store Connect secrets. Tauri signs the app and bundled executables, notarizes the app, and staples its ticket. The release check also notarizes and staples the DMG, then mounts it to check the packaged runtime, signatures, and tickets. Windows checks silently install NSIS into a temporary directory and execute the installed tools. Linux checks extract and run the AppImage, including the managed daemon and tray lifecycle. Verified installers are also retained as CI artifacts for seven days.

For a build-only test, manually run `Release Bitcraft` on the branch to test and supply a version tag label. That label need not exist as a Git tag: the selected workflow commit is built. Manual runs produce unsigned installers as CI artifacts, use no Apple signing secrets, and do not create or modify a GitHub release. The main release workflow calls it with publishing enabled. The workflow files must be available on the default branch before GitHub exposes manual dispatch in the Actions UI.

Tauri's updater plugin is registered in the Rust host. When both `BITCRAFT_UPDATE_ENDPOINT` and `BITCRAFT_UPDATE_PUBLIC_KEY` are supplied, preparation generates updater configuration and enables signed updater artifacts. The endpoint must use HTTPS. Tauri's build also needs `TAURI_SIGNING_PRIVATE_KEY` and, if applicable, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. The signing key must correspond to the embedded public key. See [Tauri updater documentation](https://v2.tauri.app/plugin/updater/).

The public updater feed, native update controls, scheduled checks, safe daemon shutdown, and update installation are not implemented yet. Registering the plugin and producing artifacts do not turn on automatic updates. Storage-format compatibility checks and migrations must handle incompatible changes explicitly. Downgrades need their own compatibility policy; retaining previous action archives alone does not make an older runtime able to read newer data.

Production macOS bundles use Developer ID signing, hardened runtime, notarization, and stapling through the release workflow. Windows Authenticode signing still needs credentials and configuration. OS signing and updater signatures are separate; no public updater feed or updater signing key is configured here. Actual packaged tray behaviour on macOS and Windows still needs native validation.

The current Quit path terminates the child process. Safe quiescence of active proofs and pending submissions must precede update installation. Runtime monitoring after startup, log rotation, launch at login, custom MCP docs, and command-line integration are subsequent work.
