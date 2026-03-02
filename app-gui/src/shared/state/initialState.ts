import type { AppUiState } from "../types/domain";

export const initialUiState: AppUiState = {
  contextSelection: { kind: "none" },
  activeItemId: null,
  activeRecipeId: null,
  showNullifiedItems: false,
};
