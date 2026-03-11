import type { ActionId, ClassName } from "../generated/ids";

export type MethodArgKind = "class";
export type ProofPhase = "generateProof" | "commit";
export type ProofProgressStatus = "running" | "done";

export interface MethodArgPayload {
  kind: MethodArgKind;
  label: ClassName;
  classHash: string;
}

export interface ClassMetaPayload {
  name: ClassName;
  hash: string;
}

export interface SourceActionMetaPayload {
  name: ActionId;
  hash: string;
}

export interface ObjectDataEntryPayload {
  key: string;
  value: string;
}

export interface InventoryItemPayload {
  id: string;
  fileName: string;
  emoji: string;
  nullifier?: string;
  classMeta: ClassMetaPayload;
  sourceAction: SourceActionMetaPayload;
  description?: string;
  obj: ObjectDataEntryPayload[];
}

export interface RecipePayload {
  id: ActionId;
  group: string;
  name: ActionId;
  emoji: string;
  hash: string;
  verb: ActionId;
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
  actionId: ActionId;
  inputObjectPaths: string[];
}

export interface RunSdkActionResult {
  ok: boolean;
  oldRoot: string;
  newRoot: string;
  outputFiles: string[];
  nullifiedFiles: string[];
}

export interface ObjectRecordPayload {
  id: string;
  className: ClassName;
  sourceAction: ActionId;
  nullifier: string | null;
  pod: unknown;
  obj: unknown;
  tx: unknown;
}

export interface RunSdkActionProgress {
  runId: string;
  phase: ProofPhase;
  status: ProofProgressStatus;
  message: string;
  oldRoot: string | null;
  newRoot: string | null;
  outputFile: string | null;
}

export interface CpuSample {
  usagePct: number;
  totalCpuSecs: number;
}

export interface AppSettingsPayload {
  synchronizerApiUrl: string;
  relayerApiUrl: string;
}
