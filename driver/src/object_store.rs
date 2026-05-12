use anyhow::{Result, anyhow};
use std::{collections::HashMap, fs, path::Path};

use crate::object_record::ObjectRecord;
use crate::types::{DriverPaths, ObjectQuery, ObjectSelector};

#[derive(Debug, Clone)]
pub(crate) struct ObjectFileEntry {
    pub(crate) file_name: String,
    pub(crate) record: ObjectRecord,
}

pub(crate) fn ensure_store_dirs(paths: &DriverPaths) -> Result<()> {
    fs::create_dir_all(&paths.objects_dir).map_err(|err| {
        anyhow!(
            "failed to create objects directory {}: {err}",
            paths.objects_dir.display()
        )
    })?;
    fs::create_dir_all(&paths.nullified_objects_dir).map_err(|err| {
        anyhow!(
            "failed to create nullified directory {}: {err}",
            paths.nullified_objects_dir.display()
        )
    })?;
    fs::create_dir_all(&paths.actions_dir).map_err(|err| {
        anyhow!(
            "failed to create actions directory {}: {err}",
            paths.actions_dir.display()
        )
    })?;
    Ok(())
}

fn parse_object_file(contents: &str, file_name: &str) -> Result<ObjectRecord> {
    serde_json::from_str::<ObjectRecord>(contents)
        .map_err(|err| anyhow!("failed to parse {file_name} as object file: {err}"))
}

pub(crate) fn write_object_file(
    paths: &DriverPaths,
    record: &ObjectRecord,
    file_name: &str,
) -> Result<()> {
    ensure_store_dirs(paths)?;
    let persisted = serde_json::to_value(record)
        .map_err(|err| anyhow!("failed to serialize object file {file_name}: {err}"))?;
    let serialized = serde_json::to_string(&persisted)
        .map_err(|err| anyhow!("failed to serialize object file {file_name}: {err}"))?;
    let target_path = if record.is_nullified() {
        paths.nullified_objects_dir.join(file_name)
    } else {
        paths.objects_dir.join(file_name)
    };
    fs::write(&target_path, serialized)
        .map_err(|err| anyhow!("failed to write object file {file_name}: {err}"))?;

    let stale_path = if record.is_nullified() {
        paths.objects_dir.join(file_name)
    } else {
        paths.nullified_objects_dir.join(file_name)
    };
    if stale_path != target_path {
        match fs::remove_file(&stale_path) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                eprintln!(
                    "zk-craft: failed to remove stale object file {}: {err}",
                    stale_path.display()
                );
            }
        }
    }

    Ok(())
}

fn load_object_files_from_dir(
    objects: &mut HashMap<String, ObjectRecord>,
    source_dir: &Path,
    in_nullified_dir: bool,
) -> Result<()> {
    if !source_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(source_dir)
        .map_err(|err| anyhow!("failed to read objects directory: {err}"))?
    {
        let entry = entry.map_err(|err| anyhow!("failed to read objects entry: {err}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if !crate::paths::is_dobj_file(&path) {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) => {
                eprintln!("zk-craft: failed to read {file_name}, skipping: {err}");
                continue;
            }
        };

        match parse_object_file(&contents, file_name) {
            Ok(record) => {
                let is_nullified = record.is_nullified();
                if is_nullified != in_nullified_dir {
                    return Err(anyhow!(
                        "invalid object placement for {}: expected {} in {}, found {}",
                        file_name,
                        if in_nullified_dir {
                            "nullified"
                        } else {
                            "live"
                        },
                        source_dir.display(),
                        if is_nullified { "nullified" } else { "live" }
                    ));
                }

                if objects.contains_key(file_name) {
                    return Err(anyhow!(
                        "duplicate object file name detected across object directories: {}",
                        file_name
                    ));
                }

                objects.insert(file_name.to_string(), record);
            }
            Err(err) => eprintln!("zk-craft: failed to parse {file_name}, skipping: {err}"),
        }
    }

    Ok(())
}

pub(crate) fn load_object_files(paths: &DriverPaths) -> Result<Vec<ObjectFileEntry>> {
    ensure_store_dirs(paths)?;
    let mut records_by_file = HashMap::<String, ObjectRecord>::new();
    load_object_files_from_dir(&mut records_by_file, &paths.objects_dir, false)?;
    load_object_files_from_dir(&mut records_by_file, &paths.nullified_objects_dir, true)?;

    let mut objects = records_by_file
        .into_iter()
        .map(|(file_name, record)| ObjectFileEntry { file_name, record })
        .collect::<Vec<_>>();
    objects.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(objects)
}

pub(crate) fn select_object<'a>(
    entries: &'a [ObjectFileEntry],
    selector: &ObjectSelector,
) -> Result<&'a ObjectFileEntry> {
    match selector {
        ObjectSelector::FileName(file_name) => entries
            .iter()
            .find(|entry| entry.file_name == *file_name)
            .ok_or_else(|| anyhow!("object file not found: {file_name}")),
        ObjectSelector::ObjectId(object_id) => entries
            .iter()
            .find(|entry| entry.record.id == *object_id)
            .ok_or_else(|| anyhow!("object not found: {object_id}")),
    }
}

pub(crate) fn matches_query(entry: &ObjectFileEntry, query: &ObjectQuery) -> bool {
    if let Some(class_name) = &query.class_name
        && &entry.record.class_name != class_name
    {
        return false;
    }
    if let Some(status) = query.status
        && status != entry.record.status
    {
        return false;
    }
    if let Some(id) = &query.id
        && &entry.record.id != id
    {
        return false;
    }
    if let Some(file_name) = &query.file_name
        && &entry.file_name != file_name
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::tempdir;
    use txlib::{GroundingWitness, StateRoot};

    use crate::catalog::ActionCatalog;
    use crate::paths::default_paths;
    use crate::pexe_catalog::{PexeCatalog, test_plugin_bytes};
    use crate::types::DriverPaths;

    use super::{load_object_files, write_object_file};
    use crate::object_record::{
        ObjectRecord, ObjectStatus, ensure_extra_pod_deserializers_registered,
        parse_object_record_file,
    };

    fn temp_paths() -> DriverPaths {
        let dir = tempdir().unwrap();
        DriverPaths::from_dobj_root(dir.keep())
    }

    fn dummy_grounding_witness() -> GroundingWitness {
        GroundingWitness::new(
            StateRoot::new(
                1,
                pod2::middleware::EMPTY_HASH,
                pod2::middleware::EMPTY_HASH,
                pod2::middleware::EMPTY_HASH,
            ),
            HashMap::new(),
        )
    }

    fn make_record() -> ObjectRecord {
        ensure_extra_pod_deserializers_registered();
        let catalog = PexeCatalog::from_bytes(
            std::iter::once((
                std::path::PathBuf::from("craft-basics.pexe"),
                test_plugin_bytes(),
            )),
            true,
        )
        .unwrap();
        let outputs = catalog
            .execute_action("FindLog".to_string(), dummy_grounding_witness(), vec![])
            .unwrap();
        let spendable = outputs.obj(0);
        ObjectRecord {
            id: format!("{:#}", spendable.obj.commitment()),
            class_name: "Log".to_string(),
            status: ObjectStatus::Live,
            tx_hash: None,
            obj: spendable.obj,
            evidence: spendable.evidence,
        }
    }

    #[test]
    fn test_write_and_load_round_trip() {
        let paths = temp_paths();
        let record = make_record();
        write_object_file(&paths, &record, "log_test.dobj").unwrap();
        let loaded = load_object_files(&paths).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].file_name, "log_test.dobj");
        assert_eq!(loaded[0].record.id, record.id);
    }

    #[test]
    fn test_parse_object_record_file_round_trip() {
        let paths = temp_paths();
        let record = make_record();
        write_object_file(&paths, &record, "log_test.dobj").unwrap();
        let loaded = parse_object_record_file(&paths.objects_dir.join("log_test.dobj")).unwrap();
        assert_eq!(loaded.id, record.id);
    }

    #[test]
    fn test_default_paths_available() {
        assert!(default_paths().is_ok());
    }
}
