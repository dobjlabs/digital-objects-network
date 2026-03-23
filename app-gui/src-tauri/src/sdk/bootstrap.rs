use super::object_store::{ensure_objects_dirs, load_object_files, write_object_file};
use crate::error::CommandError;
use crate::objects::objects_dir;
use craft_sdk::Helper;
use pod2::middleware::Hash;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use txlib::object_nullifier_hash;

use crate::{objects::ObjectRecord, settings::get_app_settings, spec};

use super::synchronizer_client::{
    encode_hash_hex, fetch_synchronizer_head, fetch_synchronizer_membership_with_nullifiers,
    SynchronizerMembership,
};

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    pub id: String,
    pub file_name: String,
    pub class_name: String,
    pub class_hash: String,
    pub emoji: String,
    pub nullifier: Option<String>,
    pub grounded: bool,
    pub description: Option<String>,
    pub obj: serde_json::Value,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub id: String,
    pub emoji: String,
    pub hash: String,
    pub input_class_hashes: Vec<String>,
    pub description: String,
    pub cpu_cost: String,
    pub reads_block: bool,
    pub input_classes: Vec<String>,
}

pub(super) fn build_action_catalog(
    action_hashes: &HashMap<String, Hash>,
    class_hashes: &HashMap<String, Hash>,
) -> Vec<Action> {
    spec::visible_action_descriptors()
        .into_iter()
        .map(|descriptor| Action {
            id: descriptor.name.clone(),
            emoji: descriptor.ui.emoji.to_string(),
            hash: action_hashes
                .get(&descriptor.name)
                .map(|hash| format!("{:#}", hash))
                .unwrap_or_default(),
            input_class_hashes: descriptor
                .input_classes
                .iter()
                .map(|class_name| {
                    class_hashes
                        .get(class_name)
                        .map(|hash| format!("{:#}", hash))
                        .unwrap_or_default()
                })
                .collect(),
            description: descriptor.ui.description.to_string(),
            cpu_cost: descriptor.ui.cpu_cost.to_string(),
            reads_block: descriptor.ui.reads_block,
            input_classes: descriptor.input_classes,
        })
        .collect()
}

pub(super) fn to_inventory_object(
    record: &ObjectRecord,
    file_name: &str,
    class_hashes: &HashMap<String, Hash>,
    grounded_txs: &HashSet<Hash>,
) -> InventoryObject {
    let class_ui = spec::class_ui_meta(&record.class_name);
    let source_tx_hash = record.spendable().tx.dict().commitment();
    let grounded = record.is_nullified() || grounded_txs.contains(&source_tx_hash);
    InventoryObject {
        id: record.id.clone(),
        file_name: file_name.to_string(),
        class_name: record.class_name.clone(),
        class_hash: class_hashes
            .get(&record.class_name)
            .map(|hash| format!("{:#}", hash))
            .unwrap_or_default(),
        emoji: class_ui.emoji.to_string(),
        nullifier: record.nullifier.clone(),
        grounded,
        description: Some(class_ui.description.to_string()),
        obj: serde_json::to_value(&record.obj).expect("object dictionary should serialize"),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadGuiInventoryResult {
    pub inventory: Vec<InventoryObject>,
    pub actions: Vec<Action>,
}

/// Reconcile live objects against on-chain state.
/// If a live object's nullifier is already spent on-chain, auto-nullify it on disk.
fn reconcile_objects(
    objects_dir: &Path,
    objects: &mut [super::object_store::ObjectFileEntry],
    on_chain_nullifiers: &HashSet<Hash>,
) {
    for entry in objects.iter_mut() {
        if entry.record.is_nullified() {
            continue;
        }
        let nullifier_hash = match object_nullifier_hash(&entry.record.obj) {
            Ok(hash) => hash,
            Err(_) => continue,
        };
        if !on_chain_nullifiers.contains(&nullifier_hash) {
            continue;
        }
        let nullified_record = ObjectRecord {
            id: entry.record.id.clone(),
            class_name: entry.record.class_name.clone(),
            source_action: entry.record.source_action.clone(),
            nullifier: Some(encode_hash_hex(&nullifier_hash)),
            pod: entry.record.pod.clone(),
            obj: entry.record.obj.clone(),
            tx: entry.record.tx.clone(),
        };
        if let Err(e) = write_object_file(&nullified_record, &entry.file_name, objects_dir) {
            eprintln!(
                "zk-craft: reconcile failed to nullify {}: {e}",
                entry.file_name
            );
            continue;
        }
        entry.record = nullified_record;
    }
}

#[tauri::command]
pub async fn load_gui_inventory(
    app: tauri::AppHandle,
) -> Result<LoadGuiInventoryResult, CommandError> {
    let objects_dir = objects_dir(&app)?;
    ensure_objects_dirs(&objects_dir)?;
    let mut objects = load_object_files(&objects_dir)?;
    let helper = Helper::new(spec::dependencies(), spec::actions());
    let action_hashes = helper.action_hashes();
    let class_hashes = helper.class_hashes();
    let actions = build_action_catalog(&action_hashes, &class_hashes);

    let app_settings = get_app_settings(app)?;
    let source_tx_hashes = objects
        .iter()
        .map(|entry| entry.record.spendable().tx.dict().commitment())
        .collect::<HashSet<_>>();
    let live_nullifiers = objects
        .iter()
        .filter(|entry| !entry.record.is_nullified())
        .filter_map(|entry| object_nullifier_hash(&entry.record.obj).ok())
        .collect::<HashSet<_>>();

    let membership = fetch_synchronizer_membership_with_nullifiers(
        &app_settings.synchronizer_api_url,
        &source_tx_hashes.iter().copied().collect::<Vec<_>>(),
        &live_nullifiers.iter().copied().collect::<Vec<_>>(),
    )
    .unwrap_or_else(|err| {
        eprintln!("zk-craft: failed to load synchronizer inventory membership: {err}");
        SynchronizerMembership {
            grounded_txs: HashSet::new(),
            on_chain_nullifiers: HashSet::new(),
        }
    });

    reconcile_objects(&objects_dir, &mut objects, &membership.on_chain_nullifiers);

    Ok(LoadGuiInventoryResult {
        inventory: objects
            .iter()
            .map(|entry| {
                to_inventory_object(
                    &entry.record,
                    &entry.file_name,
                    &class_hashes,
                    &membership.grounded_txs,
                )
            })
            .collect(),
        actions,
    })
}

#[tauri::command]
pub async fn get_global_state_root(app: tauri::AppHandle) -> Result<String, CommandError> {
    let app_settings = get_app_settings(app)?;
    let sync_head = fetch_synchronizer_head(&app_settings.synchronizer_api_url)?;
    Ok(encode_hash_hex(&sync_head.current_gsr))
}
