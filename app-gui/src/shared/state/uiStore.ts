import { create } from "zustand";
import { createDobj, type CreateDobjProgress } from "../api/tauriClient";
import { initialUiState } from "./initialState";
import type { AppUiState } from "../types/domain";

type ProofStatus = "idle" | "generating" | "committing" | "summary" | "error";
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

interface ProofSummary {
  nullified: string[];
  live: string[];
}

interface ProofState {
  runDobjId: string | null;
  status: ProofStatus;
  methodName: string | null;
  cpuCost: string | null;
  args: string[];
  messages: string[];
  steps: ProofStep[];
  oldRoot: string | null;
  newRoot: string | null;
  summary: ProofSummary | null;
  error: string | null;
  stats: ProofStats;
}

interface UiStoreState extends AppUiState {
  proof: ProofState;
  selectItem: (itemId: string) => void;
  selectRecipe: (recipeId: string) => void;
  toggleNullified: () => void;
  recordCpuSample: (usagePct: number, totalCpuSecs: number) => void;
  applyCreateDobjProgress: (event: CreateDobjProgress) => void;
  runProof: (input: {
    id: string;
    methodName: string;
    inputFiles: string[];
    cpuCost: string;
  }) => Promise<void>;
}

export const useUiStore = create<UiStoreState>((set) => ({
  ...initialUiState,
  proof: {
    runDobjId: null,
    status: "idle",
    methodName: null,
    cpuCost: null,
    args: [],
    messages: [],
    steps: [],
    oldRoot: null,
    newRoot: null,
    summary: null,
    error: null,
    stats: {
      cpuHistory: Array.from({ length: 24 }, () => 0),
      totalCpuSecs: 0,
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
        return {
          ...prev,
          activeItemId: null,
          contextSelection: { kind: "none" },
        };
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
        return {
          ...prev,
          activeRecipeId: null,
          contextSelection: { kind: "none" },
        };
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
  recordCpuSample: (usagePct, totalCpuSecs) =>
    set((prev) => {
      const nextUsage = Math.max(0, Math.min(100, Math.round(usagePct)));
      const nextTotal = Math.max(0, Math.floor(totalCpuSecs));
      return {
        ...prev,
        proof: {
          ...prev.proof,
          stats: {
            ...prev.proof.stats,
            cpuHistory: [...prev.proof.stats.cpuHistory, nextUsage].slice(-24),
            totalCpuSecs: nextTotal,
          },
        },
      };
    }),
  applyCreateDobjProgress: (event) =>
    set((prev) => {
      if (prev.proof.runDobjId !== event.dobjId) return prev;

      const nextSteps = prev.proof.steps.map((step) => ({ ...step }));
      const updateStep = (stepId: string, patch: Partial<ProofStep>) => {
        const index = nextSteps.findIndex((step) => step.id === stepId);
        if (index === -1) return;
        nextSteps[index] = { ...nextSteps[index], ...patch };
      };

      let nextStatus = prev.proof.status;
      let nextOldRoot = prev.proof.oldRoot;
      let nextNewRoot = prev.proof.newRoot;

      if (event.phase === "hash") {
        nextStatus = "generating";
        updateStep("hash", {
          status: event.status,
          detail: event.detail ?? prev.proof.cpuCost ?? "pending",
        });
      } else if (event.phase === "verify") {
        nextStatus = "generating";
        const index = event.verifyIndex ?? 0;
        updateStep(`verify-${index}`, {
          status: event.status,
          detail: event.detail ?? "input",
        });
      } else if (event.phase === "nullify") {
        nextStatus = "committing";
        nextOldRoot = event.oldRoot ?? event.detail ?? nextOldRoot;
        updateStep("nullify", {
          status: event.status,
          detail: event.detail ?? nextOldRoot ?? "pending",
        });
      } else if (event.phase === "commit") {
        nextStatus = "committing";
        nextNewRoot = event.newRoot ?? event.detail ?? nextNewRoot;
        updateStep("commit", {
          status: event.status,
          detail: event.detail ?? nextNewRoot ?? "pending",
        });
      }

      const shouldUpdateRoots =
        event.phase === "commit" &&
        event.status === "done" &&
        !!(nextOldRoot && nextNewRoot);
      const liveRoot = nextNewRoot ?? "";
      const nullifiedRoot = nextOldRoot ?? "";

      return {
        ...prev,
        proof: {
          ...prev.proof,
          status: nextStatus,
          messages: [...prev.proof.messages, event.message].slice(-8),
          steps: nextSteps,
          oldRoot: nextOldRoot,
          newRoot: nextNewRoot,
          stats: shouldUpdateRoots
            ? {
                ...prev.proof.stats,
                roots: [
                  { hash: liveRoot, state: "live" },
                  { hash: nullifiedRoot, state: "nullified" },
                  ...prev.proof.stats.roots.slice(0, 6),
                ],
              }
            : prev.proof.stats,
        },
      };
    }),
  runProof: async ({ id, methodName, inputFiles, cpuCost }) => {
    const postDoneHoldMs = 2800;
    const verifyTargets = inputFiles.length > 0 ? inputFiles : ["(no inputs)"];
    const normalizeOutputLabel = (value: string) =>
      value.endsWith(".dobj") ? value : `${value}.dobj`;

    set((prev) => {
      if (
        prev.proof.status === "generating" ||
        prev.proof.status === "committing" ||
        prev.proof.status === "summary"
      )
        return prev;
      return {
        ...prev,
        proof: {
          runDobjId: id,
          status: "generating",
          methodName,
          cpuCost,
          args: inputFiles,
          messages: ["Creating digital object..."],
          steps: [
            {
              id: "hash",
              label: "Hashing",
              detail: cpuCost,
              status: "pending",
            },
            ...verifyTargets.map((arg, i) => ({
              id: `verify-${i}`,
              label: "Verifying",
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
          summary: null,
          error: null,
          stats: prev.proof.stats,
        },
      };
    });

    try {
      const result = await createDobj({
        dobjId: id,
        inputFiles,
      });
      set((prev) => {
        if (prev.proof.runDobjId !== id) return prev;
        const consumed = inputFiles.map(normalizeOutputLabel);
        const produced = [`${methodName}_output.dobj`];
        return {
          ...prev,
          proof: {
            ...prev.proof,
            status: "summary",
            oldRoot: result.oldRoot,
            newRoot: result.newRoot,
            summary: {
              nullified: consumed,
              live: produced,
            },
          },
        };
      });

      await new Promise((resolve) => setTimeout(resolve, postDoneHoldMs));
      set((prev) => ({
        ...prev,
        proof: {
          ...prev.proof,
          runDobjId: null,
          status: "idle",
          methodName: null,
          cpuCost: null,
          args: [],
          messages: [],
          steps: [],
          oldRoot: null,
          newRoot: null,
          summary: null,
          error: null,
        },
      }));
    } catch (error) {
      set((prev) => ({
        ...prev,
        proof: {
          runDobjId: null,
          status: "error",
          methodName,
          cpuCost,
          args: inputFiles,
          messages: [],
          steps: [],
          oldRoot: null,
          newRoot: null,
          summary: null,
          error: error instanceof Error ? error.message : "Failed to run proof",
          stats: prev.proof.stats,
        },
      }));
    }
  },
}));
