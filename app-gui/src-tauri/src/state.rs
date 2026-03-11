use std::sync::{Arc, Mutex};
use std::time::Instant;

use craft_sdk::SpendableObject;
use pod2::{
    frontend::MainPod,
    middleware::containers::{Array, Dictionary, Set},
};
use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sysinfo::{Pid, ProcessesToUpdate, System};
use txlib::{StateRoot, Tx};

/// Shared runtime state used by the CPU sampling command.
///
/// This tracks the current process in `sysinfo`, plus rolling CPU totals so
/// the frontend can render both instantaneous usage and accumulated CPU time.
pub(crate) struct CpuMonitor {
    /// PID for this Tauri process.
    pub(crate) pid: Pid,
    /// `sysinfo` system handle used to refresh process CPU stats.
    pub(crate) system: Mutex<System>,
    /// Accumulated CPU time in core-seconds.
    pub(crate) total_cpu_secs: Mutex<f64>,
    /// Wall-clock time of the previous sample.
    pub(crate) last_sample_at: Mutex<Option<Instant>>,
    /// Ensures persisted CPU totals are loaded from disk only once.
    pub(crate) total_loaded: Mutex<bool>,
}

impl CpuMonitor {
    pub(crate) fn new() -> Self {
        let pid = Pid::from_u32(std::process::id());
        let mut system = System::new_all();
        let _ = system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        Self {
            pid,
            system: Mutex::new(system),
            total_cpu_secs: Mutex::new(0.0),
            last_sample_at: Mutex::new(None),
            total_loaded: Mutex::new(false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Lifecycle marker for an object tracked by the runtime.
pub(crate) enum RuntimeValidity {
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
    pub(crate) validity: RuntimeValidity,
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StateRootFileRecord {
    block_number: i64,
    transactions: Set,
    nullifiers: Set,
    gsrs: Array,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TxFileRecord {
    live: Set,
    nullifiers: Set,
    state_root: StateRootFileRecord,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectFileRecord {
    id: String,
    class_name: String,
    source_action: Option<String>,
    validity: String,
    state_hash: String,
    nullifier: Option<String>,
    pod: Option<MainPod>,
    obj: Option<Dictionary>,
    tx: Option<TxFileRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectFileRecordRaw {
    id: String,
    class_name: String,
    source_action: Option<String>,
    validity: String,
    state_hash: String,
    nullifier: Option<String>,
    pod: Option<Value>,
    obj: Option<Value>,
    tx: Option<Value>,
}

impl RuntimeValidity {
    fn as_file_str(self) -> &'static str {
        match self {
            RuntimeValidity::Live => "live",
            RuntimeValidity::Nullified => "nullified",
        }
    }

    fn from_file_str(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "live" => Ok(RuntimeValidity::Live),
            "nullified" => Ok(RuntimeValidity::Nullified),
            other => Err(format!("invalid object validity: {other}")),
        }
    }
}

fn tx_to_file_record(tx: &Tx) -> TxFileRecord {
    TxFileRecord {
        live: tx.live.clone(),
        nullifiers: tx.nullifiers.clone(),
        state_root: StateRootFileRecord {
            block_number: tx.state_root.block_number,
            transactions: tx.state_root.transactions.clone(),
            nullifiers: tx.state_root.nullifiers.clone(),
            gsrs: tx.state_root.gsrs.clone(),
        },
    }
}

fn tx_from_file_record(tx: TxFileRecord) -> Tx {
    Tx {
        live: tx.live,
        nullifiers: tx.nullifiers,
        state_root: Arc::new(StateRoot {
            block_number: tx.state_root.block_number,
            transactions: tx.state_root.transactions,
            nullifiers: tx.state_root.nullifiers,
            gsrs: tx.state_root.gsrs,
        }),
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

    fn to_file_record(&self) -> Result<ObjectFileRecord, String> {
        let tx = match (&self.pod, &self.obj, &self.tx) {
            (None, None, None) => None,
            (Some(_), Some(_), Some(tx)) => Some(tx_to_file_record(tx)),
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

        Ok(ObjectFileRecord {
            id: self.id.clone(),
            class_name: self.class_name.clone(),
            source_action: self.source_action.clone(),
            validity: self.validity.as_file_str().to_string(),
            state_hash,
            nullifier: self.nullifier.clone(),
            pod: self.pod.clone(),
            obj: self.obj.clone(),
            tx,
        })
    }

    fn from_file_record_raw(
        record: ObjectFileRecordRaw,
        file_name: String,
    ) -> Result<Self, String> {
        let validity = RuntimeValidity::from_file_str(&record.validity)?;
        let (pod, obj, tx) = match (record.pod, record.obj, record.tx) {
            (None, None, None) => (None, None, None),
            (Some(pod), Some(obj), Some(tx)) => {
                let pod = serde_json::from_value::<MainPod>(pod)
                    .map_err(|err| format!("failed to deserialize spendable.pod: {err}"))?;
                let obj = serde_json::from_value::<Dictionary>(obj)
                    .map_err(|err| format!("failed to deserialize spendable.obj: {err}"))?;
                let tx_record = serde_json::from_value::<TxFileRecord>(tx)
                    .map_err(|err| format!("failed to deserialize spendable.tx: {err}"))?;
                (Some(pod), Some(obj), Some(tx_from_file_record(tx_record)))
            }
            _ => {
                return Err(
                    "invalid object file: pod, obj and tx must all be present or all absent"
                        .to_string(),
                )
            }
        };
        let state_hash = obj
            .as_ref()
            .map(|obj| format!("{:#}", obj.commitment()))
            .unwrap_or(record.state_hash);

        Ok(Self {
            id: record.id,
            file_name,
            class_name: record.class_name,
            source_action: record.source_action,
            validity,
            state_hash,
            nullifier: record.nullifier,
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
        let file_record = self.to_file_record().map_err(S::Error::custom)?;
        file_record.serialize(serializer)
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

        let file_record =
            serde_json::from_value::<ObjectFileRecordRaw>(value).map_err(D::Error::custom)?;
        ObjectRecord::from_file_record_raw(file_record, String::new()).map_err(D::Error::custom)
    }
}

#[derive(Debug)]
/// Shared mutable runtime synchronization state.
pub(crate) struct ObjectsRuntimeState {
    /// Guard to prevent concurrent action runs.
    pub(crate) run_in_progress: bool,
}

pub(crate) struct ObjectsRuntime {
    pub(crate) inner: Mutex<ObjectsRuntimeState>,
}

impl ObjectsRuntime {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(ObjectsRuntimeState {
                run_in_progress: false,
            }),
        }
    }
}
