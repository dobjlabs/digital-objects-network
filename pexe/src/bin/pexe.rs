//! `pexe` — build and install zk-craft plugin archives.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use pexe::{
    MANIFEST_FILE, PEXE_EXTENSION, PluginSource, compile_module_hash, install, pack,
    set_manifest_hash, unpack,
};
use pod2::{
    backends::plonky2::signer::Signer as PodSigner,
    frontend::SignedDictBuilder,
    middleware::{Params, SecretKey},
};

// These names intentionally mirror `driver::paths::{DOBJ_HOME_DIR, ACTIONS_DIR}`.
// They're duplicated here because the `pexe` lib is a dependency of `driver`, so
// `pexe` can't depend on `driver` without a cycle. If either changes over there,
// change it here too.
const DRIVER_DOBJ_HOME_DIR: &str = ".dobj";
const DRIVER_ACTIONS_DIR: &str = "actions";
const DRIVER_KEYS_DIR: &str = "keys";
const DRIVER_CREDENTIALS_DIR: &str = "credentials";

fn default_install_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("failed to resolve home directory"))?;
    Ok(home.join(DRIVER_DOBJ_HOME_DIR).join(DRIVER_ACTIONS_DIR))
}

fn default_keys_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("failed to resolve home directory"))?;
    Ok(home.join(DRIVER_DOBJ_HOME_DIR).join(DRIVER_KEYS_DIR))
}

fn default_credentials_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("failed to resolve home directory"))?;
    Ok(home.join(DRIVER_DOBJ_HOME_DIR).join(DRIVER_CREDENTIALS_DIR))
}

fn sk_path(keys_dir: &Path, label: &str) -> PathBuf {
    keys_dir.join(format!("{label}.sk.json"))
}

fn pk_path(keys_dir: &Path, label: &str) -> PathBuf {
    keys_dir.join(format!("{label}.pk.json"))
}

fn load_secret_key(keys_dir: &Path, label: &str) -> Result<SecretKey> {
    let path = sk_path(keys_dir, label);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read secret key at {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse secret key at {}", path.display()))
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
    /// Generate a fresh Schnorr keypair and save it under
    /// `~/.dobj/keys/<label>.{sk,pk}.json`. Prints the public key
    /// (base58) to stdout so you can paste it into a plugin script.
    GenKeypair {
        /// Identity label, e.g. `employer`, `govt`, `farmer`.
        label: String,

        /// Override the keys directory (default: ~/.dobj/keys).
        #[arg(long)]
        keys_dir: Option<PathBuf>,

        /// Overwrite an existing keypair with the same label.
        #[arg(long)]
        force: bool,
    },
    /// Sign a dictionary with a stored secret key, producing a
    /// `SignedDict` JSON that downstream actions can consume via
    /// `input_signed_dict`. Use to issue credentials off-band — e.g.
    /// an employer signing an income statement for a farmer.
    SignCredential {
        /// Identity label of the signing key (must exist under
        /// `<keys-dir>/<label>.sk.json`).
        #[arg(long)]
        signer: String,

        /// Where to write the signed credential. Pass `-` for stdout.
        /// Default: `<credentials-dir>/<signer>-<unix-ts>.cred.json`.
        #[arg(long)]
        output: Option<PathBuf>,

        /// Override the keys directory (default: ~/.dobj/keys).
        #[arg(long)]
        keys_dir: Option<PathBuf>,

        /// Override the credentials directory (default: ~/.dobj/credentials).
        #[arg(long)]
        credentials_dir: Option<PathBuf>,

        /// Field assignments `key=value`. Values that parse as i64 are
        /// stored as ints; everything else is stored as strings.
        #[arg(required = true)]
        fields: Vec<String>,
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
        Cmd::GenKeypair {
            label,
            keys_dir,
            force,
        } => {
            let keys_dir = match keys_dir {
                Some(p) => p,
                None => default_keys_dir()?,
            };
            gen_keypair(&keys_dir, &label, force)?;
        }
        Cmd::SignCredential {
            signer,
            output,
            keys_dir,
            credentials_dir,
            fields,
        } => {
            let keys_dir = match keys_dir {
                Some(p) => p,
                None => default_keys_dir()?,
            };
            let credentials_dir = match credentials_dir {
                Some(p) => p,
                None => default_credentials_dir()?,
            };
            sign_credential(&keys_dir, &credentials_dir, &signer, output.as_deref(), &fields)?;
        }
    }
    Ok(())
}

fn gen_keypair(keys_dir: &Path, label: &str, force: bool) -> Result<()> {
    if label.is_empty() || label.contains(['/', '\\']) {
        bail!("invalid label: must be non-empty and contain no path separators");
    }
    std::fs::create_dir_all(keys_dir)
        .with_context(|| format!("failed to create keys dir {}", keys_dir.display()))?;
    let sk_p = sk_path(keys_dir, label);
    let pk_p = pk_path(keys_dir, label);
    if !force && (sk_p.exists() || pk_p.exists()) {
        bail!(
            "keypair {label} already exists at {} / {}; pass --force to overwrite",
            sk_p.display(),
            pk_p.display()
        );
    }
    let sk = SecretKey::new_rand();
    let pk = sk.public_key();
    let sk_json = serde_json::to_string(&sk)?;
    let pk_json = serde_json::to_string(&pk)?;
    std::fs::write(&sk_p, &sk_json)
        .with_context(|| format!("failed to write {}", sk_p.display()))?;
    std::fs::write(&pk_p, &pk_json)
        .with_context(|| format!("failed to write {}", pk_p.display()))?;
    log::info!("  wrote {}", sk_p.display());
    log::info!("  wrote {}", pk_p.display());
    // Public key on stdout for easy paste-into-plugin.
    println!("{pk}");
    Ok(())
}

fn sign_credential(
    keys_dir: &Path,
    credentials_dir: &Path,
    signer: &str,
    output: Option<&Path>,
    fields: &[String],
) -> Result<()> {
    let sk = load_secret_key(keys_dir, signer)?;
    let params = Params::default();
    let mut builder = SignedDictBuilder::new(&params);
    for field in fields {
        let (k, v) = field
            .split_once('=')
            .ok_or_else(|| anyhow!("field {field:?} must be of the form key=value"))?;
        if let Ok(i) = v.parse::<i64>() {
            builder.insert(k, i);
        } else {
            builder.insert(k, v);
        }
    }
    let signed = builder
        .sign(&PodSigner(sk))
        .map_err(|e| anyhow!("signing failed: {e}"))?;
    let json = serde_json::to_string_pretty(&signed)?;

    match output {
        Some(p) if p.as_os_str() == "-" => {
            println!("{json}");
        }
        Some(p) => {
            if let Some(parent) = p.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create parent dir {}", parent.display())
                    })?;
                }
            }
            std::fs::write(p, &json)
                .with_context(|| format!("failed to write {}", p.display()))?;
            log::info!("  wrote {}", p.display());
        }
        None => {
            std::fs::create_dir_all(credentials_dir)
                .with_context(|| format!("failed to create {}", credentials_dir.display()))?;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let p = credentials_dir.join(format!("{signer}-{ts}.cred.json"));
            std::fs::write(&p, &json)
                .with_context(|| format!("failed to write {}", p.display()))?;
            log::info!("  wrote {}", p.display());
            println!("{}", p.display());
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
        let rewritten = set_manifest_hash(&source.manifest_toml, &real_hash_clean)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use pod2::frontend::SignedDict;

    /// Round-trips gen-keypair → sign-credential and verifies the
    /// produced credential is a valid `SignedDict` whose signature
    /// checks out against the saved public key.
    #[test]
    fn test_gen_keypair_then_sign_credential_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let keys_dir = tmp.path().join("keys");
        let creds_dir = tmp.path().join("credentials");

        gen_keypair(&keys_dir, "employer", false).unwrap();
        assert!(sk_path(&keys_dir, "employer").exists());
        assert!(pk_path(&keys_dir, "employer").exists());

        sign_credential(
            &keys_dir,
            &creds_dir,
            "employer",
            None,
            &[
                "income=30000".to_string(),
                "year=2026".to_string(),
                "recipient=alice".to_string(),
            ],
        )
        .unwrap();

        // One credential file written.
        let cred_files: Vec<_> = std::fs::read_dir(&creds_dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(cred_files.len(), 1);

        // Parses as a SignedDict and verifies.
        let raw = std::fs::read_to_string(&cred_files[0]).unwrap();
        let signed: SignedDict = serde_json::from_str(&raw).unwrap();
        signed.verify().unwrap();

        // PK in the credential matches the saved one.
        let saved_pk_json = std::fs::read_to_string(pk_path(&keys_dir, "employer")).unwrap();
        let saved_pk: pod2::middleware::PublicKey =
            serde_json::from_str(&saved_pk_json).unwrap();
        assert_eq!(signed.public_key, saved_pk);

        // Field values round-tripped.
        assert_eq!(
            signed.dict.get(&"income".into()).unwrap().unwrap().as_int(),
            Some(30000)
        );
        assert_eq!(
            signed.dict.get(&"year".into()).unwrap().unwrap().as_int(),
            Some(2026)
        );
    }

    /// `gen-keypair` without `--force` must refuse to overwrite an
    /// existing keypair.
    #[test]
    fn test_gen_keypair_refuses_to_clobber() {
        let tmp = tempfile::tempdir().unwrap();
        let keys_dir = tmp.path().join("keys");
        gen_keypair(&keys_dir, "alice", false).unwrap();
        let err = gen_keypair(&keys_dir, "alice", false).unwrap_err();
        assert!(err.to_string().contains("already exists"));
        // With --force, it succeeds.
        gen_keypair(&keys_dir, "alice", true).unwrap();
    }

    /// Bad field syntax should fail with a clear error.
    #[test]
    fn test_sign_credential_rejects_malformed_field() {
        let tmp = tempfile::tempdir().unwrap();
        let keys_dir = tmp.path().join("keys");
        let creds_dir = tmp.path().join("credentials");
        gen_keypair(&keys_dir, "x", false).unwrap();
        let err = sign_credential(
            &keys_dir,
            &creds_dir,
            "x",
            Some(std::path::Path::new("-")),
            &["no_equals_sign".to_string()],
        )
        .unwrap_err();
        assert!(err.to_string().contains("must be of the form"));
    }
}
