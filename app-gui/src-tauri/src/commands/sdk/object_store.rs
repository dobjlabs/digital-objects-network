use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::state::{ObjectRecord, RuntimeValidity};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ObjectFileMetadata {
    pub file_name: String,
    pub class_name: String,
    pub validity: String,
}

const NULLIFIED_DIR_NAME: &str = ".nullified";

pub(super) fn nullified_objects_dir(objects_dir: &Path) -> PathBuf {
    objects_dir.join(NULLIFIED_DIR_NAME)
}

pub(super) fn ensure_objects_dirs(objects_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(objects_dir)
        .map_err(|err| format!("failed to create objects directory: {err}"))?;
    fs::create_dir_all(nullified_objects_dir(objects_dir))
        .map_err(|err| format!("failed to create nullified directory: {err}"))?;
    Ok(())
}

fn take_required_string(
    fields: &mut Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<String, String> {
    match fields.remove(key) {
        Some(Value::String(value)) => Ok(value),
        Some(Value::Null) => Err(format!("missing {key} in {context}")),
        Some(_) => Err(format!("invalid {key} in {context}: expected string")),
        None => Err(format!("missing {key} in {context}")),
    }
}

fn validity_from_str(raw: &str, context: &str) -> Result<RuntimeValidity, String> {
    match raw {
        "live" => Ok(RuntimeValidity::Live),
        "nullified" => Ok(RuntimeValidity::Nullified),
        other => Err(format!("invalid object validity in {context}: {other}")),
    }
}

fn parse_object_file(contents: &str, file_name: &str) -> Result<ObjectRecord, String> {
    let record = serde_json::from_str::<ObjectRecord>(contents)
        .map_err(|err| format!("failed to parse {file_name} as object file: {err}"))?;
    Ok(record.with_file_name(file_name.to_string()))
}

pub(crate) fn read_object_file_metadata(path: &Path) -> Result<ObjectFileMetadata, String> {
    if !path.exists() {
        return Err(format!("selected file does not exist: {}", path.display()));
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid input path (missing file name): {}", path.display()))?
        .to_string();
    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read selected file {}: {err}", path.display()))?;
    let mut parsed = serde_json::from_str::<Map<String, Value>>(&contents)
        .map_err(|err| format!("invalid .dobj JSON in {}: {err}", path.display()))?;

    let class_name = take_required_string(&mut parsed, "className", &path.display().to_string())?
        .trim()
        .to_string();
    if class_name.is_empty() {
        return Err(format!("missing className in {}", path.display()));
    }

    let validity_raw = take_required_string(&mut parsed, "validity", &path.display().to_string())?
        .trim()
        .to_lowercase();
    if validity_raw.is_empty() {
        return Err(format!("missing validity in {}", path.display()));
    }
    let context = path.display().to_string();
    let validity = validity_from_str(&validity_raw, &context)?;

    Ok(ObjectFileMetadata {
        file_name,
        class_name,
        validity: match validity {
            RuntimeValidity::Live => "live".to_string(),
            RuntimeValidity::Nullified => "nullified".to_string(),
        },
    })
}

pub(super) fn write_object_file(record: &ObjectRecord, objects_dir: &Path) -> Result<(), String> {
    ensure_objects_dirs(objects_dir)?;
    let nullified_dir = nullified_objects_dir(objects_dir);

    let persisted = serde_json::to_value(record).map_err(|err| {
        format!(
            "failed to serialize object file {}: {err}",
            record.file_name
        )
    })?;
    let serialized = serde_json::to_string_pretty(&persisted).map_err(|err| {
        format!(
            "failed to serialize object file {}: {err}",
            record.file_name
        )
    })?;
    let target_path = match record.validity {
        RuntimeValidity::Live => objects_dir.join(&record.file_name),
        RuntimeValidity::Nullified => nullified_dir.join(&record.file_name),
    };
    fs::write(&target_path, serialized)
        .map_err(|err| format!("failed to write object file {}: {err}", record.file_name))?;

    let stale_path = match record.validity {
        RuntimeValidity::Live => nullified_dir.join(&record.file_name),
        RuntimeValidity::Nullified => objects_dir.join(&record.file_name),
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
                let expected_validity = if in_nullified_dir {
                    RuntimeValidity::Nullified
                } else {
                    RuntimeValidity::Live
                };

                if record.validity != expected_validity {
                    return Err(format!(
                        "invalid object validity placement for {}: expected {} in {}, found {}",
                        record.file_name,
                        match expected_validity {
                            RuntimeValidity::Live => "live",
                            RuntimeValidity::Nullified => "nullified",
                        },
                        source_dir.display(),
                        match record.validity {
                            RuntimeValidity::Live => "live",
                            RuntimeValidity::Nullified => "nullified",
                        }
                    ));
                }

                if objects.contains_key(&record.file_name) {
                    return Err(format!(
                        "duplicate object file name detected across object directories: {}",
                        record.file_name
                    ));
                }

                objects.insert(record.file_name.clone(), record);
            }
            Err(err) => eprintln!("zk-craft: failed to parse {file_name}, skipping: {err}"),
        }
    }

    Ok(())
}

pub(super) fn load_object_files(objects_dir: &Path) -> Result<Vec<ObjectRecord>, String> {
    let mut records_by_file = HashMap::<String, ObjectRecord>::new();
    load_object_files_from_dir(&mut records_by_file, objects_dir, false)?;
    load_object_files_from_dir(
        &mut records_by_file,
        &nullified_objects_dir(objects_dir),
        true,
    )?;

    let mut objects = records_by_file.into_values().collect::<Vec<_>>();
    objects.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(objects)
}

pub(super) fn parse_object_file_from_path(path: &Path) -> Result<ObjectRecord, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid input path (missing file name): {}", path.display()))?;
    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read input file {}: {err}", path.display()))?;
    parse_object_file(&contents, file_name)
}
