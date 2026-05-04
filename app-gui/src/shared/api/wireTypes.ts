export type ProofPhase = "generateProof" | "commit";
export type ProofProgressStatus = "running" | "done";

export type ObjectStatus = "unknown" | "pending" | "live" | "nullified";

// Action IDs and class names are not enumerated at compile time — they come
// from whichever plugin archives the user has installed in ~/.dobj/actions/.

export interface InventoryObjectPayload {
  id: string;
  fileName: string;
  /** Qualified class id (`<plugin>:<class>`). */
  classId: string;
  /** Bare class name from the plugin manifest. */
  classDisplayName: string;
  pluginName: string;
  classHash: string;
  emoji: string;
  status: ObjectStatus;
  txHash: string | null;
  grounded: boolean;
  description?: string;
  obj: unknown;
}

export interface ClassRefPayload {
  /** Qualified class id (`<plugin>:<class>`). */
  id: string;
  /** Bare class name from the producing plugin's manifest. */
  displayName: string;
  /** Hex-encoded `Is{class}` predicate hash. */
  hash: string;
}

export interface ActionPayload {
  /** Qualified action id (`<plugin>:<action>`). */
  id: string;
  /** Bare action name from the plugin manifest. */
  displayName: string;
  pluginName: string;
  emoji: string;
  hash: string;
  totalInputs: ClassRefPayload[];
  description: string;
}

export interface LoadGuiInventoryResult {
  inventory: InventoryObjectPayload[];
  actions: ActionPayload[];
}

export interface RunActionInput {
  actionId: string;
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
  /** Qualified class id (`<plugin>:<class>`). */
  classId: string;
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
