export type Validity = "live" | "nullified";

export type FieldValue = string | number | boolean | null;

export interface ObjectMethod {
  methodName: string;
  cpuCost: string;
  readsBlock: boolean;
  args: string[];
}

export interface InventoryItem {
  id: string;
  name: string;
  emoji: string;
  className: string;
  validity: Validity;
  stateRoot: string;
  nullifier?: string;
  methods: ObjectMethod[];
  fields: Record<string, FieldValue>;
}

export interface RecipeRequirement {
  label: string;
}

export interface Recipe {
  id: string;
  name: string;
  emoji: string;
  className: string;
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
