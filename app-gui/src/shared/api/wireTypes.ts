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
  grounded: boolean;
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

export interface LoadGuiInventoryResult {
  inventory: InventoryObjectPayload[];
  actions: ActionPayload[];
}

export interface RunActionInput {
  actionId: string;
  inputObjectPaths: string[];
  // Client-generated correlation id. The daemon echoes it back in
  // run-action-progress events so we can filter to our own run when more
  // than one is in flight (e.g. the user kicks off a craft while an MCP
  // agent is mid-action in the background).
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
}

export interface CpuSample {
  usagePct: number;
  totalCpuSecs: number;
}

export interface AppSettingsPayload {
  synchronizerApiUrl: string;
  relayerApiUrl: string;
}
