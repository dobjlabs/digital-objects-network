import type { ActionId, ClassName } from "../generated/contracts";

export type Validity = "live" | "nullified";

export type FieldValue = string | number | boolean | null;
export type StatTone = "neutral" | "good" | "warn" | "danger";
export type MethodArgKind = "class";

export interface ClassMeta {
  name: ClassName;
  hash: string;
}

export interface SourceActionMeta {
  name: ActionId;
  hash: string;
}

export interface MethodArg {
  kind: MethodArgKind;
  label: ClassName;
  classHash: string;
}

export interface ItemStat {
  key: string;
  value: FieldValue;
  tone?: StatTone;
  progressPercent?: number;
  progressTone?: Exclude<StatTone, "neutral">;
}

export interface ObjectMethod {
  methodName: string;
  cpuCost: string;
  readsBlock: boolean;
  args: MethodArg[];
}

export interface InventoryItem {
  id: string;
  fileName: string;
  emoji: string;
  validity: Validity;
  stateRoot: string;
  nullifier?: string;
  classMeta: ClassMeta;
  sourceAction?: SourceActionMeta;
  description?: string;
  methods: ObjectMethod[];
  stats: ItemStat[];
}

export interface Recipe {
  id: ActionId;
  group: string;
  name: ActionId;
  emoji: string;
  hash: string;
  verb: ActionId;
  desc: string;
  cpu: string;
  readsBlock: boolean;
  args: MethodArg[];
  unlocked: boolean;
}

export type ContextSelection =
  | { kind: "none" }
  | { kind: "item"; itemId: string }
  | { kind: "recipe"; recipeId: string };

export interface AppUiState {
  contextSelection: ContextSelection;
  activeItemId: string | null;
  activeRecipeId: string | null;
  showNullifiedItems: boolean;
}
