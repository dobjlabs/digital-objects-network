// LAYER 2 — SIMULATION
// Pure functions ported from the Bitcraft spec. No React, no Tauri.
// Operates on a spec-shaped inventory map: { [objectId]: { count, ...stateFields } }.

import {
  BAR_AMBER,
  BAR_BLUE,
  BAR_GREY,
  BASE_MS,
  LEVEL_ORDER,
  OBJ_MAP,
  type Recipe,
} from "./data";

export interface InvSlot {
  count: number;
  durability?: number;
  busy?: boolean;
  level?: number;
}

export type Inv = Record<string, InvSlot>;

// ── reads ─────────────────────────────────────────────────────────────────────

export const invCount = (inv: Inv, id: string): number => inv[id]?.count ?? 0;
export const invBusy = (inv: Inv, id: string): boolean => inv[id]?.busy ?? false;

// ── writes — every helper returns a new inventory, never mutates ──────────────

export function invAdd(inv: Inv, id: string, qty = 1): Inv {
  const cur = inv[id] ?? { count: 0, ...(OBJ_MAP[id]?.state ?? {}) };
  return { ...inv, [id]: { ...cur, count: cur.count + qty } };
}

export function invTake(inv: Inv, id: string, qty = 1): Inv | null {
  const cur = inv[id];
  if (!cur || cur.count < qty) return null;
  return { ...inv, [id]: { ...cur, count: cur.count - qty } };
}

export function invSet(inv: Inv, id: string, fields: Partial<InvSlot>): Inv {
  const cur = inv[id] ?? { count: 0, ...(OBJ_MAP[id]?.state ?? {}) };
  return { ...inv, [id]: { ...cur, ...fields } };
}

// How many more uses a wears item has — caps batch size for tool recipes.
// Sums durability across instances: (count - 1) full tools + remaining on the
// active one.
export const usesAvailable = (inv: Inv, id: string | undefined): number => {
  if (!id) return Number.POSITIVE_INFINITY;
  const slot = inv[id];
  if (!slot || slot.count === 0) return 0;
  const maxDur = (OBJ_MAP[id]?.state?.durability as number | undefined) ?? 1;
  return (slot.count - 1) * maxDur + (slot.durability ?? 0);
};

// ── duration resolution ──────────────────────────────────────────────────────
// Each job instance bakes its predicted duration on start. Real proof time
// drives completion (see useJobQueue), so this is just the visual prediction
// the segmented bar animates against.

let _jid = 0;
export const nextJid = (): number => ++_jid;

export function resolveDur(r: Recipe): number {
  const pow = (): number => -(r.pow_ms ?? 0) * Math.log(Math.random());
  switch (r.mechanic) {
    case "pow":
      return BASE_MS + pow();
    case "vdf":
      return BASE_MS + (r.vdf_ms ?? 0);
    case "both":
      return BASE_MS + (r.vdf_ms ?? 0) + pow();
    default:
      return BASE_MS; // "none"
  }
}

// ── game rules ───────────────────────────────────────────────────────────────

export function currentLevel(inv: Inv): "none" | "machine_1" | "machine_2" {
  if (invCount(inv, "assembler") > 0) return "machine_2";
  if (invCount(inv, "basic_asm") > 0) return "machine_1";
  return "none";
}

export function isUnlocked(r: Recipe, lvl: string, inv: Inv): boolean {
  if (r.cat === "mine" || r.cat === "farm") return true;
  if (LEVEL_ORDER[lvl] < LEVEL_ORDER[r.level || "none"]) return false;
  if (r.station && invCount(inv, r.station) === 0) return false;
  if (r.uses && usesAvailable(inv, r.uses) === 0) return false;
  const keys = Object.keys(r.inp);
  return keys.length === 0 || keys.every((k) => invCount(inv, k) > 0);
}

export function maxBatch(inv: Inv, inp: Record<string, number>): number {
  const keys = Object.keys(inp);
  if (!keys.length) return Number.POSITIVE_INFINITY;
  return Math.min(...keys.map((k) => Math.floor(invCount(inv, k) / inp[k])));
}

// Consume qty × inputs from inv. Returns new inv or null if insufficient.
export function doCraft(
  inv: Inv,
  inp: Record<string, number>,
  qty = 1,
): Inv | null {
  let next: Inv | null = inv;
  for (const [k, v] of Object.entries(inp)) {
    next = invTake(next as Inv, k, v * qty);
    if (!next) return null;
  }
  return next;
}

// ── job phase ────────────────────────────────────────────────────────────────

export interface JobInst {
  jid: number;
  start: number;
  dur: number;
}

export function jobPhase(
  recipe: Recipe,
  inst: JobInst,
  now: number,
): { label: string; color: string } {
  const vdf_ms = recipe.vdf_ms || 0;
  const pow_actual = Math.max(inst.dur - BASE_MS - vdf_ms, 0);
  const elapsed = now - inst.start;
  if (elapsed < pow_actual) return { label: "hashing", color: BAR_AMBER };
  if (elapsed < pow_actual + vdf_ms)
    return { label: "sequentially hashing", color: BAR_BLUE };
  return { label: "generating proof", color: BAR_GREY };
}
