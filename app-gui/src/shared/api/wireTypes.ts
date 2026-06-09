export type ProofPhase = "generateProof" | "commit";
export type ProofProgressStatus = "running" | "done" | "failed";

/** Lifecycle state of a run in the daemon's run registry. Mirrors
 * `wire_types::RunStatus` (camelCase). */
export type RunStatus =
  | "queued"
  | "generateProof"
  | "committing"
  | "succeeded"
  | "failed";

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

export interface ObjectListingPayload {
  contentHash: string;
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

/** The `ObjectSummary` wire shape returned by `/objects/{name}` and
 * `/objects/import` — like `ObjectListingPayload` but without the
 * folded-in class emoji/description. */
export interface ObjectSummaryPayload {
  contentHash: string;
  fileName: string;
  class: QualifiedNamePayload;
  classHash: string;
  status: ObjectStatus;
  txHash: string | null;
  fields: Record<string, unknown>;
}

/** `POST /objects/import` request body — the raw JSON contents of an external
 * `.dobj` file, one not produced by this driver (e.g. from outside `~/.dobj/`). */
export interface ImportObjectRequest {
  dobj: string;
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
}

export interface RunActionResult {
  runId: string;
  oldRoot: string;
  newRoot: string;
  outputFiles: string[];
  nullifiedFiles: string[];
}

/** `POST /actions/run` response: the run was accepted and is executing in the
 * background. Follow it via `getRun` (poll) or the run's SSE stream. */
export interface RunAccepted {
  runId: string;
  status: RunStatus;
}

/** `GET /actions/runs/{runId}` response: current state of a run. */
export interface RunState {
  runId: string;
  action: QualifiedNamePayload;
  status: RunStatus;
  result: RunActionResult | null;
  error: string | null;
  progress: RunActionProgress[];
}

export interface ObjectRecordPayload {
  contentHash: string;
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
