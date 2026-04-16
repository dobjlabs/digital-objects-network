import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppSettingsPayload,
  CpuSample,
  LoadGuiInventoryResult,
  ObjectRecordPayload,
  RunActionInput,
  RunActionProgress,
  RunActionResult,
} from "./wireTypes";

export type { ActionId, ClassName } from "../generated/ids";

export type {
  ActionPayload,
  AppSettingsPayload,
  CpuSample,
  InventoryObjectPayload,
  LoadGuiInventoryResult,
  ObjectRecordPayload,
  RunActionInput,
  RunActionProgress,
  RunActionResult,
} from "./wireTypes";

export function getObjectsDir(): Promise<string> {
  return invoke<string>("get_objects_dir");
}

export function openObjectsDir(): Promise<string> {
  return invoke<string>("open_objects_dir");
}

export function loadGuiInventory(): Promise<LoadGuiInventoryResult> {
  return invoke<LoadGuiInventoryResult>("load_gui_inventory");
}

export function runAction(input: RunActionInput): Promise<RunActionResult> {
  return invoke<RunActionResult>("run_action", { input });
}

export function pickDobjFilePath(): Promise<string> {
  return invoke<string>("pick_dobj_file_path");
}

export function readDobjFile(path: string): Promise<ObjectRecordPayload> {
  return invoke<ObjectRecordPayload>("read_dobj_file", { path });
}

export function listenRunActionProgress(
  handler: (event: RunActionProgress) => void,
): Promise<UnlistenFn> {
  return listen<RunActionProgress>("run-action-progress", (event) => {
    handler(event.payload);
  });
}

export function listenObjectsChanged(handler: () => void): Promise<UnlistenFn> {
  return listen("objects-changed", () => {
    handler();
  });
}

export function listenOpenSettings(handler: () => void): Promise<UnlistenFn> {
  return listen("open-settings", () => {
    handler();
  });
}

export function sampleAppCpu(): Promise<CpuSample> {
  return invoke<CpuSample>("sample_app_cpu");
}

export function getGlobalStateRoot(): Promise<string> {
  return invoke<string>("get_global_state_root");
}

export function listenMcpActionStarted(
  handler: (event: { actionId: string }) => void,
): Promise<UnlistenFn> {
  return listen<{ actionId: string }>("mcp-action-started", (event) => {
    handler(event.payload);
  });
}

export function getAppSettings(): Promise<AppSettingsPayload> {
  return invoke<AppSettingsPayload>("get_app_settings");
}

export function saveAppSettings(
  input: AppSettingsPayload,
): Promise<AppSettingsPayload> {
  return invoke<AppSettingsPayload>("save_app_settings", { input });
}
