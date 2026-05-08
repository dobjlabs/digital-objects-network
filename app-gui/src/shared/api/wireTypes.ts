export type ProofPhase = "generateProof" | "commit";
export type ProofProgressStatus = "running" | "done";

export type ObjectStatus = "unknown" | "pending" | "live" | "nullified";

// Action IDs and class names are not enumerated at compile time — they come
// from whichever plugin archives the user has installed in ~/.dobj/actions/.

export interface InventoryObjectPayload {
  id: string;
  fileName: string;
  className: string;
  classHash: string;
  emoji: string;
  status: ObjectStatus;
  txHash: string | null;
  description?: string;
  obj: unknown;
}

export interface ActionPayload {
  id: string;
  emoji: string;
  hash: string;
  totalInputClassHashes: string[];
  description: string;
  totalInputClasses: string[];
}

export interface RunActionInput {
  actionId: string;
  inputObjectPaths: string[];
  runId: string;
}

export interface RunActionResult {
  runId: string;
  oldRoot: string;
  newRoot: string;
  outputFiles: string[];
  nullifiedFiles: string[];
}

export interface ObjectRecordPayload {
  id: string;
  className: string;
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
  outputStatus: ObjectStatus | null;
  nullifiedFiles: string[] | null;
}

export interface CpuSample {
  usagePct: number;
  totalCpuSecs: number;
}

export interface AppSettingsPayload {
  synchronizerApiUrl: string;
  relayerApiUrl: string;
}
