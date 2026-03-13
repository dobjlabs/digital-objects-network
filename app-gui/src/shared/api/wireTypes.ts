import type { ActionId, ClassName } from "../generated/ids";

export type ProofPhase = "generateProof" | "commit";
export type ProofProgressStatus = "running" | "done";

export interface InventoryObjectPayload {
  id: string;
  fileName: string;
  className: ClassName;
  classHash: string;
  emoji: string;
  nullifier: string | null;
  description?: string;
  obj: Record<string, unknown>;
}

export interface ActionPayload {
  id: ActionId;
  emoji: string;
  hash: string;
  inputClassHashes: string[];
  description: string;
  cpuCost: string;
  readsBlock: boolean;
  inputClasses: ClassName[];
}

export interface LoadGuiInventoryResult {
  inventory: InventoryObjectPayload[];
  actions: ActionPayload[];
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
  outputFiles: string[] | null;
}

export interface CpuSample {
  usagePct: number;
  totalCpuSecs: number;
}

export interface AppSettingsPayload {
  synchronizerApiUrl: string;
  relayerApiUrl: string;
}
