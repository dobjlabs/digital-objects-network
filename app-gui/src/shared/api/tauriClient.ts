import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppSettingsPayload,
  CpuSample,
  LoadGuiBootstrapResult,
  ObjectFileMetadata,
  RunSdkActionInput,
  RunSdkActionProgress,
  RunSdkActionResult,
} from "./wireTypes";

export type { ActionId, ClassName } from "../generated/ids";

export type {
  AppSettingsPayload,
  CpuSample,
  InventoryItemPayload,
  LoadGuiBootstrapResult,
  ObjectFileMetadata,
  RecipePayload,
  RunSdkActionArgInput,
  RunSdkActionInput,
  RunSdkActionProgress,
  RunSdkActionResult,
} from "./wireTypes";

export function getObjectsDir(): Promise<string> {
  return invoke<string>("get_objects_dir");
}

export function openObjectsDir(): Promise<string> {
  return invoke<string>("open_objects_dir");
}

export function loadGuiBootstrap(): Promise<LoadGuiBootstrapResult> {
  return invoke<LoadGuiBootstrapResult>("load_gui_bootstrap");
}

export function runSdkAction(
  input: RunSdkActionInput,
): Promise<RunSdkActionResult> {
  return invoke<RunSdkActionResult>("run_sdk_action", { input });
}

export function pickDobjFilePath(): Promise<string> {
  return invoke<string>("pick_dobj_file_path");
}

export function readDobjFileMetadata(path: string): Promise<ObjectFileMetadata> {
  return invoke<ObjectFileMetadata>("read_dobj_file_metadata", { path });
}

export function listenRunSdkActionProgress(
  handler: (event: RunSdkActionProgress) => void,
): Promise<UnlistenFn> {
  return listen<RunSdkActionProgress>("run-sdk-action-progress", (event) => {
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

export function getAppSettings(): Promise<AppSettingsPayload> {
  return invoke<AppSettingsPayload>("get_app_settings");
}

export function saveAppSettings(
  input: AppSettingsPayload,
): Promise<AppSettingsPayload> {
  return invoke<AppSettingsPayload>("save_app_settings", { input });
}
