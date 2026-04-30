//! Filesystem operations on the local `.dobj` directory.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::object::{ObjectRecord, ObjectStatus, file_name_for, parse_object_record_file, write_object_record_file};
use crate::paths::{DriverPaths, is_dobj_file};
use txlib_core::Hash;

pub fn ensure_dirs(paths: &DriverPaths) -> Result<()> {
    fs::create_dir_all(&paths.objects_dir)
        .map_err(|e| anyhow!("mkdir {}: {e}", paths.objects_dir.display()))?;
    fs::create_dir_all(&paths.nullified_objects_dir)
        .map_err(|e| anyhow!("mkdir {}: {e}", paths.nullified_objects_dir.display()))?;
    Ok(())
}

/// Write a record into the objects dir under its canonical file name.
/// Returns the full path written.
pub fn write_live(paths: &DriverPaths, record: &ObjectRecord) -> Result<PathBuf> {
    ensure_dirs(paths)?;
    let path = paths
        .objects_dir
        .join(file_name_for(&record.class_name, record.commitment()));
    write_object_record_file(&path, record)?;
    Ok(path)
}

/// List all live `.dobj` files (i.e. those in `objects_dir` directly, not
/// under `.nullified`).
pub fn list_live(paths: &DriverPaths) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !paths.objects_dir.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(&paths.objects_dir)
        .map_err(|e| anyhow!("read_dir {}: {e}", paths.objects_dir.display()))?
    {
        let entry = entry.map_err(|e| anyhow!("dir entry: {e}"))?;
        let path = entry.path();
        if path.is_file() && is_dobj_file(&path) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// Move a live `.dobj` into the `.nullified` subdirectory and rewrite it
/// with `status = nullified`. Idempotent: if `path` is already inside the
/// nullified dir, it just updates the status.
pub fn nullify(paths: &DriverPaths, path: &Path) -> Result<PathBuf> {
    ensure_dirs(paths)?;
    let mut record = parse_object_record_file(path)?;
    record.status = ObjectStatus::Nullified;
    let target = paths
        .nullified_objects_dir
        .join(file_name_for(&record.class_name, record.commitment()));
    write_object_record_file(&target, &record)?;
    if path != target {
        let _ = fs::remove_file(path);
    }
    Ok(target)
}

/// Find a live record by its `id` (commitment hex). Returns `Ok(None)` if
/// no matching file is found, `Err` if multiple files share the id (which
/// would be a deduplication bug).
pub fn find_by_id(paths: &DriverPaths, id: &str) -> Result<Option<(PathBuf, ObjectRecord)>> {
    let mut matches = Vec::new();
    for path in list_live(paths)? {
        let r = parse_object_record_file(&path)?;
        if r.id == id {
            matches.push((path, r));
        }
    }
    if matches.len() > 1 {
        return Err(anyhow!("found {} records with id {}", matches.len(), id));
    }
    Ok(matches.into_iter().next())
}

/// Find a live record by file name (relative to `objects_dir`).
pub fn find_by_file_name(
    paths: &DriverPaths,
    file_name: &str,
) -> Result<Option<(PathBuf, ObjectRecord)>> {
    let path = paths.objects_dir.join(file_name);
    if !path.exists() {
        return Ok(None);
    }
    let r = parse_object_record_file(&path)?;
    Ok(Some((path, r)))
}

/// Update an existing record on disk by `obj.commitment` lookup.
pub fn update_by_commitment(
    paths: &DriverPaths,
    commitment: Hash,
    update: impl FnOnce(&mut ObjectRecord),
) -> Result<()> {
    let id = format!("{commitment}");
    let (path, mut record) = find_by_id(paths, &id)?
        .ok_or_else(|| anyhow!("no live record with id {id}"))?;
    update(&mut record);
    write_object_record_file(&path, &record)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{SourceTxData, sorted_commitments};
    use tempfile::tempdir;
    use txlib_core::hash::sha256;
    use txlib_core::merkle::set_smt_root;
    use txlib_core::merkle_store::empty_root;
    use txlib_core::object;

    fn sample_paths() -> (DriverPaths, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let p = DriverPaths::from_dobj_root(dir.path());
        (p, dir)
    }

    fn sample_record(blueprint: &str, key_seed: &str) -> ObjectRecord {
        let obj = object! {
            "blueprint" => blueprint,
            "key" => sha256(key_seed.as_bytes()),
        };
        let live = sorted_commitments(&[obj.clone()]);
        let source_tx = SourceTxData {
            action_id: 1,
            live_root: set_smt_root(&live),
            nullifiers_root: empty_root(),
            action_nonce: sha256(blueprint.as_bytes()),
        };
        ObjectRecord::new(obj, blueprint.to_string(), source_tx, live)
    }

    #[test]
    fn write_then_list_then_find() {
        let (paths, _dir) = sample_paths();
        let r = sample_record("Wood", "k1");
        let written = write_live(&paths, &r).unwrap();
        let listed = list_live(&paths).unwrap();
        assert_eq!(listed, vec![written.clone()]);
        let found = find_by_id(&paths, &r.id).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().0, written);
    }

    #[test]
    fn nullify_moves_file() {
        let (paths, _dir) = sample_paths();
        let r = sample_record("Log", "k");
        let path = write_live(&paths, &r).unwrap();
        let target = nullify(&paths, &path).unwrap();
        assert!(target.starts_with(&paths.nullified_objects_dir));
        assert!(!path.exists());
        assert!(target.exists());
        // Live list no longer includes it.
        assert!(list_live(&paths).unwrap().is_empty());
    }

    #[test]
    fn list_skips_nullified_dir() {
        let (paths, _dir) = sample_paths();
        let live = sample_record("Wood", "live");
        let dead = sample_record("Stone", "dead");
        write_live(&paths, &live).unwrap();
        let dead_path = write_live(&paths, &dead).unwrap();
        nullify(&paths, &dead_path).unwrap();

        let listed = list_live(&paths).unwrap();
        assert_eq!(listed.len(), 1);
        let r = parse_object_record_file(&listed[0]).unwrap();
        assert_eq!(r.class_name, "Wood");
    }

    #[test]
    fn duplicate_id_errors() {
        // The store dedups by file name, so creating two distinct files
        // with the same id requires manual fiddling — but we still detect
        // it on `find_by_id`.
        let (paths, _dir) = sample_paths();
        let r = sample_record("Wood", "k");
        write_live(&paths, &r).unwrap();
        // Write a second file by hand with a different name but same id.
        let other = paths.objects_dir.join("dupe.dobj");
        write_object_record_file(&other, &r).unwrap();
        let err = find_by_id(&paths, &r.id).unwrap_err();
        assert!(err.to_string().contains("found 2 records"), "{err}");
    }
}
