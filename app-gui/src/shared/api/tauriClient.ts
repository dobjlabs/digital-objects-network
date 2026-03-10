import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface MethodArgPayload {
  kind: string;
  label: string;
  classHash: string;
}

export interface ObjectMethodPayload {
  methodName: string;
  cpuCost: string;
  readsBlock: boolean;
  args: MethodArgPayload[];
}

export interface InventoryItemPayload {
  id: string;
  fileName: string;
  emoji: string;
  validity: "live" | "nullified";
  stateRoot: string;
  nullifier?: string;
  classMeta: {
    name: string;
    hash: string;
  };
  sourceAction?: {
    name: string;
    hash: string;
  };
  description?: string;
  methods: ObjectMethodPayload[];
  stats: Array<{
    key: string;
    value: string;
    tone?: string;
  }>;
}

export interface RecipePayload {
  id: string;
  group: string;
  name: string;
  emoji: string;
  hash: string;
  verb: string;
  desc: string;
  cpu: string;
  readsBlock: boolean;
  args: MethodArgPayload[];
  unlocked: boolean;
}

export interface LoadGuiBootstrapResult {
  objects: InventoryItemPayload[];
  actions: RecipePayload[];
}

export interface RunSdkActionInput {
  actionId: string;
  inputs: Array<{
    objectPath: string;
    label?: string;
  }>;
}

export interface RunSdkActionResult {
  ok: boolean;
  oldRoot: string;
  newRoot: string;
  outputFiles: string[];
  nullifiedFiles: string[];
  objects: InventoryItemPayload[];
}

export interface DobjFileMetadata {
  fileName: string;
  className: string;
  validity: string;
}

export interface RunSdkActionProgress {
  runId: string;
  phase: "hash" | "verify" | "nullify" | "commit";
  status: "running" | "done";
  message: string;
  verifyIndex: number | null;
  detail: string | null;
  oldRoot: string | null;
  newRoot: string | null;
  outputFile: string | null;
}

export interface CpuSample {
  usagePct: number;
  totalCpuSecs: number;
}

export function getThingsDir(): Promise<string> {
  return invoke<string>("get_things_dir");
}

export function openThingsDir(): Promise<string> {
  return invoke<string>("open_things_dir");
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

export function readDobjFileMetadata(path: string): Promise<DobjFileMetadata> {
  return invoke<DobjFileMetadata>("read_dobj_file_metadata", { path });
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

export function sampleAppCpu(): Promise<CpuSample> {
  return invoke<CpuSample>("sample_app_cpu");
}
