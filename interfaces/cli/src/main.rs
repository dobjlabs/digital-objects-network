//! `dobj` — terminal client for `dobjd`.
//!
//! Thin HTTP wrapper around the same `dobjd` HTTP server that powers the
//! desktop GUI, the website, and the MCP transport. Run `dobjd` first; this
//! CLI talks to it.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

mod client;
mod commands;
mod daemon;
mod update;

use client::DobjdClient;

const DEFAULT_URL: &str = "http://127.0.0.1:7717";

/// Release tag + target triple, stamped by build.rs ("dev" outside a
/// release build).
const VERSION: &str = concat!(
    env!("DOBJ_RELEASE_TAG"),
    " (",
    env!("DOBJ_TARGET_TRIPLE"),
    ")"
);

#[derive(Parser)]
#[command(
    name = "dobj",
    about = "Talk to your local dobjd driver process",
    version = VERSION
)]
struct Cli {
    /// Base URL of the dobjd HTTP server. Defaults to http://127.0.0.1:7717
    /// (the dobjd default), or `DOBJD_URL` if set.
    #[arg(long, global = true, env = "DOBJD_URL", default_value = DEFAULT_URL)]
    url: String,

    /// Emit machine-readable JSON instead of human output where applicable.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show every object the driver knows about.
    Objects,
    /// Show every action the action catalog exposes.
    Actions,
    /// Show every class the action catalog defines.
    Classes,
    /// Inspect a single object by its `.dobj` file name.
    InspectObject {
        /// The `.dobj` basename in `~/.dobj/objects/` (e.g.
        /// `wood_0xabc….dobj`). See `dobj objects`.
        file_name: String,
    },
    /// Inspect a single class (with predicate source).
    InspectClass {
        /// Class to inspect, e.g. `craft-basics::Wood`. Run `dobj classes`
        /// for the available list.
        #[arg(value_name = "PLUGIN::CLASS")]
        qualified_id: String,
    },
    /// Inspect a single action (with predicate source).
    InspectAction {
        /// Action to inspect, e.g. `craft-basics::CraftWoodPick`. Run
        /// `dobj actions` for the available list.
        #[arg(value_name = "PLUGIN::ACTION")]
        qualified_id: String,
    },
    /// Check whether an action can run with the current objects.
    Feasibility {
        /// Action to check, e.g. `craft-basics::CraftWoodPick`. Run
        /// `dobj actions` for the available list.
        #[arg(value_name = "PLUGIN::ACTION")]
        qualified_id: String,
    },
    /// Print the current state root.
    StateRoot,
    /// Print the local objects directory path (`~/.dobj/objects/`).
    ObjectsDir,
    /// Import an external `.dobj` file into your objects.
    Import {
        /// Path to the `.dobj` file to import. Its contents are read and sent
        /// to dobjd, which files the object under a canonical name.
        path: PathBuf,
    },
    /// Install a plugin (`.pexe`) from a local path or URL, then hot-reload
    /// the daemon's action catalog so it's usable right away.
    Install {
        /// Path to a `.pexe` file, or an http(s) URL to download one from.
        source: String,
    },
    /// Read or write daemon settings (synchronizer / relayer URLs, MCP
    /// toggle).
    #[command(subcommand)]
    Settings(SettingsCmd),
    /// Execute an action. Streams progress to stderr; result on stdout.
    Run {
        /// Action to execute, e.g. `craft-basics::CraftWoodPick`. Run
        /// `dobj actions` for the available list.
        #[arg(value_name = "PLUGIN::ACTION")]
        qualified_id: String,
        /// Input object filenames or paths. Filenames must exist in
        /// `~/.dobj/objects/` (the driver looks them up by basename).
        inputs: Vec<String>,
        /// Don't print per-step progress messages.
        #[arg(long)]
        quiet: bool,
    },
    /// Stream every dobjd event as JSON lines until interrupted.
    Events,
    /// Start dobjd in the background. Idempotent — safe to run when
    /// dobjd is already up.
    Start,
    /// Stop the dobjd process started by `dobj start`.
    Stop,
    /// Show whether dobjd is running and reachable.
    Status,
    /// Print the dobjd log. Defaults to the last 100 lines.
    Logs {
        /// Follow the log as it grows (Ctrl+C to stop).
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to print from the end of the log.
        #[arg(short = 'n', long, default_value_t = 100)]
        lines: usize,
    },
    /// Update dobj, dobjd, and dobj-mcp-proxy to a newer release.
    Update {
        /// Report current and latest versions without changing anything.
        #[arg(long)]
        check: bool,
        /// Target a specific release tag instead of the latest.
        #[arg(long, value_name = "TAG")]
        version: Option<String>,
        /// Proceed when the target is not newer (reinstall or downgrade).
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// Print current settings.
    Get,
    /// Update any subset of settings. Omitted flags are left unchanged.
    Set {
        #[arg(long)]
        synchronizer: Option<String>,
        #[arg(long)]
        relayer: Option<String>,
        /// Serve MCP on the port adjacent to the HTTP port (on/off).
        /// Takes effect immediately.
        #[arg(long, value_name = "ON|OFF", value_parser = clap::builder::BoolishValueParser::new())]
        mcp: Option<bool>,
    },
}

/// Run a daemon command, then surface the "a newer release is available"
/// notice on success only. The update flow restarts dobjd via `daemon::start`
/// directly (not through this path), so a post-update restart never prints a
/// stale notice against the old CLI binary still doing the update.
async fn with_update_notice(result: Result<()>) -> Result<()> {
    if result.is_ok() {
        update::notify_if_outdated().await;
    }
    result
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = DobjdClient::new(cli.url);

    match cli.cmd {
        Cmd::Objects => commands::objects(&client, cli.json).await,
        Cmd::Actions => commands::actions(&client, cli.json).await,
        Cmd::Classes => commands::classes(&client, cli.json).await,
        Cmd::InspectObject { file_name } => {
            commands::inspect_object(&client, file_name, cli.json).await
        }
        Cmd::InspectClass { qualified_id } => {
            commands::inspect_class(&client, qualified_id, cli.json).await
        }
        Cmd::InspectAction { qualified_id } => {
            commands::inspect_action(&client, qualified_id, cli.json).await
        }
        Cmd::Feasibility { qualified_id } => {
            commands::feasibility(&client, qualified_id, cli.json).await
        }
        Cmd::StateRoot => commands::state_root(&client).await,
        Cmd::ObjectsDir => commands::objects_dir(&client).await,
        Cmd::Import { path } => commands::import(&client, path, cli.json).await,
        Cmd::Install { source } => commands::install(&client, source, cli.json).await,
        Cmd::Settings(SettingsCmd::Get) => commands::settings_get(&client, cli.json).await,
        Cmd::Settings(SettingsCmd::Set {
            synchronizer,
            relayer,
            mcp,
        }) => commands::settings_set(&client, synchronizer, relayer, mcp).await,
        Cmd::Run {
            qualified_id,
            inputs,
            quiet,
        } => commands::run(&client, qualified_id, inputs, quiet).await,
        Cmd::Events => commands::events(&client).await,
        Cmd::Start => with_update_notice(daemon::start(&client).await).await,
        Cmd::Stop => daemon::stop().await,
        Cmd::Status => with_update_notice(daemon::status(&client).await).await,
        Cmd::Logs { follow, lines } => daemon::logs(follow, lines).await,
        Cmd::Update {
            check,
            version,
            force,
        } => update::run(&client, check, version, force).await,
    }
}
