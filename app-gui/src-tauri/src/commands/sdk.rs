use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use craftlib::{
    scenario::test_sdk,
    sdk::{Helper, SpendableObject, SpendableObjects},
};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use txlib::{StateRoot, Tx};

use crate::{
    state::{CraftRuntime, CraftRuntimeInner, RuntimeObjectRecord, RuntimeValidity},
    types::{
        ClassMetaDto, RunSdkActionProgress, InventoryItemDto, ItemStatDto, LoadGuiBootstrapResult,
        MethodArgDto, ObjectMethodDto, RecipeDto, RunSdkActionInput, RunSdkActionResult,
        SourceActionMetaDto,
    },
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedStateRoot {
    transactions: serde_json::Value,
    nullifiers: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedSpendableObject {
    pod: serde_json::Value,
    obj: serde_json::Value,
    tx_live: serde_json::Value,
    tx_nullifiers: serde_json::Value,
    tx_state_root: PersistedStateRoot,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedObjectRecord {
    id: String,
    file_name: String,
    class_name: String,
    source_action: Option<String>,
    validity: String,
    state_hash: String,
    nullifier: Option<String>,
    stats: Vec<(String, String)>,
    spendable: Option<PersistedSpendableObject>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedRuntimeSnapshot {
    next_object_index: u64,
    state_root: PersistedStateRoot,
    objects: Vec<PersistedObjectRecord>,
}

fn short_hash(seed: &str) -> String {
    let mut bytes = [0u8; 8];
    for (idx, b) in seed.bytes().enumerate() {
        bytes[idx % 8] = bytes[idx % 8].wrapping_add(b);
    }
    format!(
        "0x{:02x}{:02x}...{:02x}{:02x}",
        bytes[0], bytes[1], bytes[6], bytes[7]
    )
}

fn resolve_objects_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let home = app
        .path()
        .home_dir()
        .map_err(|err| format!("failed to resolve home directory: {err}"))?;
    Ok(home.join(".objects"))
}

fn runtime_dir(objects_dir: &Path) -> PathBuf {
    objects_dir.join(".zkcraft")
}

fn runtime_snapshot_path(objects_dir: &Path) -> PathBuf {
    runtime_dir(objects_dir).join("runtime.json")
}

fn empty_state_root() -> StateRoot {
    let empty = HashSet::new();
    StateRoot::new(0, &empty, &empty, &[])
}

fn update_state_root(state_root: &mut StateRoot, tx: &Tx) {
    let tx_dict = tx.dict();
    let tx_value = tx_dict.into();
    state_root.transactions.insert(&tx_value).unwrap();
    for nullifier in tx.nullifiers.set() {
        state_root.transactions.insert(nullifier).unwrap();
    }
}

fn clone_spendable(spendable: &SpendableObject) -> SpendableObject {
    SpendableObject {
        pod: spendable.pod.clone(),
        obj: spendable.obj.clone(),
        tx: spendable.tx.clone(),
    }
}

fn normalize_component_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn format_output_file_name(class_name: &str, index: u64) -> String {
    format!("{}_{index}.dobj", normalize_component_name(class_name))
}

fn value_string(raw: String) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn stats_from_object(spendable: &SpendableObject) -> Vec<(String, String)> {
    let mut stats = Vec::new();
    for (key, value) in spendable.obj.kvs() {
        let name = key.name();
        if matches!(name, "key" | "work" | "blueprint") {
            continue;
        }
        stats.push((name.to_string(), value_string(format!("{value}"))));
    }
    stats.sort_by(|a, b| a.0.cmp(&b.0));
    stats
}

fn action_descriptors_by_name() -> HashMap<String, craftlib::scenario::test_sdk::ActionDescriptor> {
    let mut out = HashMap::new();
    for descriptor in test_sdk::action_descriptors() {
        out.insert(descriptor.name.clone(), descriptor);
    }
    out
}

fn build_action_catalog() -> Vec<RecipeDto> {
    test_sdk::action_descriptors()
        .into_iter()
        .filter(|descriptor| !descriptor.hidden)
        .map(|descriptor| RecipeDto {
            id: descriptor.name.clone(),
            group: String::new(),
            name: descriptor.name.clone(),
            emoji: descriptor.ui.emoji.to_string(),
            hash: short_hash(&descriptor.name),
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

fn to_inventory_item(record: &RuntimeObjectRecord) -> InventoryItemDto {
    let class_ui = test_sdk::class_ui_meta(&record.class_name);
    InventoryItemDto {
        id: record.id.clone(),
        file_name: record.file_name.clone(),
        emoji: class_ui.emoji.to_string(),
        validity: match record.validity {
            RuntimeValidity::Live => "live".to_string(),
            RuntimeValidity::Nullified => "nullified".to_string(),
        },
        state_root: record.state_hash.clone(),
        nullifier: record.nullifier.clone(),
        class_meta: ClassMetaDto {
            name: record.class_name.clone(),
            hash: short_hash(&record.class_name),
        },
        source_action: record.source_action.as_ref().map(|name| SourceActionMetaDto {
            name: name.clone(),
            hash: short_hash(name),
        }),
        description: Some(class_ui.description.to_string()),
        methods: Vec::<ObjectMethodDto>::new(),
        stats: record
            .stats
            .iter()
            .map(|(key, value)| ItemStatDto {
                key: key.clone(),
                value: value.clone(),
                tone: None,
            })
            .collect(),
    }
}

fn persist_state_root(state_root: &StateRoot) -> Result<PersistedStateRoot, String> {
    Ok(PersistedStateRoot {
        transactions: serde_json::to_value(&state_root.transactions)
            .map_err(|err| format!("failed to serialize state_root.transactions: {err}"))?,
        nullifiers: serde_json::to_value(&state_root.nullifiers)
            .map_err(|err| format!("failed to serialize state_root.nullifiers: {err}"))?,
    })
}

fn restore_state_root(data: PersistedStateRoot) -> Result<StateRoot, String> {
    Ok(StateRoot {
        transactions: serde_json::from_value(data.transactions)
            .map_err(|err| format!("failed to deserialize state_root.transactions: {err}"))?,
        nullifiers: serde_json::from_value(data.nullifiers)
            .map_err(|err| format!("failed to deserialize state_root.nullifiers: {err}"))?,
    })
}

fn persist_spendable(spendable: &SpendableObject) -> Result<PersistedSpendableObject, String> {
    Ok(PersistedSpendableObject {
        pod: serde_json::to_value(&spendable.pod)
            .map_err(|err| format!("failed to serialize spendable.pod: {err}"))?,
        obj: serde_json::to_value(&spendable.obj)
            .map_err(|err| format!("failed to serialize spendable.obj: {err}"))?,
        tx_live: serde_json::to_value(&spendable.tx.live)
            .map_err(|err| format!("failed to serialize spendable.tx.live: {err}"))?,
        tx_nullifiers: serde_json::to_value(&spendable.tx.nullifiers)
            .map_err(|err| format!("failed to serialize spendable.tx.nullifiers: {err}"))?,
        tx_state_root: persist_state_root(spendable.tx.state_root.as_ref())?,
    })
}

fn restore_spendable(data: PersistedSpendableObject) -> Result<SpendableObject, String> {
    let state_root = restore_state_root(data.tx_state_root)?;
    let tx = txlib::Tx {
        live: serde_json::from_value(data.tx_live)
            .map_err(|err| format!("failed to deserialize spendable.tx.live: {err}"))?,
        nullifiers: serde_json::from_value(data.tx_nullifiers)
            .map_err(|err| format!("failed to deserialize spendable.tx.nullifiers: {err}"))?,
        state_root: Arc::new(state_root),
    };
    Ok(SpendableObject {
        pod: serde_json::from_value(data.pod)
            .map_err(|err| format!("failed to deserialize spendable.pod: {err}"))?,
        obj: serde_json::from_value(data.obj)
            .map_err(|err| format!("failed to deserialize spendable.obj: {err}"))?,
        tx,
    })
}

fn persist_snapshot(inner: &CraftRuntimeInner, objects_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(runtime_dir(objects_dir))
        .map_err(|err| format!("failed to create runtime directory: {err}"))?;

    let snapshot = PersistedRuntimeSnapshot {
        next_object_index: inner.next_object_index,
        state_root: persist_state_root(&inner.state_root)?,
        objects: inner
            .objects
            .iter()
            .map(|record| {
                Ok(PersistedObjectRecord {
                    id: record.id.clone(),
                    file_name: record.file_name.clone(),
                    class_name: record.class_name.clone(),
                    source_action: record.source_action.clone(),
                    validity: match record.validity {
                        RuntimeValidity::Live => "live".to_string(),
                        RuntimeValidity::Nullified => "nullified".to_string(),
                    },
                    state_hash: record.state_hash.clone(),
                    nullifier: record.nullifier.clone(),
                    stats: record.stats.clone(),
                    spendable: record.spendable.as_ref().map(persist_spendable).transpose()?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    };

    let serialized = serde_json::to_string_pretty(&snapshot)
        .map_err(|err| format!("failed to serialize runtime snapshot: {err}"))?;
    fs::write(runtime_snapshot_path(objects_dir), serialized)
        .map_err(|err| format!("failed to write runtime snapshot: {err}"))
}

fn sync_object_files(inner: &CraftRuntimeInner, objects_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(objects_dir)
        .map_err(|err| format!("failed to create objects directory: {err}"))?;

    for record in &inner.objects {
        let mut data = serde_json::Map::new();
        for (key, value) in &record.stats {
            data.insert(key.clone(), serde_json::Value::String(value.clone()));
        }

        let content = serde_json::json!({
            "id": record.id,
            "class": record.class_name,
            "validity": match record.validity {
                RuntimeValidity::Live => "live",
                RuntimeValidity::Nullified => "nullified",
            },
            "stateRoot": record.state_hash,
            "nullifier": record.nullifier,
            "sourceAction": record.source_action,
            "data": data,
        });

        let path = objects_dir.join(&record.file_name);
        let serialized = serde_json::to_string_pretty(&content)
            .map_err(|err| format!("failed to serialize object file {}: {err}", record.file_name))?;
        fs::write(path, serialized)
            .map_err(|err| format!("failed to write object file {}: {err}", record.file_name))?;
    }

    Ok(())
}

fn load_snapshot_if_present(inner: &mut CraftRuntimeInner, objects_dir: &Path) -> Result<(), String> {
    let path = runtime_snapshot_path(objects_dir);
    if !path.exists() {
        return Ok(());
    }

    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read runtime snapshot: {err}"))?;
    let snapshot: PersistedRuntimeSnapshot = serde_json::from_str(&contents)
        .map_err(|err| format!("failed to parse runtime snapshot: {err}"))?;

    inner.next_object_index = snapshot.next_object_index;
    inner.state_root = restore_state_root(snapshot.state_root)?;
    inner.objects = snapshot
        .objects
        .into_iter()
        .map(|record| {
            Ok(RuntimeObjectRecord {
                id: record.id,
                file_name: record.file_name,
                class_name: record.class_name,
                source_action: record.source_action,
                validity: match record.validity.as_str() {
                    "live" => RuntimeValidity::Live,
                    "nullified" => RuntimeValidity::Nullified,
                    other => {
                        return Err(format!("invalid object validity in snapshot: {other}"));
                    }
                },
                state_hash: record.state_hash,
                nullifier: record.nullifier,
                stats: record.stats,
                spendable: record.spendable.map(restore_spendable).transpose()?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(())
}

fn ensure_runtime_loaded(inner: &mut CraftRuntimeInner, objects_dir: &Path) -> Result<(), String> {
    if inner.loaded {
        return Ok(());
    }
    fs::create_dir_all(objects_dir)
        .map_err(|err| format!("failed to create objects directory: {err}"))?;
    if let Err(err) = load_snapshot_if_present(inner, objects_dir) {
        eprintln!("zk-craft: failed to load runtime snapshot, resetting state: {err}");
        inner.next_object_index = 1;
        inner.state_root = empty_state_root();
        inner.objects.clear();
        let _ = persist_snapshot(inner, objects_dir);
    }
    inner.loaded = true;
    Ok(())
}

fn emit_progress(app: &tauri::AppHandle, payload: &RunSdkActionProgress) -> Result<(), String> {
    app.emit("run-sdk-action-progress", payload)
        .map_err(|err| format!("failed to emit run progress: {err}"))
}

fn clear_run_in_progress(runtime: &tauri::State<'_, CraftRuntime>) {
    if let Ok(mut inner) = runtime.inner.lock() {
        inner.run_in_progress = false;
    }
}

fn lock_runtime<'a>(
    runtime: &'a tauri::State<'_, CraftRuntime>,
) -> std::sync::MutexGuard<'a, CraftRuntimeInner> {
    match runtime.inner.lock() {
        Ok(inner) => inner,
        Err(poisoned) => {
            eprintln!("zk-craft: runtime lock poisoned, recovering state");
            poisoned.into_inner()
        }
    }
}

#[tauri::command]
pub async fn load_gui_bootstrap(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, CraftRuntime>,
) -> Result<LoadGuiBootstrapResult, String> {
    let objects_dir = resolve_objects_dir(&app)?;
    let actions = build_action_catalog();
    let mut inner = lock_runtime(&runtime);
    if let Err(err) = ensure_runtime_loaded(&mut inner, &objects_dir) {
        eprintln!("zk-craft: bootstrap runtime failed, resetting state: {err}");
        inner.next_object_index = 1;
        inner.state_root = empty_state_root();
        inner.objects.clear();
        inner.loaded = true;
        let _ = persist_snapshot(&inner, &objects_dir);
    }

    Ok(LoadGuiBootstrapResult {
        objects: inner.objects.iter().map(to_inventory_item).collect(),
        actions,
    })
}

fn execute_action(
    action_id: String,
    state_root: StateRoot,
    inputs: Vec<SpendableObject>,
) -> Result<SpendableObjects, String> {
    let helper = Helper::new(test_sdk::dependencies(), test_sdk::actions());
    let builder = helper.builder(true, Arc::new(state_root));
    Ok(builder.action(&action_id, inputs))
}

#[tauri::command]
pub async fn run_sdk_action(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, CraftRuntime>,
    input: RunSdkActionInput,
) -> Result<RunSdkActionResult, String> {
    let objects_dir = resolve_objects_dir(&app)?;
    let descriptors = action_descriptors_by_name();
    let descriptor = descriptors
        .get(&input.action_id)
        .ok_or_else(|| format!("unknown action: {}", input.action_id))?;
    if descriptor.hidden {
        return Err(format!("action is internal and cannot be run directly: {}", input.action_id));
    }

    if input.input_object_ids.len() != descriptor.input_classes.len() {
        return Err(format!(
            "{} expects {} inputs, got {}",
            input.action_id,
            descriptor.input_classes.len(),
            input.input_object_ids.len()
        ));
    }

    let mut seen = HashSet::new();
    for object_id in &input.input_object_ids {
        if !seen.insert(object_id) {
            return Err("duplicate input object IDs are not allowed".to_string());
        }
    }

    let (state_root_for_run, input_spendables, verify_targets, old_root);
    {
        let mut inner = lock_runtime(&runtime);
        ensure_runtime_loaded(&mut inner, &objects_dir)?;

        if inner.run_in_progress {
            return Err("another action run is already in progress".to_string());
        }

        let mut collected_spendables = Vec::new();
        let mut collected_targets = Vec::new();

        for (slot, object_id) in input.input_object_ids.iter().enumerate() {
            let expected_class = descriptor.input_classes[slot].as_str();
            let record = inner
                .objects
                .iter()
                .find(|record| &record.id == object_id)
                .ok_or_else(|| format!("input object not found: {object_id}"))?;

            if record.validity != RuntimeValidity::Live {
                return Err(format!("input object is not live: {object_id}"));
            }
            if record.class_name != expected_class {
                return Err(format!(
                    "input class mismatch for {}: expected {}, got {}",
                    object_id, expected_class, record.class_name
                ));
            }

            let spendable = record
                .spendable
                .as_ref()
                .ok_or_else(|| format!("input object missing spendable witness: {object_id}"))?;
            collected_spendables.push(clone_spendable(spendable));
            collected_targets.push(record.file_name.clone());
        }

        inner.run_in_progress = true;
        old_root = short_hash(&format!("{:#}", inner.state_root.hash()));
        state_root_for_run = inner.state_root.clone();
        input_spendables = collected_spendables;
        verify_targets = collected_targets;
    }

    let run_id = input.action_id.clone();
    if let Err(err) = emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
            phase: "hash".to_string(),
            status: "running".to_string(),
            message: format!("Running {}", input.action_id),
            verify_index: None,
            detail: Some(descriptor.ui.cpu_cost.to_string()),
            old_root: None,
            new_root: None,
            output_file: None,
        },
    ) {
        clear_run_in_progress(&runtime);
        return Err(err);
    }

    let action_id = input.action_id.clone();
    let execution = match tauri::async_runtime::spawn_blocking(move || {
        execute_action(action_id, state_root_for_run, input_spendables)
    })
    .await
    {
        Ok(value) => value,
        Err(err) => {
            clear_run_in_progress(&runtime);
            return Err(format!("failed while executing action: {err}"));
        }
    };

    let spendable_outputs = match execution {
        Ok(output) => output,
        Err(err) => {
            let mut inner = lock_runtime(&runtime);
            inner.run_in_progress = false;
            return Err(err);
        }
    };

    if let Err(err) = emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
            phase: "hash".to_string(),
            status: "done".to_string(),
            message: "Proof generation complete".to_string(),
            verify_index: None,
            detail: Some(descriptor.ui.cpu_cost.to_string()),
            old_root: None,
            new_root: None,
            output_file: None,
        },
    ) {
        clear_run_in_progress(&runtime);
        return Err(err);
    }

    if verify_targets.is_empty() {
        let placeholder = "(no inputs)".to_string();
        if let Err(err) = emit_progress(
            &app,
            &RunSdkActionProgress {
                run_id: run_id.clone(),
                phase: "verify".to_string(),
                status: "running".to_string(),
                message: format!("Verifying {placeholder}"),
                verify_index: Some(0),
                detail: Some(placeholder.clone()),
                old_root: None,
                new_root: None,
                output_file: None,
            },
        ) {
            clear_run_in_progress(&runtime);
            return Err(err);
        }
        if let Err(err) = emit_progress(
            &app,
            &RunSdkActionProgress {
                run_id: run_id.clone(),
                phase: "verify".to_string(),
                status: "done".to_string(),
                message: format!("Verified {placeholder}"),
                verify_index: Some(0),
                detail: Some(placeholder),
                old_root: None,
                new_root: None,
                output_file: None,
            },
        ) {
            clear_run_in_progress(&runtime);
            return Err(err);
        }
    } else {
        for (index, target) in verify_targets.iter().enumerate() {
            if let Err(err) = emit_progress(
                &app,
                &RunSdkActionProgress {
                    run_id: run_id.clone(),
                    phase: "verify".to_string(),
                    status: "running".to_string(),
                    message: format!("Verifying {target}"),
                    verify_index: Some(index),
                    detail: Some(target.clone()),
                    old_root: None,
                    new_root: None,
                    output_file: None,
                },
            ) {
                clear_run_in_progress(&runtime);
                return Err(err);
            }
            if let Err(err) = emit_progress(
                &app,
                &RunSdkActionProgress {
                    run_id: run_id.clone(),
                    phase: "verify".to_string(),
                    status: "done".to_string(),
                    message: format!("Verified {target}"),
                    verify_index: Some(index),
                    detail: Some(target.clone()),
                    old_root: None,
                    new_root: None,
                    output_file: None,
                },
            ) {
                clear_run_in_progress(&runtime);
                return Err(err);
            }
        }
    }

    if let Err(err) = emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
            phase: "nullify".to_string(),
            status: "running".to_string(),
            message: format!("Nullifying {old_root}"),
            verify_index: None,
            detail: Some(old_root.clone()),
            old_root: Some(old_root.clone()),
            new_root: None,
            output_file: None,
        },
    ) {
        clear_run_in_progress(&runtime);
        return Err(err);
    }

    let mut inner = lock_runtime(&runtime);

    let apply_result = (|| {
        ensure_runtime_loaded(&mut inner, &objects_dir)?;

        let mut nullified_files = Vec::new();
        for object_id in &input.input_object_ids {
            let record = inner
                .objects
                .iter_mut()
                .find(|record| &record.id == object_id)
                .ok_or_else(|| format!("input object not found while finalizing: {object_id}"))?;
            if record.validity != RuntimeValidity::Live {
                return Err(format!("input object already nullified: {object_id}"));
            }
            record.validity = RuntimeValidity::Nullified;
            record.nullifier = Some(short_hash(&format!("{}:{}:null", object_id, old_root)));
            record.spendable = None;
            nullified_files.push(record.file_name.clone());
        }

        if spendable_outputs.objs.len() != descriptor.output_classes.len() {
            return Err(format!(
                "action {} output mismatch: descriptor expects {}, engine returned {}",
                input.action_id,
                descriptor.output_classes.len(),
                spendable_outputs.objs.len()
            ));
        }

        let mut output_files = Vec::new();
        for (index, class_name) in descriptor.output_classes.iter().enumerate() {
            let spendable = spendable_outputs.obj(index);
            let object_index = inner.next_object_index;
            let file_name = format_output_file_name(class_name, object_index);
            inner.next_object_index += 1;

            output_files.push(file_name.clone());
            inner.objects.push(RuntimeObjectRecord {
                id: format!("obj-{object_index}"),
                file_name,
                class_name: class_name.clone(),
                source_action: Some(input.action_id.clone()),
                validity: RuntimeValidity::Live,
                state_hash: short_hash(&format!("{:#}", spendable.obj.commitment())),
                nullifier: None,
                stats: stats_from_object(&spendable),
                spendable: Some(spendable),
            });
        }

        update_state_root(&mut inner.state_root, &spendable_outputs.tx);
        let new_root = short_hash(&format!("{:#}", inner.state_root.hash()));

        persist_snapshot(&inner, &objects_dir)?;
        sync_object_files(&inner, &objects_dir)?;

        Ok(RunSdkActionResult {
            ok: true,
            old_root: old_root.clone(),
            new_root,
            output_files,
            nullified_files,
            objects: inner.objects.iter().map(to_inventory_item).collect(),
        })
    })();

    inner.run_in_progress = false;

    let result = apply_result?;

    emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
            phase: "nullify".to_string(),
            status: "done".to_string(),
            message: "Nullify complete".to_string(),
            verify_index: None,
            detail: Some(result.old_root.clone()),
            old_root: Some(result.old_root.clone()),
            new_root: None,
            output_file: None,
        },
    )?;

    emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
            phase: "commit".to_string(),
            status: "running".to_string(),
            message: format!("Committing {}", result.new_root),
            verify_index: None,
            detail: Some(result.new_root.clone()),
            old_root: Some(result.old_root.clone()),
            new_root: Some(result.new_root.clone()),
            output_file: result.output_files.first().cloned(),
        },
    )?;

    emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id,
            phase: "commit".to_string(),
            status: "done".to_string(),
            message: "Commit complete".to_string(),
            verify_index: None,
            detail: Some(result.new_root.clone()),
            old_root: Some(result.old_root.clone()),
            new_root: Some(result.new_root.clone()),
            output_file: result.output_files.first().cloned(),
        },
    )?;

    Ok(result)
}
