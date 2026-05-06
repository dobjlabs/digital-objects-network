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
mod types;

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
    /// Inspect a single object by its content-addressed id.
    InspectObject {
        /// Hex-encoded object id (e.g. `0xabc…`).
        id: String,
    },
    /// Inspect a single class by name (with predicate source).
    InspectClass {
        /// Class name (e.g. `Wood`, `WoodPick`). See `dobj classes`.
        name: String,
    },
    /// Check whether an action can run with the current inventory.
    Feasibility {
        /// Action id to check (e.g. `CraftWood`). See `dobj actions`.
        action_id: String,
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
        /// Action id (e.g. `FindLog`, `CraftWoodPick`). See `dobj actions`.
        action_id: String,
        /// Input object filenames or paths. Filenames must exist in
        /// `~/.dobj/objects/` (the driver looks them up by basename).
        inputs: Vec<String>,
        /// Don't print per-step progress messages.
        #[arg(long)]
        quiet: bool,
    },
    /// Stream every dobjd event (object changes, action progress, MCP
    /// activity) as JSON lines until interrupted. This is the broadcast
    /// hub every client shares — useful for seeing what the desktop, web,
    /// or MCP are doing in real time.
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
        Cmd::InspectObject { id } => commands::inspect_object(&client, id, cli.json).await,
        Cmd::InspectClass { name } => commands::inspect_class(&client, name, cli.json).await,
        Cmd::Feasibility { action_id } => commands::feasibility(&client, action_id, cli.json).await,
        Cmd::StateRoot => commands::state_root(&client).await,
        Cmd::ObjectsDir => commands::objects_dir(&client).await,
        Cmd::Settings(SettingsCmd::Get) => commands::settings_get(&client, cli.json).await,
        Cmd::Settings(SettingsCmd::Set {
            synchronizer,
            relayer,
        }) => commands::settings_set(&client, synchronizer, relayer).await,
        Cmd::Run {
            action_id,
            inputs,
            quiet,
        } => commands::run(&client, action_id, inputs, quiet).await,
        Cmd::Events => commands::events(&client).await,
        Cmd::Start => daemon::start(&client).await,
        Cmd::Stop => daemon::stop().await,
        Cmd::Status => daemon::status(&client).await,
        Cmd::Logs { follow, lines } => daemon::logs(follow, lines),
    }
}
