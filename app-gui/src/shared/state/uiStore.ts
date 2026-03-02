import { create } from "zustand";
import { initialUiState } from "./initialState";
import type { AppUiState } from "../types/domain";

interface UiStoreState extends AppUiState {
  selectItem: (itemId: string) => void;
  selectRecipe: (recipeId: string) => void;
  toggleNullified: () => void;
}

export const useUiStore = create<UiStoreState>((set) => ({
  ...initialUiState,
  selectItem: (itemId) =>
    set((prev) => ({
      ...prev,
      activeItemId: itemId,
      activeRecipeId: null,
      contextSelection: { kind: "item", itemId },
    })),
  selectRecipe: (recipeId) =>
    set((prev) => ({
      ...prev,
      activeItemId: null,
      activeRecipeId: recipeId,
      contextSelection: { kind: "recipe", recipeId },
    })),
  toggleNullified: () =>
    set((prev) => ({
      ...prev,
      showNullifiedItems: !prev.showNullifiedItems,
    })),
}));
