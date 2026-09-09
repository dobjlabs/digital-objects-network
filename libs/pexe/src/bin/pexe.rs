//! `pexe`: build and install plugin archives.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use pexe::{
    DEFAULT_OUT_DIR, MANIFEST_FILE, PEXE_EXTENSION, PluginSource, default_install_dir,
    dep_search_dirs, import_build_order, inspect, install, pack, read_pexe_file,
    resolve_manifest_imports, set_manifest_hash, set_manifest_import_hash, unpack,
};
use pod2::middleware::Hash;
use sdk::{ImportLookup, Sdk, manifest::Manifest};

/// Release tag + target triple, stamped by build.rs ("dev" outside a release
/// build). pexe ships in the same release bundle as dobj/dobjd and `dobj
/// update` validates every bundled binary reports the release tag, so its
/// `--version` must read the same stamp.
const VERSION: &str = concat!(
    env!("DOBJ_RELEASE_TAG"),
    " (",
    env!("DOBJ_TARGET_TRIPLE"),
    ")"
);

#[derive(Parser, Debug)]
#[command(name = "pexe", about = "plugin packaging tool", version = VERSION)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Build a .pexe from a plugin source directory.
    Build {
        /// One or more plugin source directories (each must contain manifest.toml
        /// and plugin.rhai).
        #[arg(required = true)]
        plugins: Vec<PathBuf>,

        /// Output directory for the built .pexe files.
        #[arg(long, default_value = DEFAULT_OUT_DIR)]
        out_dir: PathBuf,

        /// Also install the built archives into the target install dir.
        #[arg(long)]
        install: bool,

        /// Override the install directory (default: ~/.dobj/actions).
        #[arg(long)]
        install_dir: Option<PathBuf>,

        /// Don't rewrite the source manifest.toml when module_hash mismatches;
        /// fail instead.
        #[arg(long)]
        check: bool,

        /// Search these directories for built dependencies before the output
        /// and install directories. Dependencies must be built first.
        #[arg(long)]
        deps: Vec<PathBuf>,
    },
    /// Dump the contents of a .pexe archive to stdout.
    Dump {
        /// Path to the .pexe file.
        pexe: PathBuf,
    },
    /// Inspect a plugin's predicates, classes, or action graph.
    Inspect {
        #[command(subcommand)]
        cmd: InspectCmd,
    },
}

#[derive(clap::Args, Debug)]
struct InspectTarget {
    /// Path to a `.pexe` archive or a plugin source directory
    /// (containing `manifest.toml` and `plugin.rhai`).
    target: PathBuf,

    /// Search these directories for imported plugins before the build output
    /// and install directories. Archives must match their manifest pins.
    #[arg(long)]
    deps: Vec<PathBuf>,
}

impl InspectTarget {
    fn as_inspect(&self) -> inspect::Target<'_> {
        inspect::Target::new(&self.target, &self.deps)
    }
}

#[derive(Subcommand, Debug)]
enum InspectCmd {
    /// Render the Podlang for the plugin's predicates.
    ///
    /// Default form is the SDK-synthesized frontend Podlang. With
    /// `--middleware`, the compiled `CustomPredicateBatch` is rendered
    /// instead via pod2's pretty-printer.
    Predicates {
        #[command(flatten)]
        target: InspectTarget,

        /// Restrict output to a single predicate name. Without this,
        /// every predicate is emitted with a `--- name ---` header.
        #[arg(long)]
        action: Option<String>,

        /// Render the compiled middleware form rather than the
        /// SDK-synthesized frontend form.
        #[arg(long)]
        middleware: bool,
    },
    /// Render each class's state-space signature.
    Classes {
        #[command(flatten)]
        target: InspectTarget,

        /// Restrict output to a single class.
        class: Option<String>,
    },
    /// Emit the action/class relationship graph.
    Graph {
        #[command(flatten)]
        target: InspectTarget,

        /// Output format. `dot` (default) emits Graphviz; `mermaid`
        /// emits a Mermaid flowchart that pastes into mermaid.live or
        /// renders inline in GitHub markdown.
        #[arg(long, value_enum, default_value_t = GraphFormat::Dot)]
        format: GraphFormat,

        /// Only meaningful with `--format mermaid`. Emit a mermaid.live
        /// URL instead of the raw source so the graph can be opened in
        /// a browser with one click.
        #[arg(long)]
        link: bool,
    },
    /// Mint synthetic inputs for an action and generate a real
    /// plonky2 proof. Much slower than `plan` (uses the real prover,
    /// not MockProver) and produces a verifiable MainPod.
    Prove {
        #[command(flatten)]
        target: InspectTarget,

        /// Action to prove.
        #[arg(long)]
        action: String,

        /// Seed the RNG used by fixture minting and `action.random()`
        /// for reproducible output (commitments, tx_final, proof
        /// bytes). Default uses OS entropy.
        #[arg(long)]
        seed: Option<u64>,
    },
    /// Mint synthetic inputs for an action and run it in mock mode so
    /// the SDK's multi-pod solver runs. Prints the solution breakdown
    /// and a statement dependency graph.
    Plan {
        #[command(flatten)]
        target: InspectTarget,

        /// Action to plan.
        #[arg(long)]
        action: String,

        /// Output format. `text` (default) prints the breakdown plus
        /// an indented dep listing. `dot` emits only a Graphviz digraph
        /// of the statement DAG, clustered by POD.
        #[arg(long, value_enum, default_value_t = PlanFormat::Text)]
        format: PlanFormat,

        /// Only meaningful with `--format mermaid` / `mermaid-full`.
        /// Emit a mermaid.live URL instead of the raw source so the
        /// graph can be opened in a browser with one click.
        #[arg(long)]
        link: bool,

        /// Restrict the `text` format to specific sections. Comma-
        /// separated list of: `header`, `summary`, `totals`, `deps`,
        /// `all`. Default is `all` (full output). Has no effect on
        /// non-text formats.
        #[arg(long, value_delimiter = ',', value_enum)]
        show: Vec<PlanSection>,

        /// Seed the RNG used by fixture minting and `action.random()`
        /// for reproducible structural output (statement indices,
        /// commitments shown in the dep graph). Default uses OS
        /// entropy.
        #[arg(long)]
        seed: Option<u64>,
    },
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PlanSection {
    Header,
    Summary,
    Totals,
    Deps,
    All,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum GraphFormat {
    Dot,
    Mermaid,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum PlanFormat {
    Text,
    /// Graphviz DOT, compressed: only Custom and Intro predicate
    /// nodes, with native plumbing folded into the consumer.
    Dot,
    /// Graphviz DOT, full: every Native statement included.
    DotFull,
    /// Mermaid flowchart, compressed. Embeds in GitHub markdown and
    /// pastes into mermaid.live for a shareable link.
    Mermaid,
    /// Mermaid flowchart, full.
    MermaidFull,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Build {
            plugins,
            out_dir,
            install: do_install,
            install_dir,
            check,
            deps,
        } => {
            std::fs::create_dir_all(&out_dir)
                .with_context(|| format!("failed to create {}", out_dir.display()))?;
            let target_install = if do_install {
                Some(match install_dir {
                    Some(p) => p,
                    None => default_install_dir()?,
                })
            } else {
                None
            };
            let dep_dirs = dep_search_dirs(&deps, &out_dir, target_install.as_deref());
            // Read all manifests before computing the dependency order.
            let mut sources = Vec::with_capacity(plugins.len());
            for plugin_dir in &plugins {
                let source = PluginSource::read(plugin_dir)?;
                let manifest = source.parse_manifest()?;
                sources.push((source, manifest));
            }
            let declared: Vec<(String, Vec<String>)> = sources
                .iter()
                .map(|(_, manifest)| {
                    (
                        manifest.plugin.name.clone(),
                        manifest
                            .imports
                            .iter()
                            .map(|import| import.name.clone())
                            .collect(),
                    )
                })
                .collect();
            for idx in import_build_order(&declared)? {
                let (source, manifest) = &sources[idx];
                build_one(
                    source,
                    manifest,
                    &out_dir,
                    target_install.as_deref(),
                    check,
                    &dep_dirs,
                )?;
            }
        }
        Cmd::Dump { pexe } => {
            let bytes = read_pexe_file(&pexe)?;
            let (manifest, script) = unpack(&bytes)?;
            println!("# manifest");
            println!("{:#?}", manifest);
            println!("\n# plugin.rhai");
            println!("{}", script);
        }
        Cmd::Inspect { cmd } => match cmd {
            InspectCmd::Predicates {
                target,
                action,
                middleware,
            } => {
                inspect::predicates(&target.as_inspect(), action.as_deref(), middleware)?;
            }
            InspectCmd::Classes { target, class } => {
                inspect::classes(&target.as_inspect(), class.as_deref())?;
            }
            InspectCmd::Graph {
                target,
                format,
                link,
            } => {
                let mode = match format {
                    GraphFormat::Dot => inspect::GraphOutput::Dot,
                    GraphFormat::Mermaid if link => inspect::GraphOutput::MermaidLink,
                    GraphFormat::Mermaid => inspect::GraphOutput::Mermaid,
                };
                inspect::graph(&target.as_inspect(), mode)?;
            }
            InspectCmd::Prove {
                target,
                action,
                seed,
            } => {
                if let Some(seed) = seed {
                    pod2utils::set_seed(seed);
                }
                inspect::prove_action(&target.as_inspect(), &action)?;
            }
            InspectCmd::Plan {
                target,
                action,
                format,
                link,
                show,
                seed,
            } => {
                if let Some(seed) = seed {
                    pod2utils::set_seed(seed);
                }
                use std::collections::BTreeSet;
                let sections: BTreeSet<inspect::PlanSection> = if show.is_empty() {
                    inspect::PlanSection::default_all().into_iter().collect()
                } else {
                    let mut s = BTreeSet::new();
                    for sec in show {
                        match sec {
                            PlanSection::Header => {
                                s.insert(inspect::PlanSection::Header);
                            }
                            PlanSection::Summary => {
                                s.insert(inspect::PlanSection::Summary);
                            }
                            PlanSection::Totals => {
                                s.insert(inspect::PlanSection::Totals);
                            }
                            PlanSection::Deps => {
                                s.insert(inspect::PlanSection::Deps);
                            }
                            PlanSection::All => {
                                s.extend(inspect::PlanSection::default_all());
                            }
                        }
                    }
                    s
                };
                let mode = match format {
                    PlanFormat::Text => inspect::PlanOutput::Text(sections),
                    PlanFormat::Dot => inspect::PlanOutput::DotCompressed,
                    PlanFormat::DotFull => inspect::PlanOutput::DotFull,
                    PlanFormat::Mermaid if link => inspect::PlanOutput::MermaidLinkCompressed,
                    PlanFormat::Mermaid => inspect::PlanOutput::MermaidCompressed,
                    PlanFormat::MermaidFull if link => inspect::PlanOutput::MermaidLinkFull,
                    PlanFormat::MermaidFull => inspect::PlanOutput::MermaidFull,
                };
                inspect::plan(&target.as_inspect(), &action, mode)?;
            }
        },
    }
    Ok(())
}

/// Format a hash as lowercase hex without a prefix, as stored in manifests.
fn hash_hex(hash: Hash) -> String {
    format!("{hash:#}").trim_start_matches("0x").to_lowercase()
}

/// Return whether a manifest hash must be updated, or report a mismatch in
/// `--check` mode.
fn needs_stamp(check: bool, what: &str, declared: Hash, real: Hash) -> Result<bool> {
    if declared == real {
        return Ok(false);
    }
    if check {
        return Err(anyhow!(
            "{what} mismatch: manifest says {}, compiled value is {} (re-run without --check to rewrite)",
            hash_hex(declared),
            hash_hex(real),
        ));
    }
    log::info!(
        "  rewriting {what}: {} -> {}",
        hash_hex(declared),
        hash_hex(real)
    );
    Ok(true)
}

fn build_one(
    source: &PluginSource,
    manifest: &Manifest,
    out_dir: &Path,
    install_dir: Option<&Path>,
    check: bool,
    dep_dirs: &[PathBuf],
) -> Result<()> {
    log::info!("building {}", source.root.display());
    let plugin_name = manifest.plugin.name.clone();

    // Dependencies are validated against their own manifests. This plugin's
    // import pins are updated below.
    let sdk = Sdk::default();
    // Source pins may be stale, so builds resolve dependencies by name.
    let imports = resolve_manifest_imports(&sdk, manifest, dep_dirs, ImportLookup::DeclaredName)?;

    let mut manifest_toml = source.manifest_toml.clone();
    let mut manifest_rewritten = false;
    for (declared, resolved) in manifest.imports.iter().zip(imports.iter()) {
        let real = resolved.module.module().batch.id();
        let what = format!("[[imports]] {} module_hash", declared.name);
        if needs_stamp(check, &what, declared.module_hash, real)
            .with_context(|| format!("in {plugin_name}"))?
        {
            manifest_toml =
                set_manifest_import_hash(&manifest_toml, &declared.name, &hash_hex(real))?;
            manifest_rewritten = true;
        }
    }

    // Compile the script to derive the real module hash from the pod2 batch id.
    let module = pexe::compile_module(&sdk, manifest, &source.script, &imports)?;
    let real_hash = module.module().batch.id();
    if needs_stamp(check, "module_hash", manifest.plugin.module_hash, real_hash)
        .with_context(|| format!("in {plugin_name}"))?
    {
        manifest_toml = set_manifest_hash(&manifest_toml, &hash_hex(real_hash))?;
        manifest_rewritten = true;
    }
    if manifest_rewritten {
        let manifest_path = source.root.join(MANIFEST_FILE);
        std::fs::write(&manifest_path, &manifest_toml)
            .with_context(|| format!("failed to write back {}", manifest_path.display()))?;
    }

    let bytes = pack(&manifest_toml, &source.script)?;
    let out_path = out_dir.join(format!("{plugin_name}.{PEXE_EXTENSION}"));
    std::fs::write(&out_path, &bytes)
        .with_context(|| format!("failed to write {}", out_path.display()))?;
    log::info!(
        "  wrote {} ({} bytes, hash={})",
        out_path.display(),
        bytes.len(),
        hash_hex(real_hash),
    );

    if let Some(dir) = install_dir {
        let installed = install(&bytes, dir, &plugin_name)?;
        log::info!("  installed to {}", installed.display());
    }
    Ok(())
}
