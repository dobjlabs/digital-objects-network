use std::{
    collections::HashMap,
    fs,
    path::Path,
};

use crate::objects::{nullified_objects_dir, ObjectRecord};

pub(super) struct ObjectFileEntry {
    pub(super) file_name: String,
    pub(super) record: ObjectRecord,
}

pub(super) fn ensure_objects_dirs(objects_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(objects_dir)
        .map_err(|err| format!("failed to create objects directory: {err}"))?;
    fs::create_dir_all(nullified_objects_dir(objects_dir))
        .map_err(|err| format!("failed to create nullified directory: {err}"))?;
    Ok(())
}

fn parse_object_file(contents: &str, file_name: &str) -> Result<ObjectRecord, String> {
    serde_json::from_str::<ObjectRecord>(contents)
        .map_err(|err| format!("failed to parse {file_name} as object file: {err}"))
}

pub(super) fn write_object_file(
    record: &ObjectRecord,
    file_name: &str,
    objects_dir: &Path,
) -> Result<(), String> {
    ensure_objects_dirs(objects_dir)?;
    let nullified_dir = nullified_objects_dir(objects_dir);

    let persisted = serde_json::to_value(record)
        .map_err(|err| format!("failed to serialize object file {file_name}: {err}"))?;
    let serialized = serde_json::to_string_pretty(&persisted)
        .map_err(|err| format!("failed to serialize object file {file_name}: {err}"))?;
    let target_path = if record.is_nullified() {
        nullified_dir.join(file_name)
    } else {
        objects_dir.join(file_name)
    };
    fs::write(&target_path, serialized)
        .map_err(|err| format!("failed to write object file {file_name}: {err}"))?;

    let stale_path = if record.is_nullified() {
        objects_dir.join(file_name)
    } else {
        nullified_dir.join(file_name)
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
) -> Result<(), String> {
    if !source_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(source_dir)
        .map_err(|err| format!("failed to read objects directory: {err}"))?
    {
        let entry = entry.map_err(|err| format!("failed to read objects entry: {err}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_dobj = path.extension().and_then(|ext| ext.to_str()) == Some("dobj");
        if !is_dobj {
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
                    return Err(format!(
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
                    return Err(format!(
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

pub(super) fn load_object_files(objects_dir: &Path) -> Result<Vec<ObjectFileEntry>, String> {
    let mut records_by_file = HashMap::<String, ObjectRecord>::new();
    load_object_files_from_dir(&mut records_by_file, objects_dir, false)?;
    load_object_files_from_dir(
        &mut records_by_file,
        &nullified_objects_dir(objects_dir),
        true,
    )?;

    let mut objects = records_by_file
        .into_iter()
        .map(|(file_name, record)| ObjectFileEntry { file_name, record })
        .collect::<Vec<_>>();
    objects.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(objects)
}

pub(crate) fn parse_object_file_from_path(path: &Path) -> Result<ObjectRecord, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid input path (missing file name): {}", path.display()))?;
    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read input file {}: {err}", path.display()))?;
    parse_object_file(&contents, file_name)
}
