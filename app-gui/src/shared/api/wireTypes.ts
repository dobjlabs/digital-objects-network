import type { ActionId, ClassName } from "../generated/ids";

export type ProofPhase = "generateProof" | "commit";
export type ProofProgressStatus = "running" | "done";

export type ObjectStatus = "unknown" | "pending" | "live" | "nullified";

export interface InventoryObjectPayload {
  id: string;
  fileName: string;
  className: ClassName;
  classHash: string;
  emoji: string;
  status: ObjectStatus;
  txHash: string | null;
  grounded: boolean;
  description?: string;
  obj: unknown;
}

export interface ActionPayload {
  id: ActionId;
  emoji: string;
  hash: string;
  inputClassHashes: string[];
  description: string;
  inputClasses: ClassName[];
}

export interface LoadGuiInventoryResult {
  inventory: InventoryObjectPayload[];
  actions: ActionPayload[];
}

export interface RunActionInput {
  actionId: ActionId;
  inputObjectPaths: string[];
}

export interface RunActionResult {
  ok: boolean;
  oldRoot: string;
  newRoot: string;
  outputFiles: string[];
  nullifiedFiles: string[];
}

export interface ObjectRecordPayload {
  id: string;
  className: ClassName;
  status: ObjectStatus;
  txHash: string | null;
  pod: unknown;
  obj: unknown;
  tx: unknown;
}

export interface RunActionProgress {
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
