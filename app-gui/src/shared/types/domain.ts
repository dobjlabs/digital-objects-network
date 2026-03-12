import type { ActionId, ClassName } from "../generated/ids";

export interface InventoryObject {
  id: string;
  fileName: string;
  className: ClassName;
  emoji: string;
  nullifier: string | null;
  description?: string;
  obj: Record<string, unknown>;
}

export interface Action {
  id: ActionId;
  emoji: string;
  description: string;
  cpuCost: string;
  readsBlock: boolean;
  inputClasses: ClassName[];
}

export type ContextSelection =
  | { kind: "none" }
  | { kind: "object"; objectId: string }
  | { kind: "action"; actionId: string };

export interface AppUiState {
  contextSelection: ContextSelection;
  activeObjectId: string | null;
  activeActionId: string | null;
  showNullifiedItems: boolean;
}
