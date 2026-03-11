use super::{
    object_store::{ensure_objects_dirs, load_object_files},
    synchronizer_client::fetch_synchronizer_head,
};
use crate::app_paths;
use serde::Serialize;

use super::super::settings::get_app_settings;

use craft_sdk::Helper;
use pod2::middleware::containers::Dictionary;

use crate::{
    objects::{ObjectRecord, ObjectValidity},
    spec,
};

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MethodArgDto {
    pub kind: String,
    pub label: String,
    pub class_hash: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ObjectMethodDto {
    pub method_name: String,
    pub cpu_cost: String,
    pub reads_block: bool,
    pub args: Vec<MethodArgDto>,
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
pub struct ObjectDataEntryDto {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InventoryItemDto {
    pub id: String,
    pub file_name: String,
    pub emoji: String,
    pub validity: String,
    pub state_root: String,
    pub nullifier: Option<String>,
    pub class_meta: ClassMetaDto,
    pub source_action: Option<SourceActionMetaDto>,
    pub description: Option<String>,
    pub methods: Vec<ObjectMethodDto>,
    pub obj: Vec<ObjectDataEntryDto>,
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

pub(super) fn short_hash(seed: &str) -> String {
    let mut bytes = [0u8; 8];
    for (idx, b) in seed.bytes().enumerate() {
        bytes[idx % 8] = bytes[idx % 8].wrapping_add(b);
    }
    format!(
        "0x{:02x}{:02x}...{:02x}{:02x}",
        bytes[0], bytes[1], bytes[6], bytes[7]
    )
}

fn value_string(raw: String) -> String {
    let trimmed = raw.trim();
    let unquoted = if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    if let Some(raw_inner) = unquoted
        .strip_prefix("Raw(")
        .and_then(|value| value.strip_suffix(')'))
    {
        raw_inner.trim().to_string()
    } else {
        unquoted.to_string()
    }
}

fn object_data_from_object(obj: &Dictionary) -> Vec<(String, String)> {
    let mut data = Vec::new();
    for (key, value) in obj.kvs() {
        data.push((key.name().to_string(), value_string(format!("{value}"))));
    }
    data.sort_by(|a, b| a.0.cmp(&b.0));
    data
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
                    class_hash: short_hash(&class_name),
                })
                .collect(),
            unlocked: true,
        })
        .collect()
}

pub(super) fn to_inventory_item(record: &ObjectRecord) -> InventoryItemDto {
    let class_ui = spec::class_ui_meta(&record.class_name);
    let obj_data = record
        .obj
        .as_ref()
        .map(object_data_from_object)
        .unwrap_or_default();
    InventoryItemDto {
        id: record.id.clone(),
        file_name: record.file_name.clone(),
        emoji: class_ui.emoji.to_string(),
        validity: match record.validity {
            ObjectValidity::Live => "live".to_string(),
            ObjectValidity::Nullified => "nullified".to_string(),
        },
        state_root: record.state_hash.clone(),
        nullifier: record.nullifier.clone(),
        class_meta: ClassMetaDto {
            name: record.class_name.clone(),
            hash: short_hash(&record.class_name),
        },
        source_action: record
            .source_action
            .as_ref()
            .map(|name| SourceActionMetaDto {
                name: name.clone(),
                hash: short_hash(name),
            }),
        description: Some(class_ui.description.to_string()),
        methods: Vec::<ObjectMethodDto>::new(),
        obj: obj_data
            .iter()
            .map(|(key, value)| ObjectDataEntryDto {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
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
    let objects_dir = app_paths::objects_dir(&app)?;
    ensure_objects_dirs(&objects_dir)?;
    let objects = load_object_files(&objects_dir)?;
    let actions = build_action_catalog();
    let app_settings = get_app_settings(app.clone())?;
    let sync_head = fetch_synchronizer_head(&app_settings.synchronizer_api_url);

    if let Err(err) = sync_head {
        eprintln!("zk-craft: synchronizer unavailable during bootstrap: {err}");
    }

    Ok(LoadGuiBootstrapResult {
        objects: objects.iter().map(to_inventory_item).collect(),
        actions,
    })
}
