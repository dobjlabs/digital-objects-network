//! `pexe` — build and install zk-craft plugin archives.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use pexe::{
    MANIFEST_FILE, PEXE_EXTENSION, PluginSource, compile_module_hash, install, pack,
    set_manifest_hash, unpack,
};

// These names intentionally mirror `driver::paths::{DOBJ_HOME_DIR, ACTIONS_DIR}`.
// They're duplicated here because the `pexe` lib is a dependency of `driver`, so
// `pexe` can't depend on `driver` without a cycle. If either changes over there,
// change it here too.
const DRIVER_DOBJ_HOME_DIR: &str = ".dobj";
const DRIVER_ACTIONS_DIR: &str = "actions";

fn default_install_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("failed to resolve home directory"))?;
    Ok(home.join(DRIVER_DOBJ_HOME_DIR).join(DRIVER_ACTIONS_DIR))
}

#[derive(Parser, Debug)]
#[command(name = "pexe", about = "zk-craft plugin packaging tool")]
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
        #[arg(long, default_value = "target/pexe")]
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
    },
    /// Dump the contents of a .pexe archive to stdout.
    Dump {
        /// Path to the .pexe file.
        pexe: PathBuf,
    },
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
            for plugin_dir in plugins {
                build_one(&plugin_dir, &out_dir, target_install.as_deref(), check)?;
            }
        }
        Cmd::Dump { pexe } => {
            let bytes = std::fs::read(&pexe)
                .with_context(|| format!("failed to read {}", pexe.display()))?;
            let (manifest, script) = unpack(&bytes)?;
            println!("# manifest");
            println!("{:#?}", manifest);
            println!("\n# plugin.rhai");
            println!("{}", script);
        }
    }
    Ok(())
}

fn build_one(
    plugin_dir: &Path,
    out_dir: &Path,
    install_dir: Option<&Path>,
    check: bool,
) -> Result<()> {
    log::info!("building {}", plugin_dir.display());
    let source = PluginSource::read(plugin_dir)?;
    let manifest = source.parse_manifest()?;
    let plugin_name = manifest.plugin.name.clone();

    // Compile the script to derive the real module hash from the pod2 batch id.
    let real_hash = compile_module_hash(&manifest, &source.script)?;
    let declared_hash = format!("{:#}", manifest.plugin.module_hash);
    let declared_hash = declared_hash.trim_start_matches("0x").to_lowercase();
    let real_hash_clean = real_hash.trim_start_matches("0x").to_lowercase();

    let manifest_toml = if declared_hash == real_hash_clean {
        source.manifest_toml.clone()
    } else if check {
        return Err(anyhow!(
            "module_hash mismatch in {name}: manifest says {declared}, compiled script yields {real} (re-run without --check to rewrite)",
            name = plugin_name,
            declared = declared_hash,
            real = real_hash_clean,
        ));
    } else {
        log::info!(
            "  rewriting module_hash in source manifest: {} -> {}",
            declared_hash,
            real_hash_clean,
        );
        let rewritten = set_manifest_hash(&source.manifest_toml, &real_hash_clean);
        let manifest_path = source.root.join(MANIFEST_FILE);
        std::fs::write(&manifest_path, &rewritten)
            .with_context(|| format!("failed to write back {}", manifest_path.display()))?;
        rewritten
    };

    let bytes = pack(&manifest_toml, &source.script)?;
    let out_path = out_dir.join(format!("{plugin_name}.{PEXE_EXTENSION}"));
    std::fs::write(&out_path, &bytes)
        .with_context(|| format!("failed to write {}", out_path.display()))?;
    log::info!(
        "  wrote {} ({} bytes, hash={})",
        out_path.display(),
        bytes.len(),
        real_hash_clean,
    );

    if let Some(dir) = install_dir {
        let installed = install(&bytes, dir, &plugin_name)?;
        log::info!("  installed to {}", installed.display());
    }
    Ok(())
}
