export type DashboardStatus =
  | "healthy"
  | "lagging"
  | "recovering"
  | "stalled";

export type DashboardSlotStatus = "pending" | "applied";

export interface DashboardSummary {
  status: DashboardStatus;
  statusReason: string;
  lastProcessedSlot: number | null;
  beaconHeadSlot: number | null;
  slotLag: number | null;
  beaconHeadBlockNumber: number | null;
  blockLag: number | null;
  lastProcessedBlockNumber: number | null;
  currentBlockNumber: number | null;
  currentGsr: string | null;
  txCount: number;
  nullifierCount: number;
  gsrCount: number;
  pendingRecoveryCount: number;
  cursorUpdatedAt: string;
}

export interface DashboardRecentSlot {
  slot: number;
  executionBlockNumber: number | null;
  status: DashboardSlotStatus;
  isEmpty: boolean;
  blockRoot: string | null;
  parentRoot: string | null;
  txCount: number;
  nullifierCount: number;
  gsrHash: string | null;
  updatedAt: string;
}

export interface DashboardRecentSlotsResponse {
  slots: DashboardRecentSlot[];
}

export interface TxLookupResult {
  txHash: string;
  present: boolean;
  lastProcessedSlot: number | null;
  currentGsr: string | null;
}

interface RawDashboardSummary {
  status: DashboardStatus;
  status_reason: string;
  last_processed_slot: number | null;
  beacon_head_slot: number | null;
  slot_lag: number | null;
  beacon_head_block_number: number | null;
  block_lag: number | null;
  last_processed_block_number: number | null;
  current_block_number: number | null;
  current_gsr: string | null;
  tx_count: number;
  nullifier_count: number;
  gsr_count: number;
  pending_recovery_count: number;
  cursor_updated_at: string;
}

interface RawDashboardRecentSlot {
  slot: number;
  execution_block_number: number | null;
  status: DashboardSlotStatus;
  is_empty: boolean;
  block_root: string | null;
  parent_root: string | null;
  tx_count: number;
  nullifier_count: number;
  gsr_hash: string | null;
  updated_at: string;
}

interface RawDashboardRecentSlotsResponse {
  slots: RawDashboardRecentSlot[];
}

interface RawTxLookupResult {
  tx_hash: string;
  present: boolean;
  last_processed_slot: number | null;
  current_gsr: string | null;
}

type ApiErrorKind = "network" | "http" | "decode";

export class ApiClientError extends Error {
  readonly kind: ApiErrorKind;
  readonly status: number | null;

  constructor(message: string, kind: ApiErrorKind, status: number | null = null) {
    super(message);
    this.name = "ApiClientError";
    this.kind = kind;
    this.status = status;
  }
}

const rawBaseUrl = import.meta.env.VITE_SYNCHRONIZER_API_BASE_URL?.trim();
export const synchronizerApiBaseUrl = (
  rawBaseUrl && rawBaseUrl.length > 0 ? rawBaseUrl : "http://127.0.0.1:3000"
).replace(/\/+$/, "");

async function readErrorMessage(response: Response): Promise<string> {
  const text = await response.text();
  if (!text) {
    return `Request failed with status ${response.status}`;
  }

  try {
    const parsed = JSON.parse(text) as { error?: string };
    if (parsed.error) {
      return parsed.error;
    }
  } catch {
    return text;
  }

  return text;
}

async function fetchJson<T>(path: string): Promise<T> {
  let response: Response;

  try {
    response = await fetch(`${synchronizerApiBaseUrl}${path}`, {
      headers: {
        Accept: "application/json",
      },
    });
  } catch (error) {
    const message =
      error instanceof Error ? error.message : "Network request failed";
    throw new ApiClientError(message, "network");
  }

  if (!response.ok) {
    throw new ApiClientError(
      await readErrorMessage(response),
      "http",
      response.status,
    );
  }

  try {
    return (await response.json()) as T;
  } catch (error) {
    const message =
      error instanceof Error ? error.message : "Failed to decode response JSON";
    throw new ApiClientError(message, "decode", response.status);
  }
}

function mapDashboardSummary(raw: RawDashboardSummary): DashboardSummary {
  return {
    status: raw.status,
    statusReason: raw.status_reason,
    lastProcessedSlot: raw.last_processed_slot,
    beaconHeadSlot: raw.beacon_head_slot,
    slotLag: raw.slot_lag,
    beaconHeadBlockNumber: raw.beacon_head_block_number,
    blockLag: raw.block_lag,
    lastProcessedBlockNumber: raw.last_processed_block_number,
    currentBlockNumber: raw.current_block_number,
    currentGsr: raw.current_gsr,
    txCount: raw.tx_count,
    nullifierCount: raw.nullifier_count,
    gsrCount: raw.gsr_count,
    pendingRecoveryCount: raw.pending_recovery_count,
    cursorUpdatedAt: raw.cursor_updated_at,
  };
}

function mapRecentSlot(raw: RawDashboardRecentSlot): DashboardRecentSlot {
  return {
    slot: raw.slot,
    executionBlockNumber: raw.execution_block_number,
    status: raw.status,
    isEmpty: raw.is_empty,
    blockRoot: raw.block_root,
    parentRoot: raw.parent_root,
    txCount: raw.tx_count,
    nullifierCount: raw.nullifier_count,
    gsrHash: raw.gsr_hash,
    updatedAt: raw.updated_at,
  };
}

function mapRecentSlotsResponse(
  raw: RawDashboardRecentSlotsResponse,
): DashboardRecentSlotsResponse {
  return {
    slots: raw.slots.map(mapRecentSlot),
  };
}

function mapTxLookupResult(raw: RawTxLookupResult): TxLookupResult {
  return {
    txHash: raw.tx_hash,
    present: raw.present,
    lastProcessedSlot: raw.last_processed_slot,
    currentGsr: raw.current_gsr,
  };
}

export async function fetchDashboardSummary(): Promise<DashboardSummary> {
  return mapDashboardSummary(
    await fetchJson<RawDashboardSummary>("/v1/dashboard/summary"),
  );
}

export async function fetchRecentSlots(
  limit = 25,
): Promise<DashboardRecentSlotsResponse> {
  return mapRecentSlotsResponse(
    await fetchJson<RawDashboardRecentSlotsResponse>(
      `/v1/dashboard/recent-slots?limit=${limit}`,
    ),
  );
}

export async function fetchTxLookup(txHash: string): Promise<TxLookupResult> {
  return mapTxLookupResult(
    await fetchJson<RawTxLookupResult>(
      `/v1/state/tx/${encodeURIComponent(txHash.trim())}`,
    ),
  );
}
