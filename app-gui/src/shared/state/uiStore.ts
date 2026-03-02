import { create } from "zustand";
import { runMethod } from "../api/tauriClient";
import { initialUiState } from "./initialState";
import type { AppUiState } from "../types/domain";

type ProofStatus = "idle" | "generating" | "committing" | "done" | "error";

interface ProofState {
  status: ProofStatus;
  methodName: string | null;
  cpuCost: string | null;
  args: string[];
  messages: string[];
  oldRoot: string | null;
  newRoot: string | null;
  error: string | null;
}

interface UiStoreState extends AppUiState {
  dragItemName: string | null;
  proof: ProofState;
  selectItem: (itemId: string) => void;
  selectRecipe: (recipeId: string) => void;
  toggleNullified: () => void;
  beginDragItem: (itemName: string) => void;
  endDragItem: () => void;
  runProof: (input: { methodName: string; args: string[]; cpuCost: string }) => Promise<void>;
}

export const useUiStore = create<UiStoreState>((set) => ({
  ...initialUiState,
  dragItemName: null,
  proof: {
    status: "idle",
    methodName: null,
    cpuCost: null,
    args: [],
    messages: [],
    oldRoot: null,
    newRoot: null,
    error: null,
  },
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
  beginDragItem: (itemName) =>
    set((prev) => ({
      ...prev,
      dragItemName: itemName,
    })),
  endDragItem: () =>
    set((prev) => ({
      ...prev,
      dragItemName: null,
    })),
  runProof: async ({ methodName, args, cpuCost }) => {
    set((prev) => {
      if (prev.proof.status === "generating" || prev.proof.status === "committing") return prev;
      return {
        ...prev,
        proof: {
          status: "generating",
          methodName,
          cpuCost,
          args,
          messages: ["Generating recursive proof..."],
          oldRoot: null,
          newRoot: null,
          error: null,
        },
      };
    });

    try {
      const result = await runMethod({ methodName, args, cpuCost });
      set((prev) => ({
        ...prev,
        proof: {
          status: "committing",
          methodName: result.methodName,
          cpuCost,
          args,
          messages: [
            ...result.stageMessages,
            `Nullifying old root ${result.oldRoot}`,
            `Committing new root ${result.newRoot}`,
          ],
          oldRoot: result.oldRoot,
          newRoot: result.newRoot,
          error: null,
        },
      }));

      await new Promise((resolve) => setTimeout(resolve, 700));

      set((prev) => ({
        ...prev,
        proof: {
          ...prev.proof,
          status: "done",
        },
      }));
    } catch (error) {
      set((prev) => ({
        ...prev,
        proof: {
          status: "error",
          methodName,
          cpuCost,
          args,
          messages: [],
          oldRoot: null,
          newRoot: null,
          error: error instanceof Error ? error.message : "Failed to run proof",
        },
      }));
    }
  },
}));
