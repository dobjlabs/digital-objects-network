export type ProofPhase = "generateProof" | "commit";
export type ProofProgressStatus = "running" | "done";

export type ObjectStatus = "unknown" | "pending" | "live" | "nullified";

// Action and class names are not enumerated at compile time — they come
// from whichever plugin archives the user has installed in ~/.dobj/actions/.

/** A name scoped to a plugin. Both classes and actions are identified this
 * way; the printable form `<pluginName>::<name>` matches podlang's
 * namespaced-predicate syntax. */
export interface QualifiedNamePayload {
  pluginName: string;
  name: string;
}

export interface InventoryObjectPayload {
  id: string;
  fileName: string;
  class: QualifiedNamePayload;
  classHash: string;
  emoji: string;
  status: ObjectStatus;
  txHash: string | null;
  description?: string;
  /** Application-layer fields (e.g. `durability`, `key`, `work`). */
  fields: Record<string, unknown>;
}

export interface ClassRefPayload {
  class: QualifiedNamePayload;
  /** Hex-encoded `Is{class}` predicate hash. */
  hash: string;
}

export interface ActionPayload {
  action: QualifiedNamePayload;
  emoji: string;
  hash: string;
  totalInputs: ClassRefPayload[];
  description: string;
}

export interface RunActionInput {
  action: QualifiedNamePayload;
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
  class: QualifiedNamePayload;
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
