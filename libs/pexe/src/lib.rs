//! Pexe (Plugin EXEcutable) archive format.
//!
//! A pexe is a zip containing two files:
//!
//! - `manifest.toml` — static metadata ([`sdk::manifest::Manifest`])
//! - `plugin.rhai`   — action logic as a Rhai script
//!
//! Wire-format helpers plus compile/install utilities used by the packaging CLI
//! and by the driver at plugin-load time.

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sdk::{Sdk, manifest::Manifest};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

pub mod fixtures;
pub mod inspect;

pub const MANIFEST_FILE: &str = "manifest.toml";
pub const SCRIPT_FILE: &str = "plugin.rhai";

/// File extension (no leading dot) of a pexe archive.
pub const PEXE_EXTENSION: &str = "pexe";

/// Largest `.pexe` file we will read from disk into memory. A packed bundled
/// plugin is under 8 KiB; this bounds the compressed-side read for untrusted
/// archives without rejecting any realistic plugin.
pub const MAX_PEXE_BYTES: u64 = 8 * 1024 * 1024;

/// Largest decompressed size we will accept for a single entry. The biggest
/// real entry across bundled plugins is ~29 KiB, so 1 MiB is generous headroom
/// while making decompression bombs impossible: a malicious entry that inflates
/// past this cap is rejected instead of growing the heap unbounded.
const MAX_ENTRY_BYTES: u64 = 1024 * 1024;

/// A valid pexe holds exactly two entries (`manifest.toml`, `plugin.rhai`).
/// Capping the declared entry count stops a crafted central directory from
/// forcing large allocations inside `ZipArchive`.
const MAX_ENTRIES: usize = 16;

/// Pexe source on disk: a directory containing `manifest.toml` and `plugin.rhai`.
pub struct PluginSource {
    pub root: PathBuf,
    pub manifest_toml: String,
    pub script: String,
}

impl PluginSource {
    pub fn read(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let manifest_path = root.join(MANIFEST_FILE);
        let script_path = root.join(SCRIPT_FILE);
        let manifest_toml = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read manifest: {}", manifest_path.display()))?;
        let script = std::fs::read_to_string(&script_path)
            .with_context(|| format!("failed to read script: {}", script_path.display()))?;
        Ok(Self {
            root,
            manifest_toml,
            script,
        })
    }

    pub fn parse_manifest(&self) -> Result<Manifest> {
        toml::from_str(&self.manifest_toml).map_err(|err| anyhow!("invalid manifest.toml: {err}"))
    }
}

/// Pack a manifest + script into pexe bytes.
pub fn pack(manifest_toml: &str, script: &str) -> Result<Vec<u8>> {
    let buf = Cursor::new(Vec::<u8>::new());
    let mut zip = ZipWriter::new(buf);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file(MANIFEST_FILE, opts)?;
    zip.write_all(manifest_toml.as_bytes())?;

    zip.start_file(SCRIPT_FILE, opts)?;
    zip.write_all(script.as_bytes())?;

    let buf = zip.finish()?;
    Ok(buf.into_inner())
}

/// Unpack pexe bytes into `(manifest_toml_src, script_src)` without parsing.
pub fn unpack_raw(bytes: &[u8]) -> Result<(String, String)> {
    let mut zip =
        ZipArchive::new(Cursor::new(bytes)).map_err(|err| anyhow!("invalid pexe zip: {err}"))?;
    if zip.len() > MAX_ENTRIES {
        bail!(
            "pexe declares {} entries, exceeds limit of {MAX_ENTRIES}",
            zip.len()
        );
    }
    let manifest_toml = read_entry(&mut zip, MANIFEST_FILE)?;
    let script = read_entry(&mut zip, SCRIPT_FILE)?;
    Ok((manifest_toml, script))
}

/// Unpack pexe bytes into a parsed [`Manifest`] and the script source.
pub fn unpack(bytes: &[u8]) -> Result<(Manifest, String)> {
    let (manifest_toml, script) = unpack_raw(bytes)?;
    let manifest: Manifest =
        toml::from_str(&manifest_toml).map_err(|err| anyhow!("invalid manifest.toml: {err}"))?;
    Ok((manifest, script))
}

fn read_entry<R: Read + std::io::Seek>(zip: &mut ZipArchive<R>, name: &str) -> Result<String> {
    let file = zip
        .by_name(name)
        .map_err(|err| anyhow!("missing entry {name} in pexe: {err}"))?;
    // Read raw bytes through a capped reader rather than `read_to_string`: the
    // decompressed output can never grow past the limit (the decompression-bomb
    // guard), and an over-cap entry fails with a clear size error instead of a
    // confusing mid-codepoint UTF-8 error.
    let mut out = Vec::new();
    file.take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut out)
        .map_err(|err| anyhow!("failed to read {name} in pexe: {err}"))?;
    if out.len() as u64 > MAX_ENTRY_BYTES {
        bail!(
            "pexe entry {name} exceeds {MAX_ENTRY_BYTES}-byte limit (possible decompression bomb)"
        );
    }
    String::from_utf8(out).map_err(|err| anyhow!("entry {name} in pexe is not valid UTF-8: {err}"))
}

/// Compile the script against its manifest's action names and return the hex-encoded
/// module hash.
pub fn compile_module_hash(manifest: &Manifest, script: &str) -> Result<String> {
    let sdk = Sdk::default();
    let names: Vec<&str> = manifest.actions.iter().map(|a| a.name.as_str()).collect();
    let module = sdk
        .load_module_from_src_actions(script, &names)
        .map_err(|err| anyhow!("failed to compile plugin: {err}"))?;
    Ok(format!("{:#}", module.module().batch.id()))
}

/// Rewrite the `module_hash` line in a manifest's TOML source to the given hash,
/// preserving formatting of everything else. Adds the line under `[plugin]` if
/// absent.
pub fn set_manifest_hash(toml_src: &str, new_hash_hex: &str) -> Result<String> {
    let clean = new_hash_hex.trim_start_matches("0x");
    let mut doc = toml_src
        .parse::<toml_edit::DocumentMut>()
        .map_err(|err| anyhow!("invalid manifest toml: {err}"))?;
    // If the key already exists, preserve its surrounding whitespace and comments
    // by replacing only the inner string while keeping the value's decor.
    if let Some(val) = doc["plugin"]
        .get_mut("module_hash")
        .and_then(|i| i.as_value_mut())
    {
        let decor = val.decor().clone();
        *val = clean.into();
        *val.decor_mut() = decor;
    } else {
        doc["plugin"]["module_hash"] = toml_edit::value(clean);
    }
    Ok(doc.to_string())
}

/// Install pexe bytes into `target_dir` as `<plugin_name>.pexe`.
pub fn install(bytes: &[u8], target_dir: &Path, plugin_name: &str) -> Result<PathBuf> {
    if plugin_name.is_empty() {
        bail!("plugin name is empty");
    }
    std::fs::create_dir_all(target_dir)
        .with_context(|| format!("failed to create actions dir: {}", target_dir.display()))?;
    let path = target_dir.join(format!("{plugin_name}.{PEXE_EXTENSION}"));
    std::fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// Read a `.pexe` file from disk, rejecting anything larger than
/// [`MAX_PEXE_BYTES`] before it is loaded into memory. Reading through a capped
/// reader (rather than stat-then-read) closes the gap where a file grows
/// between the size check and the read.
pub fn read_pexe_file(path: &Path) -> Result<Vec<u8>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_PEXE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if bytes.len() as u64 > MAX_PEXE_BYTES {
        bail!(
            "pexe {} exceeds {MAX_PEXE_BYTES}-byte limit",
            path.display()
        );
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML_WITH_HASH: &str = r#"
[plugin]
name = "craft-basics"
version = "0.1.0"
module_hash = "0000000000000000000000000000000000000000000000000000000000000000"
"#;

    #[test]
    fn test_pack_unpack_round_trip() {
        let bytes = pack("name = \"x\"", "fn Foo() {}").unwrap();
        let (manifest, script) = unpack_raw(&bytes).unwrap();
        assert!(manifest.contains("name = \"x\""));
        assert_eq!(script, "fn Foo() {}");
    }

    fn zip_with_entries(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::<u8>::new()));
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn test_unpack_rejects_decompression_bomb() {
        // A >1 MiB run of zeros compresses to a few hundred bytes: the classic
        // small-archive, huge-payload shape.
        let bomb = vec![0u8; (MAX_ENTRY_BYTES + 1024) as usize];
        let bytes = zip_with_entries(&[(MANIFEST_FILE, b"name = \"x\""), (SCRIPT_FILE, &bomb)]);
        assert!(
            bytes.len() < 4096,
            "bomb archive should be tiny, got {} bytes",
            bytes.len()
        );
        let err = unpack_raw(&bytes).unwrap_err().to_string();
        assert!(
            err.contains("decompression bomb"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_unpack_rejects_too_many_entries() {
        let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("f{i}.txt")).collect();
        let data: &[u8] = b"x";
        let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), data)).collect();
        let bytes = zip_with_entries(&entries);
        let err = unpack_raw(&bytes).unwrap_err().to_string();
        assert!(err.contains("exceeds limit"), "unexpected error: {err}");
    }

    #[test]
    fn test_set_manifest_hash_replaces() {
        let out = set_manifest_hash(TOML_WITH_HASH, "deadbeef").unwrap();
        assert!(out.contains("module_hash = \"deadbeef\""));
        assert!(!out.contains(
            "module_hash = \"0000000000000000000000000000000000000000000000000000000000000000\""
        ));
    }

    #[test]
    fn test_set_manifest_hash_strips_prefix() {
        let out = set_manifest_hash(TOML_WITH_HASH, "0xdeadbeef").unwrap();
        assert!(out.contains("module_hash = \"deadbeef\""));
    }

    #[test]
    fn test_set_manifest_hash_inserts_when_missing() {
        let src = "[plugin]\nname = \"x\"\nversion = \"0.1\"\n";
        let out = set_manifest_hash(src, "cafe").unwrap();
        assert!(out.contains("module_hash = \"cafe\""));
    }

    #[test]
    fn test_set_manifest_hash_preserves_trailing_comment() {
        let src = "[plugin]\nname = \"x\"\nmodule_hash = \"0000\" # pinned by CI\n";
        let out = set_manifest_hash(src, "cafe").unwrap();
        assert!(out.contains("cafe"));
        assert!(out.contains("# pinned by CI"));
    }
}
