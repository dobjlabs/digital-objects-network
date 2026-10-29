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

/// A `.dobj` file: what the driver knows about the object, then the object.
/// Files written before `metadata` existed kept its fields at the top level,
/// and still parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "serde_json::Value")]
pub struct ObjectRecord {
    pub metadata: ObjectMetadata,
    /// Object payload dictionary
    pub obj: Dictionary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectMetadata {
    /// The class this object belongs to. Plugin-scoped so two plugins with
    /// the same bare class name stay distinguishable.
    pub class: QualifiedName,
    pub content_hash: Hash,
    pub status: FileStatus,
    /// Producing transaction's commitment; identifies its local record, if any.
    pub tx_final: Hash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    /// Base64 [`crate::creation_proof::CreationProof`] that the object is in
    /// the created set of a state root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_proof: Option<String>,
}

#[derive(Deserialize)]
struct NestedObjectRecord {
    metadata: ObjectMetadata,
    obj: Dictionary,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlatObjectRecord {
    content_hash: Hash,
    class: QualifiedName,
    status: FileStatus,
    #[serde(default)]
    tx_hash: Option<String>,
    obj: Dictionary,
    tx_final: Hash,
}

impl TryFrom<serde_json::Value> for ObjectRecord {
    type Error = serde_json::Error;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        if value.get("metadata").is_some() {
            let NestedObjectRecord { metadata, obj } = serde_json::from_value(value)?;
            return Ok(Self { metadata, obj });
        }
        let flat: FlatObjectRecord = serde_json::from_value(value)?;
        Ok(Self {
            metadata: ObjectMetadata {
                class: flat.class,
                content_hash: flat.content_hash,
                status: flat.status,
                tx_final: flat.tx_final,
                tx_hash: flat.tx_hash,
                creation_proof: None,
            },
            obj: flat.obj,
        })
    }
}

impl ObjectRecord {
    pub(crate) fn is_nullified(&self) -> bool {
        self.metadata.status == FileStatus::Nullified
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
