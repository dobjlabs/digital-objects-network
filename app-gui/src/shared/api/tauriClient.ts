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
  RunActionInput,
  RunActionProgress,
  RunActionResult,
} from "./wireTypes";

export type {
  ActionPayload,
  AppSettingsPayload,
  CpuSample,
  InventoryObjectPayload,
  ObjectRecordPayload,
  QualifiedNamePayload,
  RunActionInput,
  RunActionProgress,
  RunActionResult,
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
  return fetch(`${HTTP_BASE}/inventory`).then(
    httpJson<InventoryObjectPayload[]>,
  );
}

export function loadActions(): Promise<ActionPayload[]> {
  return fetch(`${HTTP_BASE}/actions`).then(httpJson<ActionPayload[]>);
}

export function getGlobalStateRoot(): Promise<string> {
  return fetch(`${HTTP_BASE}/state-root`).then(async (res) => {
    if (!res.ok) {
      throw new Error(
        ((await res.json()) as { error?: string }).error ??
          `HTTP ${res.status}`,
      );
    }
    return res.text();
  });
}

export function runAction(input: RunActionInput): Promise<RunActionResult> {
  return fetch(`${HTTP_BASE}/actions/run`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ input }),
  }).then(httpJson<RunActionResult>);
}

export function getObjectsDir(): Promise<string> {
  return fetch(`${HTTP_BASE}/objects/dir`)
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
  return fetch(`${HTTP_BASE}/settings`).then(httpJson<AppSettingsPayload>);
}

export function saveAppSettings(
  input: AppSettingsPayload,
): Promise<AppSettingsPayload> {
  return fetch(`${HTTP_BASE}/settings`, {
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
// Driver events (`run-action-progress`) always come from dobjd over a single
// SSE connection. The `open-settings` event comes from the Tauri native menu
// and is desktop-only.

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
