import type { ActionId, ClassName } from "../generated/ids";

export type Validity = "live" | "nullified";
export type MethodArgKind = "class";
export type ProofPhase = "generateProof" | "commit";
export type ProofProgressStatus = "running" | "done";

export interface MethodArgPayload {
  kind: MethodArgKind;
  label: ClassName;
  classHash: string;
}

export interface ObjectMethodPayload {
  methodName: string;
  cpuCost: string;
  readsBlock: boolean;
  args: MethodArgPayload[];
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
  validity: Validity;
  stateRoot: string;
  nullifier?: string;
  classMeta: ClassMetaPayload;
  sourceAction?: SourceActionMetaPayload;
  description?: string;
  methods: ObjectMethodPayload[];
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

export interface ObjectFileMetadata {
  fileName: string;
  className: ClassName;
  validity: Validity;
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
