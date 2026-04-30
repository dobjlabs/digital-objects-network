// Bridges the backend's flat list of .dobj files to the spec's
// stack-counted inventory shape, and resolves a recipe's input list into the
// concrete file paths that `run_action` expects.

import type { InventoryObjectPayload } from "../shared/api/wireTypes";
import {
  CLASS_TO_OBJ,
  OBJ_MAP,
  OBJ_TO_CLASS,
  type Recipe,
} from "./data";
import type { Inv } from "./sim";

// ── inventory aggregation ─────────────────────────────────────────────────────

/**
 * Group live backend objects by their spec id and produce the spec-shaped
 * inventory. For tools (which carry a per-instance durability), the surfaced
 * durability is the lowest live one — that's the next instance the driver
 * will pick up when the recipe is invoked, matching the spec's "active tool"
 * model.
 */
export function buildInvFromBackend(inventory: InventoryObjectPayload[]): Inv {
  const byObjId = new Map<string, InventoryObjectPayload[]>();
  for (const obj of inventory) {
    if (obj.status !== "live") continue;
    const id = CLASS_TO_OBJ[obj.className];
    if (!id) continue; // class not in our spec — ignore
    const bucket = byObjId.get(id) ?? [];
    bucket.push(obj);
    byObjId.set(id, bucket);
  }

  const inv: Inv = {};
  for (const [id, items] of byObjId) {
    const def = OBJ_MAP[id];
    const slot: Inv[string] = {
      count: items.length,
      ...(def?.state ?? {}),
    };
    // Tools track durability per-instance — surface the minimum so the bar
    // shows uses-left for the about-to-deplete tool.
    if (def?.cat === "tool") {
      const durabilities = items
        .map((i) => extractNumberField(i.obj, "durability"))
        .filter((d): d is number => d !== null);
      if (durabilities.length > 0) {
        slot.durability = Math.min(...durabilities);
      }
    }
    inv[id] = slot;
  }
  return inv;
}

function extractNumberField(obj: unknown, field: string): number | null {
  if (obj && typeof obj === "object" && field in (obj as Record<string, unknown>)) {
    const v = (obj as Record<string, unknown>)[field];
    if (typeof v === "number") return v;
  }
  return null;
}

// ── input file picker ────────────────────────────────────────────────────────

/**
 * Resolve a recipe's symbolic inputs to concrete .dobj file names that
 * run_action can pass to the driver.
 *
 * The order MUST match the action's input declaration order in plugin.rhai.
 * For episode-1 we follow this convention:
 *   1. level prerequisite (TouchMachineI / TouchMachineII subaction)
 *   2. station prerequisite (TouchBlastFurnace / etc subaction)
 *   3. recipe.inp ingredients in declaration order
 *   4. tool (UseDrillBit / etc subaction at the end)
 *
 * The plugin.rhai handlers were written so the parent action's first call is
 * the level-touch (when present), then the station-touch (when present), then
 * the ingredient inputs in object-order, then the tool subaction last. The
 * driver consumes inputs in that same order.
 *
 * Returns null if the inventory can't satisfy a single unit of the recipe —
 * the caller should treat that as "can't run this".
 */
export function pickInputFiles(
  recipe: Recipe,
  inventory: InventoryObjectPayload[],
): string[] | null {
  const live = inventory.filter((o) => o.status === "live");

  // Index by class for fast picking; each pick removes the chosen file from
  // its pool so we don't double-spend within a single resolution.
  const pool = new Map<string, InventoryObjectPayload[]>();
  for (const obj of live) {
    const arr = pool.get(obj.className) ?? [];
    arr.push(obj);
    pool.set(obj.className, arr);
  }
  // Deterministic order: oldest fileName first — keeps repeated craft calls
  // stable across reloads.
  for (const arr of pool.values()) {
    arr.sort((a, b) => a.fileName.localeCompare(b.fileName));
  }

  const take = (className: string): InventoryObjectPayload | null => {
    const arr = pool.get(className);
    if (!arr || arr.length === 0) return null;
    return arr.shift() ?? null;
  };

  const picked: InventoryObjectPayload[] = [];

  // 1. Level (machine_1 / machine_2 → MachineI / MachineII)
  if (recipe.level) {
    const className =
      recipe.level === "machine_2" ? "MachineII" : "MachineI";
    const obj = take(className);
    if (!obj) return null;
    picked.push(obj);
  }

  // 2. Station
  if (recipe.station) {
    const className = OBJ_TO_CLASS[recipe.station];
    if (!className) return null;
    const obj = take(className);
    if (!obj) return null;
    picked.push(obj);
  }

  // 3. Ingredients
  for (const [objId, qty] of Object.entries(recipe.inp)) {
    const className = OBJ_TO_CLASS[objId];
    if (!className) return null;
    for (let i = 0; i < qty; i++) {
      const obj = take(className);
      if (!obj) return null;
      picked.push(obj);
    }
  }

  // 4. Tool
  if (recipe.uses) {
    const className = OBJ_TO_CLASS[recipe.uses];
    if (!className) return null;
    const obj = take(className);
    if (!obj) return null;
    picked.push(obj);
  }

  return picked.map((obj) => obj.fileName);
}
