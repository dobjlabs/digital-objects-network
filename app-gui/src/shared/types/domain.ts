export type Validity = "live" | "nullified";

export type ItemType =
  | "source"
  | "raw"
  | "material"
  | "tool"
  | "station"
  | "worker"
  | "creature"
  | "vehicle"
  | "coin"
  | "bond"
  | "document"
  | "rare";

export interface InventoryItem {
  id: string;
  name: string;
  emoji: string;
  type: ItemType;
  validity: Validity;
  stateRoot: string;
  nullifier?: string;
}

export interface RecipeRequirement {
  label: string;
}

export interface Recipe {
  id: string;
  name: string;
  emoji: string;
  verb: string;
  desc: string;
  cpu: string;
  readsBlock: boolean;
  consumes: RecipeRequirement[];
  requires: RecipeRequirement[];
  unlocked: boolean;
}

export interface ProofClaim {
  name: string;
  validity: Validity;
  hash: string;
}

export interface FeedResponse {
  id: string;
  peer: string;
  time: string;
  desc: string;
  proofs: ProofClaim[];
}

export interface FeedPost {
  id: string;
  title: string;
  peer: string;
  time: string;
  desc: string;
  proofs: ProofClaim[];
  responses: FeedResponse[];
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
