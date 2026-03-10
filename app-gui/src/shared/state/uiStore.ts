import { create } from "zustand";
import {
  loadGuiBootstrap,
  runSdkAction,
  type RunSdkActionProgress,
  type InventoryItemPayload,
  type RecipePayload,
} from "../api/tauriClient";
import { initialUiState } from "./initialState";
import type {
  AppUiState,
  InventoryItem,
  Recipe,
  StatTone,
} from "../types/domain";

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
  runActionId: string | null;
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
  items: InventoryItem[];
  recipes: Recipe[];
  proof: ProofState;
  hydrateData: () => Promise<void>;
  selectItem: (itemId: string) => void;
  selectRecipe: (recipeId: string) => void;
  clearSelection: () => void;
  toggleNullified: () => void;
  recordCpuSample: (usagePct: number, totalCpuSecs: number) => void;
  applyRunSdkActionProgress: (event: RunSdkActionProgress) => void;
  runProof: (input: {
    actionId: string;
    methodName: string;
    inputBindings: Array<{
      objectPath: string;
      label: string;
    }>;
    cpuCost: string;
  }) => Promise<void>;
}

const normalizeTone = (value: string | undefined): StatTone | undefined => {
  if (!value) return undefined;
  if (value === "neutral") return "neutral";
  if (value === "good") return "good";
  if (value === "warn") return "warn";
  if (value === "danger") return "danger";
  return undefined;
};

const mapItem = (item: InventoryItemPayload): InventoryItem => ({
  id: item.id,
  fileName: item.fileName,
  emoji: item.emoji,
  validity: item.validity,
  stateRoot: item.stateRoot,
  nullifier: item.nullifier,
  classMeta: item.classMeta,
  sourceAction: item.sourceAction,
  description: item.description,
  methods: item.methods.map((method) => ({
    methodName: method.methodName,
    cpuCost: method.cpuCost,
    readsBlock: method.readsBlock,
    args: method.args.map((arg) => ({
      kind: "class",
      label: arg.label,
      classHash: arg.classHash,
    })),
  })),
  stats: item.stats.map((stat) => ({
    key: stat.key,
    value: stat.value,
    tone: normalizeTone(stat.tone),
  })),
});

const mapRecipe = (recipe: RecipePayload): Recipe => ({
  id: recipe.id,
  group: recipe.group,
  name: recipe.name,
  emoji: recipe.emoji,
  hash: recipe.hash,
  verb: recipe.verb,
  desc: recipe.desc,
  cpu: recipe.cpu,
  readsBlock: recipe.readsBlock,
  args: recipe.args.map((arg) => ({
    kind: "class",
    label: arg.label,
    classHash: arg.classHash,
  })),
  unlocked: recipe.unlocked,
});

const formatRunError = (error: unknown): string => {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message;
  }
  if (typeof error === "string" && error.trim().length > 0) {
    return error;
  }
  if (error && typeof error === "object") {
    const record = error as Record<string, unknown>;
    for (const key of ["message", "error", "cause"]) {
      const value = record[key];
      if (typeof value === "string" && value.trim().length > 0) {
        return value;
      }
    }
    try {
      const serialized = JSON.stringify(error);
      if (serialized && serialized !== "{}") {
        return serialized;
      }
    } catch {
      // fall through to generic text
    }
  }
  return "Failed to run action";
};

export const useUiStore = create<UiStoreState>((set) => ({
  ...initialUiState,
  items: [],
  recipes: [],
  proof: {
    runActionId: null,
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
        { hash: "0x0000...0000", state: "live" },
        { hash: "0x0000...0000", state: "nullified" },
      ],
    },
  },
  hydrateData: async () => {
    const data = await loadGuiBootstrap();
    set((prev) => ({
      ...prev,
      items: data.objects.map(mapItem),
      recipes: data.actions.map(mapRecipe),
    }));
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
  clearSelection: () =>
    set((prev) => ({
      ...prev,
      activeItemId: null,
      activeRecipeId: null,
      contextSelection: { kind: "none" },
    })),
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
  applyRunSdkActionProgress: (event) =>
    set((prev) => {
      if (prev.proof.runActionId !== event.runId) return prev;

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
  runProof: async ({
    actionId,
    methodName,
    inputBindings,
    cpuCost,
  }) => {
    const postDoneHoldMs = 2800;
    const verifyTargets =
      inputBindings.length > 0
        ? inputBindings.map((binding) => binding.label)
        : ["(no inputs)"];

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
          runActionId: actionId,
          status: "generating",
          methodName,
          cpuCost,
          args: verifyTargets,
          messages: ["Running SDK action..."],
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
      const result = await runSdkAction({
        actionId,
        inputs: inputBindings.map((binding) => ({
          objectPath: binding.objectPath,
          label: binding.label,
        })),
      });

      set((prev) => {
        if (prev.proof.runActionId !== actionId) return prev;
        return {
          ...prev,
          items: result.objects.map(mapItem),
          proof: {
            ...prev.proof,
            status: "summary",
            oldRoot: result.oldRoot,
            newRoot: result.newRoot,
            summary: {
              nullified: result.nullifiedFiles,
              live: result.outputFiles,
            },
          },
        };
      });

      await new Promise((resolve) => setTimeout(resolve, postDoneHoldMs));
      set((prev) => ({
        ...prev,
        proof: {
          ...prev.proof,
          runActionId: null,
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
      const errorMessage = formatRunError(error);
      console.error("Failed to run SDK action:", error);
      set((prev) => ({
        ...prev,
        proof: {
          runActionId: null,
          status: "error",
          methodName,
          cpuCost,
          args: verifyTargets,
          messages: [],
          steps: [],
          oldRoot: null,
          newRoot: null,
          summary: null,
          error: errorMessage,
          stats: prev.proof.stats,
        },
      }));
    }
  },
}));
