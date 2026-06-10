// Frontend client for the driver.
//
// All driver-related operations (inventory, actions, run_action, state-root,
// settings, /events) go to a single `dobjd` process over HTTP, regardless of
// whether the page is loaded inside Tauri or a plain browser. The driver
// lives in exactly one process; every client is thin.
//
// The few remaining `isTauri` branches are for desktop-only conveniences
// that don't touch the driver at all:
//
// - native file picker for `.dobj` (`pick_dobj_file_path`)
// - in-memory parse of a picked file (`read_dobj_file`)
// - process CPU sample for the desktop status bar (`sample_app_cpu`)
// - native menu event for `Cmd+,` settings shortcut (`open-settings`)
//
// Override the dobjd URL with `VITE_DOBJD_URL` at build time. Default:
// `http://127.0.0.1:7717`.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ActionPayload,
  AppSettingsPayload,
  CpuSample,
  InventoryObjectPayload,
  ObjectRecordPayload,
  ObjectSummaryPayload,
  RunAccepted,
  RunActionInput,
  RunActionProgress,
  RunState,
} from "./wireTypes";

export type {
  ActionPayload,
  AppSettingsPayload,
  CpuSample,
  InventoryObjectPayload,
  ObjectRecordPayload,
  ObjectSummaryPayload,
  QualifiedNamePayload,
  RunAccepted,
  RunActionInput,
  RunActionProgress,
  RunActionResult,
  RunState,
  RunStatus,
} from "./wireTypes";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const HTTP_BASE =
  (import.meta.env.VITE_DOBJD_URL as string | undefined) ??
  "http://127.0.0.1:7717";

const DEFAULT_TIMEOUT_MS = 10_000;

// fetch wrapper that bounds wall time and turns the two common dobjd-down
// failure modes into actionable messages: a killed daemon (fetch rejects
// with TypeError) and a hung daemon (fetch never resolves, surfaced via
// AbortSignal.timeout). Pass `timeoutMs: null` for endpoints that
// legitimately block.
async function dobjdFetch(
  path: string,
  init: RequestInit & { timeoutMs?: number | null } = {},
): Promise<Response> {
  const { timeoutMs = DEFAULT_TIMEOUT_MS, ...fetchInit } = init;
  const signal = timeoutMs != null ? AbortSignal.timeout(timeoutMs) : undefined;
  try {
    return await fetch(`${HTTP_BASE}${path}`, { ...fetchInit, signal });
  } catch (err) {
    if (err instanceof DOMException && err.name === "TimeoutError") {
      throw new Error(
        `dobjd ${path} timed out after ${timeoutMs}ms — is the daemon responsive?`,
      );
    }
    if (err instanceof TypeError) {
      throw new Error(
        `dobjd unreachable at ${HTTP_BASE} — is the daemon running?`,
      );
    }
    throw err;
  }
}

async function httpJson<T>(res: Response): Promise<T> {
  if (!res.ok) {
    let message = `HTTP ${res.status}`;
    try {
      const body = (await res.json()) as { error?: string };
      if (body && typeof body.error === "string") message = body.error;
    } catch {
      // body not JSON; keep status-only message
    }
    throw new Error(message);
  }
  return (await res.json()) as T;
}

// === Driver-backed operations: always HTTP ==================================

// Inventory and the action catalog are independent reads; callers run them
// in parallel via Promise.all rather than letting one block the other.
export function loadInventory(): Promise<InventoryObjectPayload[]> {
  return dobjdFetch("/inventory").then(httpJson<InventoryObjectPayload[]>);
}

export function loadActions(): Promise<ActionPayload[]> {
  return dobjdFetch("/actions").then(httpJson<ActionPayload[]>);
}

export function getStateRoot(): Promise<string> {
  return dobjdFetch("/state-root").then(httpJson<string>);
}

// Start a run. dobjd registers it and returns immediately with a handle
// (runId + status); proof generation + commit happen on a background worker.
// Follow progress via the per-run SSE stream and read the terminal outcome
// with `getRun`, so a dropped connection can't lose the result.
export function runAction(input: RunActionInput): Promise<RunAccepted> {
  return dobjdFetch("/actions/run", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ input }),
  }).then(httpJson<RunAccepted>);
}

// Current state of a run by id: status, the result once it succeeds, an error
// if it fails, and the progress log. Polled to detect completion and to
// recover the outcome if live progress was missed.
export function getRun(runId: string): Promise<RunState> {
  return dobjdFetch(`/actions/runs/${encodeURIComponent(runId)}`).then(
    httpJson<RunState>,
  );
}

// Import an external `.dobj` (one not produced by this driver). The body
// is the raw file contents as a string; the driver validates + files it and
// returns the object summary. 409 if it's already held or already spent.
export function importObject(dobj: string): Promise<ObjectSummaryPayload> {
  return dobjdFetch("/objects/import", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ dobj }),
    // Import does one synchronizer round-trip; allow more than the default
    // read timeout, but not unbounded like /actions/run.
    timeoutMs: 30_000,
  }).then(httpJson<ObjectSummaryPayload>);
}

export function getObjectsDir(): Promise<string> {
  return dobjdFetch("/objects/dir")
    .then(httpJson<{ path: string }>)
    .then((r) => r.path);
}

export function openObjectsDir(): Promise<string> {
  if (isTauri) return invoke<string>("open_objects_dir");
  // Browsers can't reveal native folders. Fall back to returning the path
  // so the UI can show / copy it.
  return getObjectsDir();
}

export function getAppSettings(): Promise<AppSettingsPayload> {
  return dobjdFetch("/settings").then(httpJson<AppSettingsPayload>);
}

export function saveAppSettings(
  input: AppSettingsPayload,
): Promise<AppSettingsPayload> {
  return dobjdFetch("/settings", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  }).then(httpJson<AppSettingsPayload>);
}

// === Desktop-only conveniences (not driver-backed): Tauri IPC, no fallback ==

export function pickDobjFilePath(): Promise<string> {
  if (isTauri) return invoke<string>("pick_dobj_file_path");
  return Promise.reject(new Error("File picker unavailable in browser mode"));
}

export function readDobjFile(path: string): Promise<ObjectRecordPayload> {
  if (isTauri) return invoke<ObjectRecordPayload>("read_dobj_file", { path });
  return Promise.reject(new Error("readDobjFile by path is desktop-only"));
}

export function sampleAppCpu(): Promise<CpuSample> {
  if (isTauri) return invoke<CpuSample>("sample_app_cpu");
  // The desktop status bar widget only makes sense inside Tauri. Return
  // zeros in browser so any code that polls this keeps rendering.
  return Promise.resolve({ usagePct: 0, totalCpuSecs: 0 });
}

// === Event subscriptions ====================================================
//
// The global `/events` stream is a firehose used for coarse refresh triggers.
// The active proof panel follows its own replayable `/actions/runs/{id}/events`
// stream so it cannot miss early progress emitted before the frontend learned
// the daemon-assigned run id. The `open-settings` event comes from the Tauri
// native menu and is desktop-only.

type Handler<T> = (payload: T) => void;

let sharedEventSource: EventSource | null = null;
const httpHandlers = new Map<string, Set<Handler<unknown>>>();

function ensureEventSource() {
  if (sharedEventSource) return;
  sharedEventSource = new EventSource(`${HTTP_BASE}/events`);
  sharedEventSource.onmessage = (e) => {
    let parsed: { type?: string } & Record<string, unknown>;
    try {
      parsed = JSON.parse(e.data);
    } catch {
      return;
    }
    const { type, ...payload } = parsed;
    if (!type) return;
    const set = httpHandlers.get(type);
    if (!set) return;
    for (const h of set) h(payload);
  };
  sharedEventSource.onerror = () => {
    // EventSource auto-reconnects; nothing to do.
  };
}

function subscribeHttp<T>(
  type: string,
  handler: Handler<T>,
): Promise<UnlistenFn> {
  ensureEventSource();
  let set = httpHandlers.get(type);
  if (!set) {
    set = new Set();
    httpHandlers.set(type, set);
  }
  set.add(handler as Handler<unknown>);
  const unlisten: UnlistenFn = () => {
    const s = httpHandlers.get(type);
    if (!s) return;
    s.delete(handler as Handler<unknown>);
  };
  return Promise.resolve(unlisten);
}

export function listenRunActionProgress(
  handler: (event: RunActionProgress) => void,
): Promise<UnlistenFn> {
  return subscribeHttp<RunActionProgress>("run-action-progress", handler);
}

function isTerminalRunProgress(event: RunActionProgress): boolean {
  return (
    event.status === "failed" ||
    (event.phase === "commit" && event.status === "done")
  );
}

export function listenRunActionProgressForRun(
  runId: string,
  handler: (event: RunActionProgress) => void,
): Promise<UnlistenFn> {
  const source = new EventSource(
    `${HTTP_BASE}/actions/runs/${encodeURIComponent(runId)}/events`,
  );
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    source.close();
  };
  source.onmessage = (event) => {
    let parsed: RunActionProgress;
    try {
      parsed = JSON.parse(event.data) as RunActionProgress;
    } catch {
      return;
    }
    handler(parsed);
    if (isTerminalRunProgress(parsed)) close();
  };
  source.onerror = () => {
    // EventSource auto-reconnects, resending Last-Event-ID so the daemon can
    // replay from the run's buffered progress log.
  };
  return Promise.resolve(close);
}

export function listenOpenSettings(handler: () => void): Promise<UnlistenFn> {
  if (isTauri) {
    return listen("open-settings", () => {
      handler();
    });
  }
  // No native menu in browser; nothing to listen for. Return a no-op
  // unlisten so callers can still pattern-match cleanup the same way.
  void handler;
  return Promise.resolve(() => {});
}
