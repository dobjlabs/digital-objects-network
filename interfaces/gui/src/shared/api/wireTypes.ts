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

/** The `ObjectSummary` wire shape returned by `/objects`, `/objects/{name}`,
 * and `/objects/import`. The driver folds in the object's class emoji and
 * description so clients can render rows without a second `/classes` call. */
export interface ObjectSummaryPayload {
  contentHash: string;
  fileName: string;
  fileSize?: number | null;
  class: QualifiedNamePayload;
  classHash: string;
  emoji: string;
  status: ObjectStatus;
  txHash: string | null;
  description?: string;
  /** Application-layer fields (e.g. `durability`, `key`). */
  fields: Record<string, unknown>;
  /** The run holding this object as an input while that run is in flight. */
  heldByRunId?: string | null;
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

/** A prover-supplied argument an action accepts (an `arg` declaration in
 * its script). A value may be supplied with the run; otherwise the script's
 * default computes one. */
export interface ActionArgPayload {
  /** Argument name declared by the action. */
  name: string;
  /** pod2 value type a supplied value must have: `Raw`, `Int`, ... */
  type: string;
  /** Script method that computes the value when none is supplied. */
  default: string;
}

export interface ActionPayload {
  action: QualifiedNamePayload;
  emoji: string;
  hash: string;
  totalInputs: ClassRefPayload[];
  description: string;
  args?: ActionArgPayload[];
}

/** Argument values by name in pod2's JSON form: `{ Raw: "<64 hex>" }`,
 * `{ Int: "<decimal>" }` or a bare string. */
export type ActionArgValues = Record<string, unknown>;

export interface RunActionInput {
  action: QualifiedNamePayload;
  inputObjectPaths: string[];
  /** Values for some declared arguments; the rest take the script default. */
  args?: ActionArgValues;
}

export interface RunActionResult {
  runId: string;
  oldRoot: string;
  newRoot: string;
  outputFiles: string[];
  nullifiedFiles: string[];
  /** The selected action's argument values, supplied or computed. */
  args?: ActionArgValues;
}

/** `POST /actions/run` response: the run was accepted and is executing in the
 * background. Follow it via `getRun` (poll) or the run's SSE stream. */
export interface RunAccepted {
  runId: string;
  status: RunStatus;
  createdAtMs?: number;
}

/** `GET /actions/runs/{runId}` response: current state of a run. */
export interface RunState {
  runId: string;
  action: QualifiedNamePayload;
  status: RunStatus;
  inputObjectPaths?: string[];
  createdAtMs?: number;
  startedAtMs?: number | null;
  finishedAtMs?: number | null;
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
  atMs?: number;
}

export interface AppSettingsPayload {
  synchronizerApiUrl: string;
  relayerApiUrl: string;
  /** Whether dobjd serves MCP on the adjacent port (default off). */
  mcpEnabled: boolean;
}
