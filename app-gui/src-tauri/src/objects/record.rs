use craft_sdk::SpendableObject;
use pod2::{frontend::MainPod, middleware::containers::Dictionary};
use serde::de::{DeserializeOwned, Error as _};
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use txlib::Tx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Lifecycle marker for an object tracked by the runtime.
pub(crate) enum ObjectValidity {
    /// Object is available for use as an input to actions.
    Live,
    /// Object has been consumed/nullified by a committed action.
    Nullified,
}

#[derive(Debug)]
pub(crate) struct ObjectRecord {
    /// Stable object identifier (commitment string).
    pub(crate) id: String,
    /// Backing `.dobj` file name on disk.
    pub(crate) file_name: String,
    /// Object class/type name
    pub(crate) class_name: String,
    /// Action that produced this object, when known.
    pub(crate) source_action: Option<String>,
    /// Current lifecycle status for this record.
    pub(crate) validity: ObjectValidity,
    /// State hash associated with this object at creation/observation time.
    pub(crate) state_hash: String,
    /// Nullifier value once object is consumed.
    pub(crate) nullifier: Option<String>,
    /// Pod proof for this object; absent for metadata-only entries.
    pub(crate) pod: Option<MainPod>,
    /// Object payload dictionary; absent for metadata-only entries.
    pub(crate) obj: Option<Dictionary>,
    /// Source transaction witness for this object; absent for metadata-only entries.
    pub(crate) tx: Option<Tx>,
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

impl ObjectValidity {
    fn as_file_str(self) -> &'static str {
        match self {
            ObjectValidity::Live => "live",
            ObjectValidity::Nullified => "nullified",
        }
    }

    fn from_file_str(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "live" => Ok(ObjectValidity::Live),
            "nullified" => Ok(ObjectValidity::Nullified),
            other => Err(format!("invalid object validity: {other}")),
        }
    }
}

impl ObjectRecord {
    pub(crate) fn spendable(&self) -> Result<Option<SpendableObject>, String> {
        match (&self.pod, &self.obj, &self.tx) {
            (None, None, None) => Ok(None),
            (Some(pod), Some(obj), Some(tx)) => Ok(Some(SpendableObject {
                pod: pod.clone(),
                obj: obj.clone(),
                tx: tx.clone(),
            })),
            _ => Err(
                "invalid object record: pod, obj and tx must all be present or all absent"
                    .to_string(),
            ),
        }
    }

    pub(crate) fn require_spendable(&self) -> Result<SpendableObject, String> {
        self.spendable()?
            .ok_or_else(|| format!("object record missing spendable witness: {}", self.id))
    }

    fn to_file_value(&self) -> Result<Value, String> {
        let tx = match (&self.pod, &self.obj, &self.tx) {
            (None, None, None) => None,
            (Some(_), Some(_), Some(tx)) => Some(tx.clone()),
            _ => {
                return Err(
                    "invalid object record: pod, obj and tx must all be present or all absent"
                        .to_string(),
                )
            }
        };
        let state_hash = self
            .obj
            .as_ref()
            .map(|obj| format!("{:#}", obj.commitment()))
            .unwrap_or_else(|| self.state_hash.clone());
        let mut fields = serde_json::Map::new();
        fields.insert("id".to_string(), Value::String(self.id.clone()));
        fields.insert(
            "className".to_string(),
            Value::String(self.class_name.clone()),
        );
        fields.insert(
            "sourceAction".to_string(),
            self.source_action
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        fields.insert(
            "validity".to_string(),
            Value::String(self.validity.as_file_str().to_string()),
        );
        fields.insert("stateHash".to_string(), Value::String(state_hash));
        fields.insert(
            "nullifier".to_string(),
            self.nullifier
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        fields.insert(
            "pod".to_string(),
            match &self.pod {
                Some(pod) => serde_json::to_value(pod)
                    .map_err(|err| format!("failed to serialize spendable.pod: {err}"))?,
                None => Value::Null,
            },
        );
        fields.insert(
            "obj".to_string(),
            match &self.obj {
                Some(obj) => serde_json::to_value(obj)
                    .map_err(|err| format!("failed to serialize spendable.obj: {err}"))?,
                None => Value::Null,
            },
        );
        fields.insert(
            "tx".to_string(),
            match tx {
                Some(tx_record) => serde_json::to_value(tx_record)
                    .map_err(|err| format!("failed to serialize spendable.tx: {err}"))?,
                None => Value::Null,
            },
        );
        Ok(Value::Object(fields))
    }

    fn from_file_value(value: Value, file_name: String) -> Result<Self, String> {
        let fields = value
            .as_object()
            .ok_or_else(|| "invalid object file: expected JSON object".to_string())?;
        let id = parse_required_field::<String>(fields, "id", "id")?;
        let class_name = parse_required_field::<String>(fields, "className", "className")?;
        let source_action = parse_optional_field::<String>(fields, "sourceAction", "sourceAction")?;
        let validity_raw = parse_required_field::<String>(fields, "validity", "validity")?;
        let validity = ObjectValidity::from_file_str(&validity_raw)?;
        let state_hash_from_file =
            parse_required_field::<String>(fields, "stateHash", "stateHash")?;
        let nullifier = parse_optional_field::<String>(fields, "nullifier", "nullifier")?;
        let pod = parse_optional_field::<MainPod>(fields, "pod", "spendable.pod")?;
        let obj = parse_optional_field::<Dictionary>(fields, "obj", "spendable.obj")?;
        let tx = parse_optional_field::<Tx>(fields, "tx", "spendable.tx")?;
        if !matches!(
            (&pod, &obj, &tx),
            (None, None, None) | (Some(_), Some(_), Some(_))
        ) {
            return Err(
                "invalid object file: pod, obj and tx must all be present or all absent"
                    .to_string(),
            );
        }
        let state_hash = obj
            .as_ref()
            .map(|obj| format!("{:#}", obj.commitment()))
            .unwrap_or(state_hash_from_file);

        Ok(Self {
            id,
            file_name,
            class_name,
            source_action,
            validity,
            state_hash,
            nullifier,
            pod,
            obj,
            tx,
        })
    }

    pub(crate) fn with_file_name(mut self, file_name: String) -> Self {
        self.file_name = file_name;
        self
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
        let fields = value
            .as_object()
            .ok_or_else(|| D::Error::custom("invalid object file: expected JSON object"))?;
        if fields.contains_key("txLive")
            || fields.contains_key("txNullifiers")
            || fields.contains_key("txStateRoot")
        {
            return Err(D::Error::custom(
                "invalid object file: legacy txLive/txNullifiers/txStateRoot fields are not supported",
            ));
        }
        ObjectRecord::from_file_value(value, String::new()).map_err(D::Error::custom)
    }
}
