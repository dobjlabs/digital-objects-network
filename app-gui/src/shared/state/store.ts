import { create } from "zustand";
import {
  loadGuiInventory,
  runAction,
  type ActionPayload as Action,
  type InventoryObjectPayload as InventoryObject,
  type RunActionProgress,
} from "../api/tauriClient";
import { normalizeErrorMessage } from "../error";

type ProofStatus = "idle" | "generating" | "committing" | "summary" | "error";
type StepStatus = "pending" | "running" | "done";

interface ProofStep {
  id: string;
  label: string;
  detail: string;
  status: StepStatus;
}

interface ProofStats {
  cpuHistory: number[];
  totalCpuSecs: number;
  globalStateRoot: string | null;
}

interface ProofSummary {
  nullified: string[];
  live: string[];
}

interface ProofState {
  runActionId: string | null;
  status: ProofStatus;
  args: string[];
  messages: string[];
  steps: ProofStep[];
  oldRoot: string | null;
  newRoot: string | null;
  summary: ProofSummary | null;
  error: string | null;
  stats: ProofStats;
}

export type ContextSelection =
  | { kind: "none" }
  | { kind: "object"; objectId: string }
  | { kind: "action"; actionId: string };

export interface AppState {
  contextSelection: ContextSelection;
  activeObjectId: string | null;
  activeActionId: string | null;
  showNullifiedItems: boolean;
  inventory: InventoryObject[];
  actions: Action[];
  /** Bare class/action names that appear in more than one loaded plugin. */
  nameCollisions: string[];
  proof: ProofState;
  hydrateData: () => Promise<void>;
  selectObject: (objectId: string) => void;
  selectAction: (actionId: string) => void;
  clearSelection: () => void;
  toggleNullified: () => void;
  recordCpuSample: (usagePct: number, totalCpuSecs: number) => void;
  setGlobalStateRoot: (hash: string | null) => void;
  applyRunActionProgress: (event: RunActionProgress) => void;
  initProofPanel: (input: { actionId: string; args: string[] }) => void;
  runProof: (input: {
    actionId: string;
    inputBindings: Array<{
      objectPath: string;
      label: string;
    }>;
  }) => Promise<void>;
}

const initialAppState: Pick<
  AppState,
  "contextSelection" | "activeObjectId" | "activeActionId" | "showNullifiedItems"
> = {
  contextSelection: { kind: "none" },
  activeObjectId: null,
  activeActionId: null,
  showNullifiedItems: false,
};

export const useStore = create<AppState>((set, get) => ({
  ...initialAppState,
  inventory: [],
  actions: [],
  nameCollisions: [],
  proof: {
    runActionId: null,
    status: "idle",
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
      globalStateRoot: null,
    },
  },
  hydrateData: async () => {
    const data = await loadGuiInventory();
    set((prev) => ({
      ...prev,
      inventory: data.inventory,
      actions: data.actions,
      nameCollisions: data.nameCollisions,
    }));
  },
  selectObject: (objectId) =>
    set((prev) => {
      if (
        prev.activeObjectId === objectId &&
        prev.activeActionId === null &&
        prev.contextSelection.kind === "object" &&
        prev.contextSelection.objectId === objectId
      ) {
        return {
          ...prev,
          activeObjectId: null,
          contextSelection: { kind: "none" },
        };
      }
      return {
        ...prev,
        activeObjectId: objectId,
        activeActionId: null,
        contextSelection: { kind: "object", objectId },
      };
    }),
  selectAction: (actionId) =>
    set((prev) => {
      if (
        prev.activeActionId === actionId &&
        prev.activeObjectId === null &&
        prev.contextSelection.kind === "action" &&
        prev.contextSelection.actionId === actionId
      ) {
        return {
          ...prev,
          activeActionId: null,
          contextSelection: { kind: "none" },
        };
      }
      return {
        ...prev,
        activeObjectId: null,
        activeActionId: actionId,
        contextSelection: { kind: "action", actionId },
      };
    }),
  clearSelection: () =>
    set((prev) => ({
      ...prev,
      activeObjectId: null,
      activeActionId: null,
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
  setGlobalStateRoot: (hash) =>
    set((prev) => {
      const nextHash = hash?.trim() || null;
      if (prev.proof.stats.globalStateRoot === nextHash) {
        return prev;
      }
      return {
        ...prev,
        proof: {
          ...prev.proof,
          stats: {
            ...prev.proof.stats,
            globalStateRoot: nextHash,
          },
        },
      };
    }),
  applyRunActionProgress: (event) =>
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

      if (event.phase === "generateProof") {
        nextStatus = "generating";
        updateStep("generate-proof", {
          status: event.status,
          detail: event.message,
        });
      } else if (event.phase === "commit") {
        nextStatus = "committing";
        nextOldRoot = event.oldRoot ?? nextOldRoot;
        nextNewRoot = event.newRoot ?? nextNewRoot;
        updateStep("commit", {
          status: event.status,
          detail: event.message,
        });
      }

      return {
        ...prev,
        proof: {
          ...prev.proof,
          status: nextStatus,
          messages: [...prev.proof.messages, event.message].slice(-8),
          steps: nextSteps,
          oldRoot: nextOldRoot,
          newRoot: nextNewRoot,
          stats: prev.proof.stats,
        },
      };
    }),
  initProofPanel: ({ actionId, args }) =>
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
          args,
          messages: ["Running action..."],
          steps: [
            {
              id: "generate-proof",
              label: "Generate Proof",
              detail: "pending",
              status: "pending",
            },
            {
              id: "commit",
              label: "Commit",
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
    }),
  runProof: async ({ actionId, inputBindings }) => {
    const postDoneHoldMs = 2800;
    const verifyTargets =
      inputBindings.length > 0
        ? inputBindings.map((binding) => binding.label)
        : ["(no inputs)"];

    get().initProofPanel({ actionId, args: verifyTargets });

    try {
      const result = await runAction({
        actionId,
        inputObjectPaths: inputBindings.map((binding) => binding.objectPath),
      });

      set((prev) => {
        if (prev.proof.runActionId !== actionId) return prev;
        return {
          ...prev,
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

      const hydrateData = useStore.getState().hydrateData;
      await hydrateData();

      await new Promise((resolve) => setTimeout(resolve, postDoneHoldMs));
      set((prev) => ({
        ...prev,
        proof: {
          ...prev.proof,
          runActionId: null,
          status: "idle",
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
      const errorMessage = normalizeErrorMessage(error, "Failed to run action");
      console.error("Failed to run SDK action:", error);
      set((prev) => ({
        ...prev,
        proof: {
          runActionId: null,
          status: "error",
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
