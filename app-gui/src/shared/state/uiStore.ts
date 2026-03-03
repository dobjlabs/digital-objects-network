import { create } from "zustand";
import { runMethod } from "../api/tauriClient";
import { initialUiState } from "./initialState";
import type { AppUiState } from "../types/domain";

type ProofStatus = "idle" | "generating" | "committing" | "done" | "error";
type StepStatus = "pending" | "running" | "done";

interface ProofStep {
  id: string;
  label: string;
  detail: string;
  status: StepStatus;
}

interface ProofState {
  status: ProofStatus;
  methodName: string | null;
  cpuCost: string | null;
  args: string[];
  messages: string[];
  steps: ProofStep[];
  oldRoot: string | null;
  newRoot: string | null;
  error: string | null;
}

interface UiStoreState extends AppUiState {
  proof: ProofState;
  selectItem: (itemId: string) => void;
  selectRecipe: (recipeId: string) => void;
  toggleNullified: () => void;
  runProof: (input: { methodName: string; args: string[]; cpuCost: string }) => Promise<void>;
}

export const useUiStore = create<UiStoreState>((set) => ({
  ...initialUiState,
  proof: {
    status: "idle",
    methodName: null,
    cpuCost: null,
    args: [],
    messages: [],
    steps: [],
    oldRoot: null,
    newRoot: null,
    error: null,
  },
  selectItem: (itemId) =>
    set((prev) => {
      if (
        prev.activeItemId === itemId &&
        prev.activeRecipeId === null &&
        prev.contextSelection.kind === "item" &&
        prev.contextSelection.itemId === itemId
      ) {
        return prev;
      }
      return {
        ...prev,
        activeItemId: itemId,
        activeRecipeId: null,
        contextSelection: { kind: "item", itemId },
      };
    }),
  selectRecipe: (recipeId) =>
    set((prev) => {
      if (
        prev.activeRecipeId === recipeId &&
        prev.activeItemId === null &&
        prev.contextSelection.kind === "recipe" &&
        prev.contextSelection.recipeId === recipeId
      ) {
        return prev;
      }
      return {
        ...prev,
        activeItemId: null,
        activeRecipeId: recipeId,
        contextSelection: { kind: "recipe", recipeId },
      };
    }),
  toggleNullified: () =>
    set((prev) => ({
      ...prev,
      showNullifiedItems: !prev.showNullifiedItems,
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
          steps: [
            { id: "hash", label: "Hashing", detail: cpuCost, status: "running" },
            ...args.map((arg, i) => ({
              id: `verify-${i}`,
              label: "Verifying Input",
              detail: arg,
              status: "pending" as StepStatus,
            })),
            { id: "nullify", label: "Nullifying Root", detail: "pending", status: "pending" },
            { id: "commit", label: "Committing New Root", detail: "pending", status: "pending" },
          ],
          oldRoot: null,
          newRoot: null,
          error: null,
        },
      };
    });

    try {
      for (const [index, arg] of args.entries()) {
        await new Promise((resolve) => setTimeout(resolve, 180));
        set((prev) => ({
          ...prev,
          proof: {
            ...prev.proof,
            steps: prev.proof.steps.map((step) => {
              if (step.id === `verify-${index}`) {
                return { ...step, detail: arg, status: "done" };
              }
              return step;
            }),
          },
        }));
      }

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
          steps: prev.proof.steps.map((step) => {
            if (step.id === "hash") return { ...step, status: "done" };
            if (step.id === "nullify") return { ...step, detail: result.oldRoot, status: "running" };
            if (step.id === "commit") return { ...step, detail: result.newRoot, status: "pending" };
            return step;
          }),
          oldRoot: result.oldRoot,
          newRoot: result.newRoot,
          error: null,
        },
      }));

      await new Promise((resolve) => setTimeout(resolve, 350));
      set((prev) => ({
        ...prev,
        proof: {
          ...prev.proof,
          steps: prev.proof.steps.map((step) =>
            step.id === "nullify" ? { ...step, status: "done" } : step,
          ),
        },
      }));

      await new Promise((resolve) => setTimeout(resolve, 350));
      set((prev) => ({
        ...prev,
        proof: {
          ...prev.proof,
          steps: prev.proof.steps.map((step) =>
            step.id === "commit" ? { ...step, status: "done" } : step,
          ),
        },
      }));

      await new Promise((resolve) => setTimeout(resolve, 250));

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
          steps: [],
          oldRoot: null,
          newRoot: null,
          error: error instanceof Error ? error.message : "Failed to run proof",
        },
      }));
    }
  },
}));
