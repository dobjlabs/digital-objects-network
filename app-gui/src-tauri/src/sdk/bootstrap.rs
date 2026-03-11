use super::{
    object_store::{ensure_objects_dirs, load_object_files},
};
use crate::objects::objects_dir;
use serde::Serialize;

use craft_sdk::Helper;
use crate::{objects::ObjectRecord, spec};

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MethodArgDto {
    pub kind: String,
    pub label: String,
    pub class_hash: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClassMetaDto {
    pub name: String,
    pub hash: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SourceActionMetaDto {
    pub name: String,
    pub hash: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InventoryItemDto {
    pub id: String,
    pub file_name: String,
    pub emoji: String,
    pub nullifier: Option<String>,
    pub class_meta: ClassMetaDto,
    pub source_action: SourceActionMetaDto,
    pub description: Option<String>,
    pub obj: serde_json::Value,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RecipeDto {
    pub id: String,
    pub group: String,
    pub name: String,
    pub emoji: String,
    pub hash: String,
    pub verb: String,
    pub desc: String,
    pub cpu: String,
    pub reads_block: bool,
    pub args: Vec<MethodArgDto>,
    pub unlocked: bool,
}

pub(super) fn build_action_catalog() -> Vec<RecipeDto> {
    let helper = Helper::new(spec::dependencies(), spec::actions());
    let action_hashes = helper.action_hashes();

    spec::visible_action_descriptors()
        .into_iter()
        .map(|descriptor| RecipeDto {
            id: descriptor.name.clone(),
            group: String::new(),
            name: descriptor.name.clone(),
            emoji: descriptor.ui.emoji.to_string(),
            hash: action_hashes
                .get(&descriptor.name)
                .map(|hash| format!("{:#}", hash))
                .unwrap_or_default(),
            verb: descriptor.name.clone(),
            desc: descriptor.ui.description.to_string(),
            cpu: descriptor.ui.cpu_cost.to_string(),
            reads_block: descriptor.ui.reads_block,
            args: descriptor
                .input_classes
                .into_iter()
                .map(|class_name| MethodArgDto {
                    kind: "class".to_string(),
                    label: class_name.clone(),
                    class_hash: class_name,
                })
                .collect(),
            unlocked: true,
        })
        .collect()
}

pub(super) fn to_inventory_item(record: &ObjectRecord, file_name: &str) -> InventoryItemDto {
    let class_ui = spec::class_ui_meta(&record.class_name);
    InventoryItemDto {
        id: record.id.clone(),
        file_name: file_name.to_string(),
        emoji: class_ui.emoji.to_string(),
        nullifier: record.nullifier.clone(),
        class_meta: ClassMetaDto {
            name: record.class_name.clone(),
            hash: record.class_name.clone(),
        },
        source_action: SourceActionMetaDto {
            name: record.source_action.clone(),
            hash: record.source_action.clone(),
        },
        description: Some(class_ui.description.to_string()),
        obj: serde_json::to_value(&record.obj).expect("object dictionary should serialize"),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadGuiBootstrapResult {
    pub objects: Vec<InventoryItemDto>,
    pub actions: Vec<RecipeDto>,
}

#[tauri::command]
pub async fn load_gui_bootstrap(app: tauri::AppHandle) -> Result<LoadGuiBootstrapResult, String> {
    let objects_dir = objects_dir(&app)?;
    ensure_objects_dirs(&objects_dir)?;
    let objects = load_object_files(&objects_dir)?;
    let actions = build_action_catalog();

    Ok(LoadGuiBootstrapResult {
        objects: objects
            .iter()
            .map(|entry| to_inventory_item(&entry.record, &entry.file_name))
            .collect(),
        actions,
    })
}
