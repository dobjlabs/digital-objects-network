use craft_sdk::SpendableObject;
use pod2::{frontend::MainPod, middleware::containers::Dictionary};
use serde::de::{DeserializeOwned, Error as _};
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::{fs, path::Path};
use txlib::Tx;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectStatus {
    Unknown,
    Pending,
    Live,
    Nullified,
}

#[derive(Debug, Clone)]
pub struct ObjectRecord {
    pub id: String,
    /// Object class/type name
    pub class_name: String,
    /// Lifecycle status of this object.
    pub status: ObjectStatus,
    /// Optional Ethereum transaction hash for the blob that anchored this object.
    /// Set once the relayer confirms on-chain inclusion.
    pub tx_hash: Option<String>,
    /// Pod proof for this object
    pub pod: MainPod,
    /// Object payload dictionary
    pub obj: Dictionary,
    /// Source transaction witness for this object
    pub tx: Tx,
}

fn parse_required_field<T: DeserializeOwned>(
    fields: &serde_json::Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<T, String> {
    let value = fields
        .get(key)
        .cloned()
        .ok_or_else(|| format!("invalid object file: missing {key}"))?;
    serde_json::from_value(value).map_err(|err| format!("failed to deserialize {context}: {err}"))
}

impl ObjectRecord {
    pub(crate) fn is_nullified(&self) -> bool {
        self.status == ObjectStatus::Nullified
    }

    pub(crate) fn spendable(&self) -> SpendableObject {
        SpendableObject {
            pod: self.pod.clone(),
            obj: self.obj.clone(),
            tx: self.tx.clone(),
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

    fn to_file_value(&self) -> Result<Value, String> {
        let mut fields = serde_json::Map::new();
        fields.insert("id".to_string(), Value::String(self.id.clone()));
        fields.insert(
            "className".to_string(),
            Value::String(self.class_name.clone()),
        );
        fields.insert(
            "status".to_string(),
            serde_json::to_value(self.status)
                .map_err(|err| format!("failed to serialize status: {err}"))?,
        );
        if let Some(ref hash) = self.tx_hash {
            fields.insert("txHash".to_string(), Value::String(hash.clone()));
        }
        fields.insert(
            "pod".to_string(),
            serde_json::to_value(&self.pod)
                .map_err(|err| format!("failed to serialize spendable.pod: {err}"))?,
        );
        fields.insert(
            "obj".to_string(),
            serde_json::to_value(&self.obj)
                .map_err(|err| format!("failed to serialize spendable.obj: {err}"))?,
        );
        fields.insert(
            "tx".to_string(),
            serde_json::to_value(&self.tx)
                .map_err(|err| format!("failed to serialize spendable.tx: {err}"))?,
        );
        Ok(Value::Object(fields))
    }

    fn from_file_value(value: Value) -> Result<Self, String> {
        let fields = value
            .as_object()
            .ok_or_else(|| "invalid object file: expected JSON object".to_string())?;
        let id = parse_required_field::<String>(fields, "id", "id")?;
        let class_name = parse_required_field::<String>(fields, "className", "className")?;
        let status = parse_required_field::<ObjectStatus>(fields, "status", "status")?;
        let tx_hash: Option<String> = match fields.get("txHash") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        };
        let pod = parse_required_field::<MainPod>(fields, "pod", "spendable.pod")?;
        let obj = parse_required_field::<Dictionary>(fields, "obj", "spendable.obj")?;
        let tx = parse_required_field::<Tx>(fields, "tx", "spendable.tx")?;

        Ok(Self {
            id,
            class_name,
            status,
            tx_hash,
            pod,
            obj,
            tx,
        })
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

    use pod2::middleware::{self, BackendError, Hash, Params, Pod, VDSet};

    static REGISTER_EXTRA_DESERIALIZERS: Once = Once::new();

    REGISTER_EXTRA_DESERIALIZERS.call_once(|| {
        fn deserialize_mock_intro(
            params: Params,
            data: Value,
            vd_set: VDSet,
            sts_hash: Hash,
        ) -> Result<Box<dyn Pod>, BackendError> {
            Ok(Box::new(
                <pod2utils::mockintro::MockIntroPod as Pod>::deserialize_data(
                    params, data, vd_set, sts_hash,
                )?,
            ))
        }

        middleware::register_pod_deserializer(999, deserialize_mock_intro);
    });
}

impl Serialize for ObjectRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = self.to_file_value().map_err(S::Error::custom)?;
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ObjectRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        ObjectRecord::from_file_value(value).map_err(D::Error::custom)
    }
}
