import type { AppUiState } from "../types/domain";

export const initialUiState: AppUiState = {
  contextSelection: { kind: "none" },
  activeObjectId: null,
  activeActionId: null,
  showNullifiedItems: false,
};
