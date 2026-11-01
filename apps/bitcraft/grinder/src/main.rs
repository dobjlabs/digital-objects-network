use std::{num::NonZeroUsize, path::PathBuf, time::Instant};

use anyhow::{Context, Result, bail};
use bitcraft_grind::{
    find_args, grind, hints_path, load_plugin, plugin_path, read_hints, read_input_object,
};
use clap::Parser;
use wire_types::QualifiedName;

/// Runs the proof-of-work search a Bitcraft action needs. Prints on stdout
/// every argument that lets the action skip its own search, one NAME=JSON
/// line each, ready for `bitcraft-dobj run ACTION ... --arg`, and reports
/// progress on stderr.
#[derive(Parser)]
#[command(name = "bitcraft-grind", version)]
struct Args {
    /// The Bitcraft home directory. Without it: BITCRAFT_HOME, then the
    /// nearest directory holding a connection.json at or above the current
    /// one, then ~/.bitcraft.
    #[arg(long)]
    home: Option<PathBuf>,
    /// Search threads [default: half the available cores].
    #[arg(long)]
    threads: Option<NonZeroUsize>,
    /// Stop searching once stdin reaches end of file, so the search ends
    /// with the process that started it.
    #[arg(long)]
    exit_on_stdin_close: bool,
    /// An input object of the action, named as for `bitcraft-dobj run`.
    /// Repeat it for each input, in the same order.
    #[arg(long = "input", value_name = "FILE")]
    inputs: Vec<String>,
    /// The action, such as bitcraft::GatherWood.
    action: String,
}

fn resolve_home(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if explicit
        .as_ref()
        .is_some_and(|path| path.as_os_str().is_empty())
    {
        bail!("--home requires a non-empty path");
    }
    let current = std::env::current_dir()?;
    let home = match explicit.or_else(|| {
        std::env::var_os("BITCRAFT_HOME")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    }) {
        Some(home) => home,
        None => match current.ancestors().find(|ancestor| {
            ancestor
                .join("connection.json")
                .try_exists()
                .unwrap_or(false)
        }) {
            Some(home) => home.to_path_buf(),
            None => dirs::home_dir()
                .context("Cannot resolve the home directory")?
                .join(".bitcraft"),
        },
    };
    Ok(if home.is_absolute() {
        home
    } else {
        current.join(home)
    })
}

fn default_threads() -> NonZeroUsize {
    let available = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    NonZeroUsize::new(available / 2).unwrap_or(NonZeroUsize::MIN)
}

fn run(args: Args) -> Result<()> {
    if args.exit_on_stdin_close {
        std::thread::spawn(|| {
            let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
            eprintln!("bitcraft-grind: stdin closed; stopping");
            std::process::exit(1);
        });
    }
    let action = QualifiedName::parse(&args.action).map_err(anyhow::Error::msg)?;
    let home = resolve_home(args.home)?;
    let actions_dir = home.join("actions");
    let hints = read_hints(&hints_path(&actions_dir, &action.plugin_name))?;
    let hint = hints
        .get(&args.action)
        .with_context(|| format!("{} needs no proof-of-work search", args.action))?;
    let inputs = args
        .inputs
        .iter()
        .map(|path| read_input_object(&home.join("objects"), path))
        .collect::<Result<Vec<_>>>()?;
    let module = load_plugin(
        &plugin_path(&actions_dir, &action.plugin_name),
        &action.plugin_name,
    )?;
    let threads = args.threads.unwrap_or_else(default_threads);
    let qualified = args.action.clone();
    let found = find_args(
        &module,
        &action.name,
        hint,
        inputs,
        move |object, field, target| {
            eprintln!(
                "Searching for a {} for {qualified} on {threads} threads",
                field.name()
            );
            let started = Instant::now();
            let result = grind(object, field, target, threads, |attempts| {
                eprintln!(
                    "Tried {attempts} values in {}s",
                    started.elapsed().as_secs()
                );
            });
            eprintln!(
                "Found one after about {} attempts in {:.1}s",
                result.attempts,
                started.elapsed().as_secs_f64()
            );
            result.value
        },
    )?;
    for (name, value) in &found {
        println!("{name}={}", serde_json::to_string(value)?);
    }
    Ok(())
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("bitcraft-grind: {error:#}");
        std::process::exit(1);
    }
}
