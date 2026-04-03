use pod2::{frontend::MainPod, middleware::containers::Dictionary};
use serde::de::{DeserializeOwned, Error as _};
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::{fs, path::Path};
use txlib::Tx;

#[derive(Debug)]
pub(crate) struct ObjectRecord {
    pub(crate) id: String,
    /// Object class/type name
    pub(crate) class_name: String,
    /// Action that produced this object.
    pub(crate) source_action: String,
    /// Nullifier value once object is consumed.
    pub(crate) nullifier: Option<String>,
    /// Pod proof for this object
    pub(crate) pod: MainPod,
    /// Object payload dictionary
    pub(crate) obj: Dictionary,
    /// Source transaction witness for this object
    pub(crate) tx: Tx,
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

fn parse_optional_field<T: DeserializeOwned>(
    fields: &serde_json::Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Option<T>, String> {
    match fields.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|err| format!("failed to deserialize {context}: {err}")),
    }
}

fn parse_object_file(contents: &str, file_name: &str) -> Result<ObjectRecord, String> {
    serde_json::from_str::<ObjectRecord>(contents)
        .map_err(|err| format!("failed to parse {file_name} as object file: {err}"))
}

pub(crate) fn parse_object_file_from_path(path: &Path) -> anyhow::Result<ObjectRecord> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid input path (missing file name): {}", path.display()))?;
    let contents = fs::read_to_string(path)
        .map_err(|err| anyhow::anyhow!("failed to read input file {}: {err}", path.display()))?;
    parse_object_file(&contents, file_name).map_err(anyhow::Error::msg)
}

impl ObjectRecord {
    fn to_file_value(&self) -> Result<Value, String> {
        let mut fields = serde_json::Map::new();
        fields.insert("id".to_string(), Value::String(self.id.clone()));
        fields.insert(
            "className".to_string(),
            Value::String(self.class_name.clone()),
        );
        fields.insert(
            "sourceAction".to_string(),
            Value::String(self.source_action.clone()),
        );
        fields.insert(
            "nullifier".to_string(),
            self.nullifier
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
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
        let source_action = parse_required_field::<String>(fields, "sourceAction", "sourceAction")?;
        let nullifier = parse_optional_field::<String>(fields, "nullifier", "nullifier")?;
        let pod = parse_required_field::<MainPod>(fields, "pod", "spendable.pod")?;
        let obj = parse_required_field::<Dictionary>(fields, "obj", "spendable.obj")?;
        let tx = parse_required_field::<Tx>(fields, "tx", "spendable.tx")?;

        Ok(Self {
            id,
            class_name,
            source_action,
            nullifier,
            pod,
            obj,
            tx,
        })
    }
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
