use anyhow::{Result, anyhow};
use hex::ToHex;
use payload::{decode_hash_hex, encode_hash_hex};
use pod2::middleware::{Hash, StrKey, containers::Dictionary, hash_values};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{ErrorKind, Write},
    path::Path,
};
use wire_types::{ObjectStatus, ObjectSummary, QualifiedName};

use crate::object_record::ObjectRecord;
use crate::paths::DOBJ_EXTENSION;
use crate::types::{DriverPaths, ObjectQuery};

#[derive(Debug, Clone)]
pub(crate) struct ObjectFileEntry {
    pub(crate) file_name: String,
    pub(crate) record: ObjectRecord,
}

fn hash_hex_prefix(hash: &Hash, num_bytes: usize) -> String {
    let full_hex: String = hash.encode_hex();
    full_hex[..num_bytes * 2].to_string()
}

/// Named `<prefix>_<stable id, 8 bytes>_<commitment, 8 bytes>`, so every
/// version of one object shares a leading token.
pub(crate) fn object_file_name(class: &QualifiedName, obj: &Dictionary) -> Result<String> {
    let stable_identifier = obj
        .get(&StrKey::from(txlib::STABLE_IDENTIFIER_FIELD))?
        .map(|value| Hash(value.raw().0))
        .ok_or_else(|| anyhow!("object has no '{}' field", txlib::STABLE_IDENTIFIER_FIELD))?;
    Ok(format!(
        "{}_{}_{}.{DOBJ_EXTENSION}",
        class.file_prefix(),
        hash_hex_prefix(&stable_identifier, 8),
        hash_hex_prefix(&obj.commitment(), 8)
    ))
}

/// Upper bound on an imported file's name, in bytes, leaving room under
/// common 255-byte filesystem limits for a clash suffix.
const MAX_IMPORTED_FILE_NAME_BYTES: usize = 200;

/// Characters some supported platform refuses in a file name.
const FILE_NAME_FORBIDDEN_CHARS: &[char] = &['/', '\\', '<', '>', ':', '"', '|', '?', '*'];

/// Names Windows reserves for devices, whatever their extension.
const WINDOWS_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether `file_name` has the `<prefix>_<16 hex>_<16 hex>.dobj` shape of a
/// name from [`object_file_name`].
fn has_generated_shape(file_name: &str) -> bool {
    let Some(stem) = file_name.strip_suffix(&format!(".{DOBJ_EXTENSION}")) else {
        return false;
    };
    let is_hash_prefix = |part: &str| {
        part.len() == 16
            && part
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    match stem.rsplitn(3, '_').collect::<Vec<_>>()[..] {
        [commitment, stable_identifier, prefix] => {
            !prefix.is_empty() && is_hash_prefix(commitment) && is_hash_prefix(stable_identifier)
        }
        _ => false,
    }
}

/// The name an imported object is filed under when its importer asks for
/// `requested`, `generated` being the name the store would give it. The
/// extension is added when missing. A name another file holds (`is_taken`)
/// gets a numbered suffix, except `generated` itself, which is returned as
/// is. Errors describe why `requested` was refused.
pub(crate) fn imported_file_name(
    requested: &str,
    generated: &str,
    is_taken: impl Fn(&str) -> bool,
) -> Result<String> {
    if requested.trim().is_empty() {
        return Err(anyhow!("file name is empty"));
    }
    if let Some(forbidden) = requested
        .chars()
        .find(|ch| ch.is_control() || FILE_NAME_FORBIDDEN_CHARS.contains(ch))
    {
        return Err(anyhow!(
            "file name {requested:?} contains the character {forbidden:?}"
        ));
    }
    if requested.starts_with('.') {
        return Err(anyhow!("file name {requested:?} starts with '.'"));
    }
    let device_name = requested.split('.').next().unwrap_or_default().trim_end();
    if WINDOWS_DEVICE_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(device_name))
    {
        return Err(anyhow!("file name {requested:?} is reserved on Windows"));
    }
    let extension = format!(".{DOBJ_EXTENSION}");
    let file_name = if requested.ends_with(&extension) {
        requested.to_string()
    } else {
        format!("{requested}{extension}")
    };
    if file_name.len() > MAX_IMPORTED_FILE_NAME_BYTES {
        return Err(anyhow!(
            "file name is longer than {MAX_IMPORTED_FILE_NAME_BYTES} bytes"
        ));
    }
    if file_name == generated {
        return Ok(file_name);
    }
    if has_generated_shape(&file_name) {
        return Err(anyhow!(
            "file name {file_name:?} has the form of a generated object name but is not \
             this object's ({generated})"
        ));
    }
    if !is_taken(&file_name) {
        return Ok(file_name);
    }
    let stem = &file_name[..file_name.len() - extension.len()];
    (2..)
        .map(|number| format!("{stem} {number}{extension}"))
        .find(|candidate| !is_taken(candidate))
        .ok_or_else(|| anyhow!("no free name for {file_name:?}"))
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
    fs::create_dir_all(&paths.transactions_dir).map_err(|err| {
        anyhow!(
            "failed to create transactions directory {}: {err}",
            paths.transactions_dir.display()
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
                    "failed to remove stale object file {}: {err}",
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
                eprintln!("failed to read {file_name}, skipping: {err}");
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
            Err(err) => eprintln!("failed to parse {file_name}, skipping: {err}"),
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

pub(crate) fn matches_query(summary: &ObjectSummary, query: &ObjectQuery) -> bool {
    if let Some(class) = &query.class
        && &summary.class != class
    {
        return false;
    }
    if let Some(wanted) = query.status
        && wanted != summary.status
    {
        return false;
    }
    !(query.exclude_nullified && summary.status == ObjectStatus::Nullified)
}

/// A hash of the object's spending key. Actions that keep the key, as most
/// mutations do unless their script replaces it, keep the fingerprint.
pub(crate) fn key_fingerprint(obj: &Dictionary) -> Option<Hash> {
    let key = obj.get(&StrKey::from("key")).ok()??;
    Some(hash_values(&[key]))
}

/// One line appended per call, so concurrent imports cannot drop each
/// other's entries.
pub(crate) fn record_imported_key(paths: &DriverPaths, fingerprint: &Hash) -> Result<()> {
    let line = format!("{}\n", encode_hash_hex(fingerprint));
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.imported_keys_path)
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .map_err(|err| {
            anyhow!(
                "failed to record imported key in {}: {err}",
                paths.imported_keys_path.display()
            )
        })
}

pub(crate) fn load_imported_keys(paths: &DriverPaths) -> Result<HashSet<Hash>> {
    let contents = match fs::read_to_string(&paths.imported_keys_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(err) => {
            return Err(anyhow!(
                "failed to read {}: {err}",
                paths.imported_keys_path.display()
            ));
        }
    };
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| decode_hash_hex(line.trim()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::tempdir;
    use txlib::{GroundingWitness, StateHeader};

    use crate::catalog::ActionCatalog;
    use crate::paths::default_paths;
    use crate::pexe_catalog::{PexeCatalog, test_plugin_bytes};
    use crate::types::DriverPaths;

    use super::{load_object_files, object_file_name, write_object_file};
    use crate::object_record::{
        FileStatus, ObjectMetadata, ObjectRecord, ensure_extra_pod_deserializers_registered,
        parse_object_record_file,
    };

    fn temp_paths() -> DriverPaths {
        let dir = tempdir().unwrap();
        DriverPaths::from_dobj_root(dir.keep())
    }

    fn dummy_grounding_witness() -> GroundingWitness {
        GroundingWitness::new(
            StateHeader::new(
                1,
                1,
                pod2::middleware::EMPTY_HASH,
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
            .execute_action(
                wire_types::QualifiedName::new("craft-basics", "FindLog"),
                dummy_grounding_witness(),
                vec![],
                &Default::default(),
            )
            .unwrap();
        let tx_final = outputs.tx.dict().commitment();
        let spendable = outputs.obj(0);
        ObjectRecord {
            metadata: ObjectMetadata {
                class: wire_types::QualifiedName::new("craft-basics", "Log"),
                content_hash: spendable.obj.commitment(),
                status: FileStatus::Live,
                tx_final,
                tx_hash: None,
                creation_proof: None,
            },
            obj: spendable.obj,
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
        assert_eq!(
            loaded[0].record.metadata.content_hash,
            record.metadata.content_hash
        );
    }

    #[test]
    fn test_parse_object_record_file_round_trip() {
        let paths = temp_paths();
        let record = make_record();
        write_object_file(&paths, &record, "log_test.dobj").unwrap();
        let loaded = parse_object_record_file(&paths.objects_dir.join("log_test.dobj")).unwrap();
        assert_eq!(loaded.metadata.content_hash, record.metadata.content_hash);
    }

    #[test]
    fn test_object_file_name_requires_stable_identifier() {
        let class = wire_types::QualifiedName::new("craft-basics", "Log");
        assert!(object_file_name(&class, &txlib::new_obj()).is_err());
    }

    #[test]
    fn test_object_file_name_with_stable_identifier() {
        let class = wire_types::QualifiedName::new("craft-basics", "Log");
        let initial = txlib::new_obj();
        let obj = txlib::with_stable_identifier(&initial);
        let stable_hex = format!("{:#}", initial.commitment());
        let content_hex = format!("{:#}", obj.commitment());
        assert_eq!(
            object_file_name(&class, &obj).unwrap(),
            format!(
                "craft-basics__log_{}_{}.dobj",
                &stable_hex[2..18],
                &content_hex[2..18]
            )
        );

        let mut mutated = obj.clone();
        txlib::rekey(&mut mutated);
        let mutated_name = object_file_name(&class, &mutated).unwrap();
        assert!(mutated_name.starts_with(&format!("craft-basics__log_{}_", &stable_hex[2..18])));
        assert_ne!(mutated_name, object_file_name(&class, &obj).unwrap());
    }

    #[test]
    fn test_executed_output_file_name_uses_stable_identifier() {
        let record = make_record();
        let name = object_file_name(&record.metadata.class, &record.obj).unwrap();
        let token = name
            .strip_prefix("craft-basics__log_")
            .and_then(|rest| rest.strip_suffix(".dobj"))
            .unwrap();
        let (stable_part, content_part) = token.split_once('_').unwrap();
        assert_eq!(stable_part.len(), 16);
        assert_eq!(content_part.len(), 16);
    }

    #[test]
    fn test_default_paths_available() {
        assert!(default_paths().is_ok());
    }

    const GENERATED: &str = "bitcraft__pocketknife_48a7b2a2369a49f9_674cf8944de101ba.dobj";

    fn imported_name(requested: &str, taken: &[&str]) -> Result<String, String> {
        super::imported_file_name(requested, GENERATED, |name| taken.contains(&name))
            .map_err(|err| err.to_string())
    }

    #[test]
    fn imported_file_name_keeps_the_requested_name_and_adds_the_extension() {
        assert_eq!(
            imported_name("my knife.dobj", &[]).unwrap(),
            "my knife.dobj"
        );
        assert_eq!(imported_name("my knife", &[]).unwrap(), "my knife.dobj");
        assert_eq!(imported_name("knife.txt", &[]).unwrap(), "knife.txt.dobj");
    }

    #[test]
    fn imported_file_name_suffixes_a_taken_name() {
        assert_eq!(
            imported_name("knife.dobj", &["knife.dobj"]).unwrap(),
            "knife 2.dobj"
        );
        assert_eq!(
            imported_name("knife", &["knife.dobj", "knife 2.dobj"]).unwrap(),
            "knife 3.dobj"
        );
    }

    #[test]
    fn imported_file_name_refuses_unsafe_names() {
        let too_long = "k".repeat(200);
        for unsafe_name in [
            "",
            "   ",
            "../knife.dobj",
            "dir/knife.dobj",
            "dir\\knife.dobj",
            ".knife.dobj",
            ".dobj",
            "knife\n.dobj",
            "knife:1.dobj",
            "knife?.dobj",
            "CON",
            "nul.dobj",
            "Com1.txt",
            too_long.as_str(),
        ] {
            assert!(
                imported_name(unsafe_name, &[]).is_err(),
                "{unsafe_name:?} should be refused"
            );
        }
        assert_eq!(imported_name("console.dobj", &[]).unwrap(), "console.dobj");
    }

    #[test]
    fn imported_file_name_accepts_only_this_objects_generated_name() {
        assert_eq!(imported_name(GENERATED, &[]).unwrap(), GENERATED);
        assert_eq!(
            imported_name(GENERATED, &[GENERATED]).unwrap(),
            GENERATED,
            "the caller decides what a taken generated name means"
        );
        let other_object = "bitcraft__pocketknife_48a7b2a2369a49f9_0000000000000000.dobj";
        let err = imported_name(other_object, &[]).unwrap_err();
        assert!(err.contains("generated object name"), "{err}");
        assert!(imported_name("craft-basics__log_0123456789abcdef_0123456789abcdef", &[]).is_err());
        assert_eq!(
            imported_name("knife_0123456789abcdef.dobj", &[]).unwrap(),
            "knife_0123456789abcdef.dobj"
        );
    }
}
