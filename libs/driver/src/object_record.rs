use pod2::middleware::{Hash, containers::Dictionary};
use sdk::SpendableObject;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

use wire_types::{ObjectStatus, QualifiedName};

/// Persisted chain status. Pending status is derived from transaction records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Unknown,
    Live,
    Nullified,
}

impl From<FileStatus> for ObjectStatus {
    fn from(status: FileStatus) -> Self {
        match status {
            FileStatus::Unknown => ObjectStatus::Unknown,
            FileStatus::Live => ObjectStatus::Live,
            FileStatus::Nullified => ObjectStatus::Nullified,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRecord {
    pub content_hash: Hash,
    /// The class this object belongs to. Plugin-scoped so two plugins with
    /// the same bare class name stay distinguishable.
    pub class: QualifiedName,
    pub status: FileStatus,
    /// Object payload dictionary
    pub obj: Dictionary,
    /// Producing transaction's commitment; identifies its local record, if any.
    pub tx_final: Hash,
}

impl ObjectRecord {
    pub(crate) fn is_nullified(&self) -> bool {
        self.status == FileStatus::Nullified
    }

    pub(crate) fn spendable(&self) -> SpendableObject {
        SpendableObject {
            obj: self.obj.clone(),
        }
    }

    pub(crate) fn fields_map(&self) -> std::collections::HashMap<String, serde_json::Value> {
        match serde_json::to_value(&self.obj) {
            Ok(serde_json::Value::Object(map)) => map.into_iter().collect(),
            Ok(value) => {
                let mut fields = std::collections::HashMap::new();
                fields.insert("_raw".to_string(), value);
                fields
            }
            Err(_) => std::collections::HashMap::new(),
        }
    }
}

pub fn parse_object_record_file(path: &Path) -> anyhow::Result<ObjectRecord> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow::anyhow!("invalid input path (missing file name): {}", path.display())
        })?;
    let contents = fs::read_to_string(path)
        .map_err(|err| anyhow::anyhow!("failed to read input file {}: {err}", path.display()))?;
    serde_json::from_str::<ObjectRecord>(&contents)
        .map_err(|err| anyhow::anyhow!("failed to parse {file_name} as object file: {err}"))
}

#[cfg(test)]
pub(crate) fn ensure_extra_pod_deserializers_registered() {
    use std::sync::Once;

    use pod2::middleware::{self, BackendError, Params, Pod, VDSet};
    use serde_json::Value;

    static REGISTER_EXTRA_DESERIALIZERS: Once = Once::new();

    REGISTER_EXTRA_DESERIALIZERS.call_once(|| {
        fn deserialize_mock_intro(
            params: Params,
            data: Value,
            vd_set: VDSet,
        ) -> Result<Box<dyn Pod>, BackendError> {
            Ok(Box::new(
                <pod2utils::mockintro::MockIntroPod as Pod>::deserialize_data(
                    params, data, vd_set,
                )?,
            ))
        }

        middleware::register_pod_deserializer(999, deserialize_mock_intro);
    });
}
