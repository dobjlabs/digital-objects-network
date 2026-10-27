import { create } from "zustand";
import {
  getRun,
  importObject as importObjectApi,
  loadActions,
  loadObjects,
  listenRunActionProgressForRun,
  startRun,
  type ActionArgValues,
  type ActionPayload as Action,
  type ObjectSummaryPayload as ObjectSummary,
  type QualifiedNamePayload,
  type RunActionProgress,
  type RunActionResult,
  type RunState,
} from "../api/httpClient";
import { normalizeErrorMessage } from "../error";
import { qualifiedEq } from "../objectUtils";

const MAX_CONSECUTIVE_RUN_POLL_ERRORS = 5;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Poll a run until it reaches a terminal state. Local HTTP, and the daemon's
 * supervisor guarantees every run reaches a terminal state while dobjd stays
 * up. Transient follow-up polling failures are tolerated so a momentary HTTP
 * hiccup is not reported as an action failure. */
async function pollRunToTerminal(runId: string): Promise<RunState> {
  let consecutiveErrors = 0;
  for (;;) {
    try {
      const state = await getRun(runId);
      consecutiveErrors = 0;
      if (state.status === "succeeded" || state.status === "failed") {
        return state;
      }
      await sleep(1000);
    } catch (error) {
      consecutiveErrors += 1;
      const message =
        error instanceof Error ? error.message : String(error ?? "unknown");
      if (consecutiveErrors >= MAX_CONSECUTIVE_RUN_POLL_ERRORS) {
        throw new Error(
          `Lost contact while following run ${runId}; it may still complete. Sync objects to reconcile. Last error: ${message}`,
        );
      }
      console.warn(
        `Failed to poll run ${runId} (${consecutiveErrors}/${MAX_CONSECUTIVE_RUN_POLL_ERRORS}); retrying`,
        error,
      );
      await sleep(Math.min(1000 * consecutiveErrors, 5000));
    }
  }
}

type ProofStatus = "idle" | "generating" | "committing" | "summary" | "error";
type StepStatus = "pending" | "running" | "done";

interface ProofStep {
  id: string;
  label: string;
  detail: string;
  status: StepStatus;
}

interface ProofStats {
  stateRoot: string | null;
}

interface ProofSummary {
  inputs: string[];
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
  summary: ProofSummary | null;
  error: string | null;
  stats: ProofStats;
}

function isProofTerminal(proof: ProofState): boolean {
  return proof.status === "summary" || proof.status === "error";
}

function finishProof(proof: ProofState, result: RunActionResult): ProofState {
  if (isProofTerminal(proof)) return proof;
  return {
    ...proof,
    status: "summary",
    oldRoot: result.oldRoot,
    summary: { inputs: result.inputFiles, live: result.outputFiles },
    steps: proof.steps.map((step) => ({
      ...step,
      status: "done",
      detail:
        step.id === "commit" ? "Transaction landed" : "Proof generation complete",
    })),
    messages: [...proof.messages, "Transaction landed"].slice(-8),
    error: null,
  };
}

function failProof(proof: ProofState, message: string): ProofState {
  if (isProofTerminal(proof)) return proof;
  return { ...proof, status: "error", error: message };
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
  objects: ObjectSummary[];
  actions: Action[];
  proof: ProofState;
  hydrateData: () => Promise<void>;
  /** Update or insert an object by filename, keeping the list sorted. */
  applyObjectUpdate: (object: ObjectSummary) => void;
  removeObject: (fileName: string) => void;
  importObject: (dobj: string) => Promise<void>;
  selectObject: (contentHash: string) => void;
  selectAction: (action: QualifiedNamePayload) => void;
  clearSelection: () => void;
  toggleNullified: () => void;
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
    args?: ActionArgValues;
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
  objects: [],
  actions: [],
  proof: {
    runActionId: null,
    action: null,
    status: "idle",
    args: [],
    messages: [],
    steps: [],
    oldRoot: null,
    summary: null,
    error: null,
    stats: {
      stateRoot: null,
    },
  },
  hydrateData: async () => {
    // Objects hits the synchronizer (network-bound); the action catalog
    // is purely local. Fire them in parallel so the catalog isn't gated
    // on the slower call.
    const [objects, actions] = await Promise.all([
      loadObjects(),
      loadActions(),
    ]);
    set((prev) => ({ ...prev, objects, actions }));
  },
  applyObjectUpdate: (object) =>
    set((prev) => {
      const index = prev.objects.findIndex(
        (existing) => existing.fileName === object.fileName,
      );
      if (index !== -1) {
        const objects = prev.objects.slice();
        objects[index] = object;
        return { ...prev, objects };
      }
      const objects = [...prev.objects, object].sort((a, b) =>
        a.fileName.localeCompare(b.fileName),
      );
      return { ...prev, objects };
    }),
  removeObject: (fileName) =>
    set((prev) => ({
      ...prev,
      objects: prev.objects.filter((object) => object.fileName !== fileName),
    })),
  importObject: async (dobj) => {
    // Show the imported object before its event arrives.
    const imported = await importObjectApi(dobj);
    get().applyObjectUpdate(imported);
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
      if (prev.proof.runActionId !== event.runId || isProofTerminal(prev.proof)) {
        return prev;
      }
      if (event.status === "failed") {
        return { proof: failProof(prev.proof, event.message) };
      }
      if (event.phase === "commit" && event.status === "done") {
        return {
          proof: finishProof(prev.proof, {
            runId: event.runId,
            oldRoot: event.oldRoot ?? prev.proof.oldRoot ?? "",
            inputFiles: event.inputFiles ?? [],
            outputFiles: event.outputFiles ?? [],
          }),
        };
      }

      const isCommit = event.phase === "commit";
      const stepId = isCommit ? "commit" : "generate-proof";
      const stepStatus = event.status;
      return {
        proof: {
          ...prev.proof,
          status: isCommit ? "committing" : "generating",
          messages: [...prev.proof.messages, event.message].slice(-8),
          steps: prev.proof.steps.map((step) =>
            step.id === stepId
              ? { ...step, status: stepStatus, detail: event.message }
              : step,
          ),
          oldRoot: event.oldRoot ?? prev.proof.oldRoot,
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
          summary: null,
          error: null,
        },
      };
    }),
  runProof: async ({ action, inputBindings, args }) => {
    const postDoneHoldMs = 2800;
    const verifyTargets =
      inputBindings.length > 0
        ? inputBindings.map((binding) => binding.label)
        : ["(no inputs)"];
    let stopRunEvents: (() => void) | null = null;
    let runId: string | null = null;
    const previousRunId = get().proof.runActionId;

    try {
      // dobjd assigns the run id and returns it immediately; the proof +
      // commit run on a background worker. Initialize the panel against that
      // id, then subscribe to the run's replayable SSE before polling for the
      // authoritative terminal result.
      const accepted = await startRun({
        action,
        inputObjectPaths: inputBindings.map((binding) => binding.objectPath),
        ...(args && Object.keys(args).length > 0 ? { args } : {}),
      });
      runId = accepted.runId;
      get().initProofPanel({ runId, action, args: verifyTargets });
      stopRunEvents = listenRunActionProgressForRun(runId, (event) => {
        get().applyRunActionProgress(event);
      });

      const finalState = await pollRunToTerminal(runId);
      if (finalState.status !== "succeeded" || !finalState.result) {
        throw new Error(
          finalState.error ?? `Run ended in state: ${finalState.status}`,
        );
      }
      const result = finalState.result;

      set((prev) =>
        prev.proof.runActionId === runId
          ? { proof: finishProof(prev.proof, result) }
          : prev,
      );
    } catch (error) {
      const errorMessage = normalizeErrorMessage(error, "Failed to run action");
      console.error("Failed to run SDK action:", error);
      set((prev) => {
        if (runId) {
          return prev.proof.runActionId === runId
            ? { proof: failProof(prev.proof, errorMessage) }
            : prev;
        }
        if (
          prev.proof.runActionId !== previousRunId ||
          (prev.proof.status !== "idle" && prev.proof.status !== "error")
        ) {
          return prev;
        }
        return {
          proof: failProof(
            {
              ...prev.proof,
              runActionId: null,
              status: "idle",
              action,
              args: verifyTargets,
              messages: [],
              steps: [],
              oldRoot: null,
              summary: null,
            },
            errorMessage,
          ),
        };
      });
    } finally {
      stopRunEvents?.();
      if (
        runId &&
        get().proof.runActionId === runId &&
        get().proof.status === "summary"
      ) {
        await sleep(postDoneHoldMs);
        get().resetProofPanel(runId);
      }
    }
  },
}));
