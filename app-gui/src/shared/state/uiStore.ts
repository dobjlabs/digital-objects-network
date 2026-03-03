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

interface RootSnapshot {
  hash: string;
  state: "live" | "nullified";
}

interface ProofStats {
  cpuHistory: number[];
  totalCpuSecs: number;
  roots: RootSnapshot[];
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
  stats: ProofStats;
}

interface UiStoreState extends AppUiState {
  proof: ProofState;
  selectItem: (itemId: string) => void;
  selectRecipe: (recipeId: string) => void;
  toggleNullified: () => void;
  runProof: (input: {
    methodName: string;
    args: string[];
    cpuCost: string;
  }) => Promise<void>;
}

function estimateCpuSecs(cpuCost: string): number {
  const match = cpuCost.match(/\d+/);
  const n = match ? Number(match[0]) : 1;
  if (cpuCost.includes("h")) return n * 3600;
  if (cpuCost.includes("m")) return n * 60;
  return n;
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
    stats: {
      cpuHistory: [4, 8, 12, 8, 22, 18, 38, 42, 60, 55, 48, 52, 65, 58, 70, 62],
      totalCpuSecs: 47 * 60 + 38,
      roots: [
        { hash: "0x1b89...cc41", state: "live" },
        { hash: "0x7c44...a203", state: "live" },
        { hash: "0x9cd4...e223", state: "live" },
        { hash: "0x9d01...f334", state: "nullified" },
        { hash: "0x2e55...7710", state: "nullified" },
      ],
    },
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
    const hashStepDelayMs = 650;
    const verifyStepDelayMs = 650;
    const commitTransitionDelayMs = 900;
    const postDoneHoldMs = 2600;

    set((prev) => {
      if (
        prev.proof.status === "generating" ||
        prev.proof.status === "committing"
      )
        return prev;
      return {
        ...prev,
        proof: {
          status: "generating",
          methodName,
          cpuCost,
          args,
          messages: ["Generating recursive proof..."],
          steps: [
            {
              id: "hash",
              label: "Hashing",
              detail: cpuCost,
              status: "running",
            },
            ...args.map((arg, i) => ({
              id: `verify-${i}`,
              label: "Verifying Input",
              detail: arg,
              status: "pending" as StepStatus,
            })),
            {
              id: "nullify",
              label: "Nullifying Root",
              detail: "pending",
              status: "pending",
            },
            {
              id: "commit",
              label: "Committing New Root",
              detail: "pending",
              status: "pending",
            },
          ],
          oldRoot: null,
          newRoot: null,
          error: null,
          stats: prev.proof.stats,
        },
      };
    });

      try {
      await new Promise((resolve) => setTimeout(resolve, hashStepDelayMs));
      set((prev) => ({
        ...prev,
        proof: {
          ...prev.proof,
          steps: prev.proof.steps.map((step) =>
            step.id === "hash" ? { ...step, status: "done" } : step,
          ),
        },
      }));

      for (const [index, arg] of args.entries()) {
        set((prev) => ({
          ...prev,
          proof: {
            ...prev.proof,
            steps: prev.proof.steps.map((step) =>
              step.id === `verify-${index}`
                ? { ...step, detail: arg, status: "running" }
                : step,
            ),
          },
        }));
        await new Promise((resolve) => setTimeout(resolve, verifyStepDelayMs));
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
            if (step.id === "nullify")
              return { ...step, detail: result.oldRoot, status: "running" };
            if (step.id === "commit")
              return { ...step, detail: result.newRoot, status: "pending" };
            return step;
          }),
          oldRoot: result.oldRoot,
          newRoot: result.newRoot,
          error: null,
          stats: prev.proof.stats,
        },
      }));

      await new Promise((resolve) =>
        setTimeout(resolve, commitTransitionDelayMs),
      );
      set((prev) => ({
        ...prev,
        proof: {
          ...prev.proof,
          steps: prev.proof.steps.map((step) => {
            if (step.id === "nullify") return { ...step, status: "done" };
            if (step.id === "commit") return { ...step, status: "running" };
            return step;
          }),
        },
      }));

      await new Promise((resolve) =>
        setTimeout(resolve, commitTransitionDelayMs),
      );
      set((prev) => ({
        ...prev,
        proof: {
          ...prev.proof,
          steps: prev.proof.steps.map((step) =>
            step.id === "commit" ? { ...step, status: "done" } : step,
          ),
        },
      }));

      await new Promise((resolve) => setTimeout(resolve, 600));

      set((prev) => {
        const nextCpu = Math.max(
          2,
          Math.min(100, Math.round(Math.random() * 40 + 30)),
        );
        return {
          ...prev,
          proof: {
            ...prev.proof,
            status: "done",
            stats: {
              cpuHistory: [...prev.proof.stats.cpuHistory, nextCpu].slice(-24),
              totalCpuSecs:
                prev.proof.stats.totalCpuSecs + estimateCpuSecs(cpuCost),
              roots: [
                { hash: result.newRoot, state: "live" },
                { hash: result.oldRoot, state: "nullified" },
                ...prev.proof.stats.roots.slice(0, 6),
              ],
            },
          },
        };
      });

      await new Promise((resolve) => setTimeout(resolve, postDoneHoldMs));
      set((prev) => ({
        ...prev,
        proof: {
          ...prev.proof,
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
          stats: prev.proof.stats,
        },
      }));
    }
  },
}));
