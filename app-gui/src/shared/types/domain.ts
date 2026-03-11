import type { ActionId, ClassName } from "../generated/ids";

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

export interface InventoryItem {
  id: string;
  fileName: string;
  emoji: string;
  nullifier?: string;
  classMeta: ClassMeta;
  sourceAction: SourceActionMeta;
  description?: string;
  obj: Record<string, unknown>;
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
