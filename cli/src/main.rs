//! `dobj` — terminal client for `dobjd`.
//!
//! Thin HTTP wrapper around the same `dobjd` HTTP server that powers the
//! desktop GUI, the website, and the MCP transport. Run `dobjd` first; this
//! CLI talks to it.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod client;
mod commands;
mod daemon;

use client::DobjdClient;

const DEFAULT_URL: &str = "http://127.0.0.1:7717";

#[derive(Parser)]
#[command(
    name = "dobj",
    about = "Talk to your local dobjd driver process",
    version
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
    Inventory,
    /// Show every action the action catalog exposes.
    Actions,
    /// Show every class the action catalog defines.
    Classes,
    /// Inspect a single object by its `.dobj` file name.
    InspectObject {
        /// The `.dobj` basename in `~/.dobj/objects/` (e.g.
        /// `wood_0xabc….dobj`). See `dobj inventory`.
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
    /// Check whether an action can run with the current inventory.
    Feasibility {
        /// Action to check, e.g. `craft-basics::CraftWoodPick`. Run
        /// `dobj actions` for the available list.
        #[arg(value_name = "PLUGIN::ACTION")]
        qualified_id: String,
    },
    /// Print the current global state root.
    StateRoot,
    /// Print the local objects directory path (`~/.dobj/objects/`).
    ObjectsDir,
    /// Read or write driver settings (synchronizer / relayer URLs).
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
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// Print current settings.
    Get,
    /// Update one or both URLs. Omitted flags are left unchanged.
    Set {
        #[arg(long)]
        synchronizer: Option<String>,
        #[arg(long)]
        relayer: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = DobjdClient::new(cli.url);

    match cli.cmd {
        Cmd::Inventory => commands::inventory(&client, cli.json).await,
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
        Cmd::Settings(SettingsCmd::Get) => commands::settings_get(&client, cli.json).await,
        Cmd::Settings(SettingsCmd::Set {
            synchronizer,
            relayer,
        }) => commands::settings_set(&client, synchronizer, relayer).await,
        Cmd::Run {
            qualified_id,
            inputs,
            quiet,
        } => commands::run(&client, qualified_id, inputs, quiet).await,
        Cmd::Events => commands::events(&client).await,
        Cmd::Start => daemon::start(&client).await,
        Cmd::Stop => daemon::stop().await,
        Cmd::Status => daemon::status(&client).await,
        Cmd::Logs { follow, lines } => daemon::logs(follow, lines).await,
    }
}
