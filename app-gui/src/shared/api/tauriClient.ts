// Frontend client for the driver.
//
// Runs in two modes:
//
// - **Tauri (desktop app)**: dispatches via `@tauri-apps/api` IPC. The Rust
//   side is `app-gui/src-tauri/src/lib.rs`.
// - **Browser (website + `dobjd` CLI)**: dispatches via HTTP + SSE to a local
//   `dobjd` HTTP server on port 7717 by default. Override with
//   `VITE_DOBJD_URL` at build time.
//
// Every exported function keeps the same name and signature it had before
// the split, so call sites elsewhere in the app don't change.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppSettingsPayload,
  CpuSample,
  LoadGuiInventoryResult,
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
  LoadGuiInventoryResult,
  ObjectRecordPayload,
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

// --- Inventory --------------------------------------------------------------

export function loadGuiInventory(): Promise<LoadGuiInventoryResult> {
  if (isTauri) return invoke<LoadGuiInventoryResult>("load_gui_inventory");
  return fetch(`${HTTP_BASE}/inventory`).then(httpJson<LoadGuiInventoryResult>);
}

// --- State root -------------------------------------------------------------

export function getGlobalStateRoot(): Promise<string> {
  if (isTauri) return invoke<string>("get_global_state_root");
  return fetch(`${HTTP_BASE}/state-root`).then(async (res) => {
    if (!res.ok) {
      throw new Error(((await res.json()) as { error?: string }).error ?? `HTTP ${res.status}`);
    }
    return res.text();
  });
}

// --- Run action -------------------------------------------------------------

export function runAction(input: RunActionInput): Promise<RunActionResult> {
  if (isTauri) return invoke<RunActionResult>("run_action", { input });
  return fetch(`${HTTP_BASE}/actions/run`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ input }),
  }).then(httpJson<RunActionResult>);
}

// --- Objects directory ------------------------------------------------------

export function getObjectsDir(): Promise<string> {
  if (isTauri) return invoke<string>("get_objects_dir");
  return fetch(`${HTTP_BASE}/objects/dir`)
    .then(httpJson<{ path: string }>)
    .then((r) => r.path);
}

export function openObjectsDir(): Promise<string> {
  if (isTauri) return invoke<string>("open_objects_dir");
  // Browser can't open the OS file manager; just return the path so the
  // caller can show it.
  return getObjectsDir();
}

// --- Picking / reading a .dobj file -----------------------------------------

export function pickDobjFilePath(): Promise<string> {
  if (isTauri) return invoke<string>("pick_dobj_file_path");
  // No native file picker in the browser; UI uses drag-and-drop instead and
  // calls `parseDobjBytes` directly. Surface a sentinel error for callers
  // that haven't been migrated yet.
  return Promise.reject(
    new Error("File picker unavailable in browser mode; use drag-and-drop"),
  );
}

export function readDobjFile(path: string): Promise<ObjectRecordPayload> {
  if (isTauri) return invoke<ObjectRecordPayload>("read_dobj_file", { path });
  return Promise.reject(
    new Error(
      "readDobjFile by path is not available in browser mode; use parseDobjBytes",
    ),
  );
}

/// Parse a dropped `.dobj` File in the browser by uploading its bytes to
/// `dobjd`. Returns the same shape as `readDobjFile`.
export function parseDobjBytes(file: File): Promise<ObjectRecordPayload> {
  if (isTauri) {
    // In desktop mode the file already has a path on disk; the existing
    // pick/read flow handles this. This helper is browser-mode only.
    return Promise.reject(
      new Error("parseDobjBytes is only used in browser mode"),
    );
  }
  const fd = new FormData();
  fd.append("file", file);
  return fetch(`${HTTP_BASE}/objects/parse`, { method: "POST", body: fd }).then(
    httpJson<ObjectRecordPayload>,
  );
}

// --- Settings ---------------------------------------------------------------

export function getAppSettings(): Promise<AppSettingsPayload> {
  if (isTauri) return invoke<AppSettingsPayload>("get_app_settings");
  return fetch(`${HTTP_BASE}/settings`).then(httpJson<AppSettingsPayload>);
}

export function saveAppSettings(
  input: AppSettingsPayload,
): Promise<AppSettingsPayload> {
  if (isTauri) return invoke<AppSettingsPayload>("save_app_settings", { input });
  return fetch(`${HTTP_BASE}/settings`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  }).then(httpJson<AppSettingsPayload>);
}

// --- CPU sampling -----------------------------------------------------------

export function sampleAppCpu(): Promise<CpuSample> {
  if (isTauri) return invoke<CpuSample>("sample_app_cpu");
  // Web mode doesn't expose host process CPU. Return a stable zero so any
  // status-bar widget keeps rendering.
  return Promise.resolve({ usagePct: 0, totalCpuSecs: 0 });
}

// --- Event subscriptions ----------------------------------------------------
//
// In Tauri mode we use the existing event system (one channel per name).
// In browser mode we open a single `EventSource` against `/events` and
// dispatch by the `type` discriminator on every message.

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
  if (isTauri) {
    return listen<RunActionProgress>("run-action-progress", (event) => {
      handler(event.payload);
    });
  }
  return subscribeHttp<RunActionProgress>("run-action-progress", handler);
}

export function listenObjectsChanged(handler: () => void): Promise<UnlistenFn> {
  if (isTauri) {
    return listen("objects-changed", () => {
      handler();
    });
  }
  return subscribeHttp<unknown>("objects-changed", () => handler());
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

export function listenMcpActionStarted(
  handler: (event: { actionId: string }) => void,
): Promise<UnlistenFn> {
  if (isTauri) {
    return listen<{ actionId: string }>("mcp-action-started", (event) => {
      handler(event.payload);
    });
  }
  return subscribeHttp<{ actionId: string }>("mcp-action-started", handler);
}
