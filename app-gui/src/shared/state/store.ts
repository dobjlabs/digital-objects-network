import { create } from "zustand";
import {
  importObject as importObjectApi,
  loadActions,
  loadInventory,
  runAction,
  type ActionPayload as Action,
  type InventoryObjectPayload as InventoryObject,
  type QualifiedNamePayload,
  type RunActionProgress,
} from "../api/tauriClient";
import { normalizeErrorMessage } from "../error";
import { qualifiedEq } from "../objectUtils";

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
  stateRoot: string | null;
}

interface ProofSummary {
  nullified: string[];
  live: string[];
}

interface ProofState {
  /** Server-minted run id; matched against incoming SSE progress events.
   * Distinct from `action` because two concurrent runs of the same action
   * would otherwise collide. */
  runActionId: string | null;
  /** Identity (qualified) of the action currently being proved, when one is. */
  action: QualifiedNamePayload | null;
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
  | { kind: "object"; contentHash: string }
  | { kind: "action"; action: QualifiedNamePayload };

export interface AppState {
  contextSelection: ContextSelection;
  activeObjectContentHash: string | null;
  activeAction: QualifiedNamePayload | null;
  showNullifiedItems: boolean;
  inventory: InventoryObject[];
  actions: Action[];
  proof: ProofState;
  hydrateData: () => Promise<void>;
  importObject: (dobj: string) => Promise<void>;
  selectObject: (contentHash: string) => void;
  selectAction: (action: QualifiedNamePayload) => void;
  clearSelection: () => void;
  toggleNullified: () => void;
  recordCpuSample: (usagePct: number, totalCpuSecs: number) => void;
  setStateRoot: (hash: string | null) => void;
  applyRunActionProgress: (event: RunActionProgress) => void;
  initProofPanel: (input: {
    runId: string;
    action: QualifiedNamePayload;
    args: string[];
  }) => void;
  resetProofPanel: (runId?: string) => void;
  runProof: (input: {
    action: QualifiedNamePayload;
    inputBindings: Array<{
      objectPath: string;
      label: string;
    }>;
  }) => Promise<void>;
}

const initialAppState: Pick<
  AppState,
  | "contextSelection"
  | "activeObjectContentHash"
  | "activeAction"
  | "showNullifiedItems"
> = {
  contextSelection: { kind: "none" },
  activeObjectContentHash: null,
  activeAction: null,
  showNullifiedItems: false,
};

export const useStore = create<AppState>((set, get) => ({
  ...initialAppState,
  inventory: [],
  actions: [],
  proof: {
    runActionId: null,
    action: null,
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
      stateRoot: null,
    },
  },
  hydrateData: async () => {
    // Inventory hits the synchronizer (network-bound); the action catalog
    // is purely local. Fire them in parallel so the catalog isn't gated
    // on the slower call.
    const [inventory, actions] = await Promise.all([
      loadInventory(),
      loadActions(),
    ]);
    set((prev) => ({ ...prev, inventory, actions }));
  },
  importObject: async (dobj) => {
    // The driver validates + files the object; refetch so the new row shows
    // up with its class emoji/description folded in (import returns the bare
    // summary). Errors propagate to the caller to surface in the UI.
    await importObjectApi(dobj);
    await get().hydrateData();
  },
  selectObject: (contentHash) =>
    set((prev) => {
      if (
        prev.activeObjectContentHash === contentHash &&
        prev.activeAction === null &&
        prev.contextSelection.kind === "object" &&
        prev.contextSelection.contentHash === contentHash
      ) {
        return {
          ...prev,
          activeObjectContentHash: null,
          contextSelection: { kind: "none" },
        };
      }
      return {
        ...prev,
        activeObjectContentHash: contentHash,
        activeAction: null,
        contextSelection: { kind: "object", contentHash },
      };
    }),
  selectAction: (action) =>
    set((prev) => {
      if (
        prev.activeAction !== null &&
        qualifiedEq(prev.activeAction, action) &&
        prev.activeObjectContentHash === null &&
        prev.contextSelection.kind === "action" &&
        qualifiedEq(prev.contextSelection.action, action)
      ) {
        return {
          ...prev,
          activeAction: null,
          contextSelection: { kind: "none" },
        };
      }
      return {
        ...prev,
        activeObjectContentHash: null,
        activeAction: action,
        contextSelection: { kind: "action", action },
      };
    }),
  clearSelection: () =>
    set((prev) => ({
      ...prev,
      activeObjectContentHash: null,
      activeAction: null,
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
  setStateRoot: (hash) =>
    set((prev) => {
      const nextHash = hash?.trim() || null;
      if (prev.proof.stats.stateRoot === nextHash) {
        return prev;
      }
      return {
        ...prev,
        proof: {
          ...prev.proof,
          stats: {
            ...prev.proof.stats,
            stateRoot: nextHash,
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
      let nextSummary = prev.proof.summary;

      if (event.phase === "generateProof") {
        nextStatus = "generating";
        updateStep("generate-proof", {
          status: event.status,
          detail: event.message,
        });
      } else if (event.phase === "commit") {
        nextStatus = event.status === "done" ? "summary" : "committing";
        nextOldRoot = event.oldRoot ?? nextOldRoot;
        nextNewRoot = event.newRoot ?? nextNewRoot;
        updateStep("commit", {
          status: event.status,
          detail: event.message,
        });
        if (event.status === "done") {
          nextSummary = {
            nullified: event.nullifiedFiles ?? [],
            live: event.outputFiles ?? [],
          };
        }
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
          summary: nextSummary,
          stats: prev.proof.stats,
        },
      };
    }),
  initProofPanel: ({ runId, action, args }) =>
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
          runActionId: runId,
          action,
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
  resetProofPanel: (runId) =>
    set((prev) => {
      if (runId && prev.proof.runActionId !== runId) return prev;
      return {
        ...prev,
        proof: {
          ...prev.proof,
          runActionId: null,
          action: null,
          status: "idle",
          args: [],
          messages: [],
          steps: [],
          oldRoot: null,
          newRoot: null,
          summary: null,
          error: null,
        },
      };
    }),
  runProof: async ({ action, inputBindings }) => {
    const postDoneHoldMs = 2800;
    const verifyTargets =
      inputBindings.length > 0
        ? inputBindings.map((binding) => binding.label)
        : ["(no inputs)"];

    // Mint a per-run id up front so progress events streaming back during
    // the action can be matched against this run before runAction returns.
    // Two concurrent runs of the same action would collide on action alone.
    const runId = crypto.randomUUID();
    get().initProofPanel({ runId, action, args: verifyTargets });

    try {
      const result = await runAction({
        action,
        inputObjectPaths: inputBindings.map((binding) => binding.objectPath),
        runId,
      });

      set((prev) => {
        if (prev.proof.runActionId !== runId) return prev;
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
      get().resetProofPanel(runId);
    } catch (error) {
      const errorMessage = normalizeErrorMessage(error, "Failed to run action");
      console.error("Failed to run SDK action:", error);
      set((prev) => ({
        ...prev,
        proof: {
          runActionId: null,
          action: null,
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
