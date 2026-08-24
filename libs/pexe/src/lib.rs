//! Pexe (Plugin EXEcutable) archive format.
//!
//! A pexe is a zip containing two files:
//!
//! - `manifest.toml` — static metadata ([`sdk::manifest::Manifest`])
//! - `plugin.rhai`   — action logic as a Rhai script
//!
//! Wire-format helpers plus compile/install utilities used by the packaging CLI
//! and by the driver at plugin-load time.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sdk::{ModuleImport, Sdk, manifest::Manifest};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

pub mod fixtures;
pub mod inspect;

pub const MANIFEST_FILE: &str = "manifest.toml";
pub const SCRIPT_FILE: &str = "plugin.rhai";

/// File extension (no leading dot) of a pexe archive.
pub const PEXE_EXTENSION: &str = "pexe";

/// Where `pexe build` writes archives, and where dependency resolution
/// looks for them first.
pub const DEFAULT_OUT_DIR: &str = "target/pexe";

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

/// Compile the script against its manifest's action names. `imports` must
/// hold the resolved module of every plugin the script sub-calls; the hash
/// pins in the manifest are not consulted (this is the compile that produces
/// the value they get stamped with).
pub fn compile_module(
    sdk: &Sdk,
    manifest: &Manifest,
    script: &str,
    imports: &[ModuleImport],
) -> Result<std::rc::Rc<sdk::SdkModule>> {
    let names: Vec<&str> = manifest.actions.iter().map(|a| a.name.as_str()).collect();
    sdk.load_module_from_src_actions(script, &names, imports)
        .map_err(|err| anyhow!("failed to compile plugin: {err}"))
}

/// `.pexe` files in `dir`, sorted by path. A directory that does not
/// exist yields nothing; one that exists but cannot be read is an
/// error, so a permission or I/O problem is never mistaken for "no
/// plugins installed".
pub fn pexe_paths_in(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(anyhow!("failed to read {}: {err}", dir.display())),
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|err| anyhow!("failed to read an entry of {}: {err}", dir.display()))?
            .path();
        if path.extension().and_then(|ext| ext.to_str()) == Some(PEXE_EXTENSION) {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Directories searched for already-built dependency pexes, in priority
/// order (first match by plugin name wins): any explicit `extra` dirs,
/// then the build output dir, then the install dir.
pub fn dep_search_dirs(
    extra: &[PathBuf],
    out_dir: &Path,
    install_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = extra.to_vec();
    dirs.push(out_dir.to_path_buf());
    if let Some(dir) = install_dir {
        dirs.push(dir.to_path_buf());
    } else if let Ok(dir) = default_install_dir() {
        dirs.push(dir);
    }
    dirs
}

/// Unpack every readable `.pexe` under `dirs` into (manifest, script)
/// sources for import resolution. Unreadable or malformed archives are
/// skipped with a log warning so an unrelated broken pexe can't block
/// building an import-free plugin. The first archive found for a plugin
/// name wins.
pub fn discover_dep_sources(dirs: &[PathBuf]) -> Vec<(Manifest, String)> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut sources = Vec::new();
    let dep_paths = dirs.iter().flat_map(|dir| {
        pexe_paths_in(dir).unwrap_or_else(|err| {
            // Search dirs are speculative (a `target/pexe` that was
            // never built, an install dir on a dead mount), so a bad
            // one only removes its own candidates.
            log::warn!("skipping dependency search dir: {err}");
            Vec::new()
        })
    });
    for path in dep_paths {
        match read_pexe_file(&path).and_then(|bytes| unpack(&bytes)) {
            Ok((manifest, script)) => {
                if seen.insert(manifest.plugin.name.clone()) {
                    sources.push((manifest, script));
                }
            }
            Err(err) => {
                log::warn!("skipping unreadable pexe {}: {err}", path.display());
            }
        }
    }
    sources
}

/// Order plugins so that one importing another in the same set builds
/// after it: an importer resolves its `[[imports]]` from archives
/// already on disk, and `examples/*` arrives alphabetically, which is
/// not that order in general. Each entry is a plugin's name and the
/// names it declares as imports; the returned indices are a build
/// order over them.
///
/// Imports naming a plugin outside the set are left alone: those
/// resolve from a previously built or installed archive. A cycle among
/// the given plugins is an error (the SDK's resolver would reject it
/// later anyway, with less context to report).
pub fn import_build_order(plugins: &[(String, Vec<String>)]) -> Result<Vec<usize>> {
    let mut index_by_name: HashMap<&str, usize> = HashMap::new();
    for (idx, (name, _)) in plugins.iter().enumerate() {
        if index_by_name.insert(name.as_str(), idx).is_some() {
            return Err(anyhow!("two plugins in this build are both named {name:?}"));
        }
    }
    let mut marks = vec![Mark::Unvisited; plugins.len()];
    let mut ordered = Vec::with_capacity(plugins.len());
    for idx in 0..plugins.len() {
        visit_imports(idx, plugins, &index_by_name, &mut marks, &mut ordered)?;
    }
    Ok(ordered)
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Unvisited,
    InProgress,
    Done,
}

/// Post-order visit for [`import_build_order`]: emit `idx` once every
/// in-set plugin it imports has been emitted.
fn visit_imports(
    idx: usize,
    plugins: &[(String, Vec<String>)],
    index_by_name: &HashMap<&str, usize>,
    marks: &mut [Mark],
    ordered: &mut Vec<usize>,
) -> Result<()> {
    if marks[idx] != Mark::Unvisited {
        return Ok(());
    }
    marks[idx] = Mark::InProgress;
    for import in &plugins[idx].1 {
        let Some(&dep) = index_by_name.get(import.as_str()) else {
            continue;
        };
        if marks[dep] == Mark::InProgress {
            return Err(anyhow!(
                "import cycle among the plugins being built: {} -> {}",
                plugins[idx].0,
                plugins[dep].0
            ));
        }
        visit_imports(dep, plugins, index_by_name, marks, ordered)?;
    }
    marks[idx] = Mark::Done;
    ordered.push(idx);
    Ok(())
}

/// Resolve a manifest's declared `[[imports]]` against already-built
/// archives found in `dep_dirs`, in `manifest.imports` order. Each
/// dependency is loaded with its own manifest validated (including its
/// own import pins); the pins THIS manifest declares for them are the
/// caller's business, since `pexe build` stamps them.
pub fn resolve_manifest_imports(
    sdk: &Sdk,
    manifest: &Manifest,
    dep_dirs: &[PathBuf],
) -> Result<Vec<ModuleImport>> {
    if manifest.imports.is_empty() {
        return Ok(Vec::new());
    }
    let sources = discover_dep_sources(dep_dirs);
    let mut resolver = sdk::ImportResolver::new(
        sdk,
        sources
            .iter()
            .map(|(manifest, script)| (manifest, script.as_str())),
    );
    resolver.resolve_imports(manifest).map_err(|err| {
        anyhow!(
            "{}: failed to resolve imports (searched {dep_dirs:?}): {err}",
            manifest.plugin.name
        )
    })
}

/// Default plugin install dir, resolved the same way as
/// `driver::paths` (which this crate cannot depend on without a cycle):
/// `$DOBJ_HOME/actions` when set, else `~/.dobj/actions`.
pub fn default_install_dir() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("DOBJ_HOME").filter(|root| !root.is_empty()) {
        return Ok(PathBuf::from(root).join("actions"));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("failed to resolve home directory"))?;
    Ok(home.join(".dobj").join("actions"))
}

/// Set `table["module_hash"]` to `clean`, preserving the existing value's
/// surrounding whitespace and comments when the key is already there.
fn write_module_hash(table: &mut dyn toml_edit::TableLike, clean: &str) {
    if let Some(val) = table.get_mut("module_hash").and_then(|i| i.as_value_mut()) {
        let decor = val.decor().clone();
        *val = clean.into();
        *val.decor_mut() = decor;
    } else {
        table.insert("module_hash", toml_edit::value(clean));
    }
}

fn parse_manifest_doc(toml_src: &str) -> Result<toml_edit::DocumentMut> {
    toml_src
        .parse::<toml_edit::DocumentMut>()
        .map_err(|err| anyhow!("invalid manifest toml: {err}"))
}

/// Rewrite the `module_hash` line in a manifest's TOML source to the given hash,
/// preserving formatting of everything else. Adds the line under `[plugin]` if
/// absent.
pub fn set_manifest_hash(toml_src: &str, new_hash_hex: &str) -> Result<String> {
    let clean = new_hash_hex.trim_start_matches("0x");
    let mut doc = parse_manifest_doc(toml_src)?;
    let plugin = doc["plugin"]
        .as_table_like_mut()
        .ok_or_else(|| anyhow!("manifest has no [plugin] table"))?;
    write_module_hash(plugin, clean);
    Ok(doc.to_string())
}

/// Rewrite the `module_hash` of one `[[imports]]` entry in a manifest's TOML
/// source, preserving formatting of everything else.
pub fn set_manifest_import_hash(
    toml_src: &str,
    import_name: &str,
    new_hash_hex: &str,
) -> Result<String> {
    let clean = new_hash_hex.trim_start_matches("0x");
    let mut doc = parse_manifest_doc(toml_src)?;
    let imports = doc
        .get_mut("imports")
        .ok_or_else(|| anyhow!("manifest declares no imports"))?;
    // `[[imports]]` tables and an inline `imports = [{ name = ... }]`
    // array both deserialize into `Manifest::imports`, so either can be
    // the form waiting to be stamped.
    let tables: Vec<&mut dyn toml_edit::TableLike> = match imports {
        toml_edit::Item::ArrayOfTables(tables) => tables
            .iter_mut()
            .map(|table| table as &mut dyn toml_edit::TableLike)
            .collect(),
        toml_edit::Item::Value(toml_edit::Value::Array(values)) => values
            .iter_mut()
            .filter_map(|value| value.as_inline_table_mut())
            .map(|table| table as &mut dyn toml_edit::TableLike)
            .collect(),
        _ => {
            return Err(anyhow!(
                "manifest `imports` is neither [[imports]] tables nor an array of tables"
            ));
        }
    };
    let table = tables
        .into_iter()
        .find(|table| table.get("name").and_then(|name| name.as_str()) == Some(import_name))
        .ok_or_else(|| anyhow!("manifest has no imports entry named {import_name:?}"))?;
    write_module_hash(table, clean);
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

    fn build_order(plugins: &[(&str, &[&str])]) -> Result<Vec<String>> {
        let owned: Vec<(String, Vec<String>)> = plugins
            .iter()
            .map(|(name, imports)| {
                (
                    name.to_string(),
                    imports.iter().map(|i| i.to_string()).collect(),
                )
            })
            .collect();
        Ok(import_build_order(&owned)?
            .into_iter()
            .map(|idx| owned[idx].0.clone())
            .collect())
    }

    #[test]
    fn test_import_build_order_puts_dependencies_first() {
        // Alphabetical input, reverse dependency order.
        let order =
            build_order(&[("craft-totem", &["craft-basics"]), ("craft-basics", &[])]).unwrap();
        assert_eq!(order, vec!["craft-basics", "craft-totem"]);

        // Transitive chain, worst-case input order.
        let order = build_order(&[
            ("builder", &["mason"]),
            ("mason", &["quarry"]),
            ("quarry", &[]),
        ])
        .unwrap();
        assert_eq!(order, vec!["quarry", "mason", "builder"]);

        // A diamond emits each plugin once, dependencies first.
        let order = build_order(&[
            ("top", &["left", "right"]),
            ("left", &["base"]),
            ("right", &["base"]),
            ("base", &[]),
        ])
        .unwrap();
        assert_eq!(order.len(), 4);
        let position = |name: &str| order.iter().position(|n| n == name).unwrap();
        assert!(position("base") < position("left"));
        assert!(position("base") < position("right"));
        assert!(position("left") < position("top"));
        assert!(position("right") < position("top"));
    }

    #[test]
    fn test_import_build_order_ignores_imports_outside_the_set() {
        // `installed` is not being built here: it resolves from disk,
        // and the given order is kept.
        let order = build_order(&[("a", &["installed"]), ("b", &[])]).unwrap();
        assert_eq!(order, vec!["a", "b"]);
    }

    #[test]
    fn test_import_build_order_rejects_cycles() {
        let err = build_order(&[("a", &["b"]), ("b", &["a"])]).unwrap_err();
        assert!(err.to_string().contains("import cycle"), "got: {err}");

        let err = build_order(&[("a", &["a"])]).unwrap_err();
        assert!(err.to_string().contains("import cycle"), "got: {err}");
    }

    #[test]
    fn test_import_build_order_rejects_duplicate_names() {
        let err = build_order(&[("a", &[]), ("a", &[])]).unwrap_err();
        assert!(err.to_string().contains("both named"), "got: {err}");
    }

    #[test]
    fn test_set_manifest_import_hash_accepts_either_toml_form() {
        let stamped = "1111111111111111111111111111111111111111111111111111111111111111";
        let table_form = r#"
classes = []
actions = []

[plugin]
name = "craft-totem"
version = "0.1.0"
module_hash = "0000000000000000000000000000000000000000000000000000000000000000"

[[imports]]
name = "craft-basics"
module_hash = "0000000000000000000000000000000000000000000000000000000000000000"
"#;
        let out = set_manifest_import_hash(table_form, "craft-basics", stamped).unwrap();
        assert!(out.contains(stamped), "not stamped:\n{out}");

        // Root-level key, so it has to precede the first table header.
        let inline_form = r#"
classes = []
actions = []
imports = [{ name = "craft-basics", module_hash = "0000000000000000000000000000000000000000000000000000000000000000" }]

[plugin]
name = "craft-totem"
version = "0.1.0"
module_hash = "0000000000000000000000000000000000000000000000000000000000000000"
"#;
        let out = set_manifest_import_hash(inline_form, "craft-basics", stamped).unwrap();
        assert!(out.contains(stamped), "not stamped:\n{out}");
        // Both forms parse back into the same declared imports.
        let manifest: Manifest = toml::from_str(&out).unwrap();
        assert_eq!(manifest.imports.len(), 1);
        assert_eq!(manifest.imports[0].name, "craft-basics");

        let err = set_manifest_import_hash(table_form, "ghost", stamped).unwrap_err();
        assert!(err.to_string().contains("no imports entry"), "got: {err}");
    }

    #[test]
    fn test_pexe_paths_in_reports_unreadable_dirs() {
        let missing = std::path::Path::new("/definitely/not/here/actions");
        assert_eq!(pexe_paths_in(missing).unwrap(), Vec::<PathBuf>::new());

        // A file where a directory is expected: exists, cannot be read
        // as a dir. An empty listing here would look like "no plugins
        // installed" rather than a broken actions dir.
        let mut file = std::env::temp_dir();
        file.push(format!("pexe-paths-in-{}.not-a-dir", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let err = pexe_paths_in(&file).unwrap_err();
        std::fs::remove_file(&file).unwrap();
        assert!(err.to_string().contains("failed to read"), "got: {err}");
    }

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

    const EXAMPLES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");

    /// Every example's committed `module_hash` (and every `[[imports]]`
    /// pin) must match what its committed source compiles to. `pexe
    /// build` rewrites a stale hash silently, and release CI builds
    /// without `--check`, so without this the repo can carry a manifest
    /// that no longer describes its own plugin -- which fails a catalog
    /// load for anyone who installs the archive as committed.
    #[test]
    fn every_example_manifest_hash_is_current() {
        let mut sources: Vec<(Manifest, String)> = Vec::new();
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(EXAMPLES_DIR)
            .expect("examples dir readable")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        assert!(!dirs.is_empty(), "no example plugins found");
        for dir in &dirs {
            let source = PluginSource::read(dir).expect("example source readable");
            let manifest = source
                .parse_manifest()
                .unwrap_or_else(|err| panic!("{}: {err}", dir.display()));
            sources.push((manifest, source.script));
        }

        // Resolving from these same sources (rather than from built
        // archives) is what makes the check about the committed state.
        let sdk = Sdk::default();
        let mut resolver = sdk::ImportResolver::new(
            &sdk,
            sources
                .iter()
                .map(|(manifest, script)| (manifest, script.as_str())),
        );
        for (manifest, _) in &sources {
            let name = &manifest.plugin.name;
            // `load` compiles the plugin and validates its own hash plus
            // its import pins, so a stale value fails right here.
            resolver.load(name).unwrap_or_else(|err| {
                panic!("{name}: stale manifest? re-run `pexe build examples/*`: {err}")
            });
        }
    }
}
