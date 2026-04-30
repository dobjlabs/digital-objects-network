// LAYER 1 — DATA
// Pure declarations ported verbatim from the Bitcraft spec, plus two name
// mapping tables that bridge the spec's snake_case ids to the backend's
// PascalCase class and action names.

export const CONFIG = {
  batchSizes: [1, 5, 10] as const,
};

export const BASE_MS = 10_000;

// ── OBJECTS ───────────────────────────────────────────────────────────────────
// Every item in the game. One entry per type.

export type ObjectCat =
  | "raw"
  | "byproduct"
  | "processed"
  | "tool"
  | "level"
  | "station";

export interface ObjectDef {
  id: string;
  label: string;
  cat: ObjectCat;
  state: Record<string, number | boolean>;
}

export const OBJECTS: ObjectDef[] = [
  // ── raw resources ─────────────────────────────────────────────────────────
  { id: "iron",   label: "Iron",   cat: "raw", state: {} },
  { id: "copper", label: "Copper", cat: "raw", state: {} },
  { id: "oil",    label: "Oil",    cat: "raw", state: {} },
  { id: "sulfur", label: "Sulfur", cat: "raw", state: {} },
  { id: "water",  label: "Water",  cat: "raw", state: {} },
  { id: "cane",   label: "Cane",   cat: "raw", state: {} },
  { id: "hemp",   label: "Hemp",   cat: "raw", state: {} },

  // ── t1 processed ──────────────────────────────────────────────────────────
  { id: "ingot", label: "Ingot", cat: "processed", state: {} },
  { id: "plate", label: "Plate", cat: "processed", state: {} },
  { id: "pulp",  label: "Pulp",  cat: "processed", state: {} },
  { id: "fiber", label: "Fiber", cat: "processed", state: {} },
  { id: "acid",  label: "Acid",  cat: "processed", state: {} },

  // ── byproducts & intermediates ────────────────────────────────────────────
  { id: "tar",      label: "Tar",      cat: "byproduct", state: {} },
  { id: "fuel",     label: "Fuel",     cat: "byproduct", state: {} },
  { id: "gas",      label: "Gas",      cat: "byproduct", state: {} },
  { id: "slag",     label: "Slag",     cat: "byproduct", state: {} },
  { id: "flux",     label: "Flux",     cat: "byproduct", state: {} },
  { id: "sludge",   label: "Sludge",   cat: "byproduct", state: {} },
  { id: "mold",     label: "Mold",     cat: "byproduct", state: {} },
  { id: "catalyst", label: "Catalyst", cat: "byproduct", state: {} },
  { id: "binder",   label: "Binder",   cat: "byproduct", state: {} },
  { id: "lye",      label: "Lye",      cat: "byproduct", state: {} },

  // ── tools (deplete on use) ────────────────────────────────────────────────
  { id: "drill_bit",      label: "Drill Bit",      cat: "tool", state: { durability: 5 } },
  { id: "soldering_iron", label: "Soldering Iron", cat: "tool", state: { durability: 5 } },
  { id: "pressure_valve", label: "Pressure Valve", cat: "tool", state: { durability: 3 } },

  // ── t2 processed ──────────────────────────────────────────────────────────
  { id: "steel",   label: "Steel",   cat: "processed", state: {} },
  { id: "wire",    label: "Wire",    cat: "processed", state: {} },
  { id: "cloth",   label: "Cloth",   cat: "processed", state: {} },
  { id: "board",   label: "Board",   cat: "processed", state: {} },
  { id: "wax",     label: "Wax",     cat: "processed", state: {} },
  { id: "grease",  label: "Grease",  cat: "processed", state: {} },
  { id: "solvent", label: "Solvent", cat: "processed", state: {} },
  { id: "coating", label: "Coating", cat: "processed", state: {} },
  { id: "rubber",  label: "Rubber",  cat: "processed", state: {} },
  { id: "extract", label: "Extract", cat: "processed", state: {} },
  { id: "gear",    label: "Gear",    cat: "processed", state: {} },
  { id: "coil",    label: "Coil",    cat: "processed", state: {} },

  // ── t3 processed ──────────────────────────────────────────────────────────
  { id: "bearing", label: "Bearing", cat: "processed", state: {} },
  { id: "circuit", label: "Circuit", cat: "processed", state: {} },
  { id: "canvas",  label: "Canvas",  cat: "processed", state: {} },
  { id: "panel",   label: "Panel",   cat: "processed", state: {} },
  { id: "pistons", label: "Pistons", cat: "processed", state: {} },
  { id: "resin",   label: "Resin",   cat: "processed", state: {} },

  // ── t4 processed ──────────────────────────────────────────────────────────
  { id: "engine",  label: "Engine",  cat: "processed", state: {} },
  { id: "casing",  label: "Casing",  cat: "processed", state: {} },
  { id: "payload", label: "Payload", cat: "processed", state: {} },

  // ── t5 ────────────────────────────────────────────────────────────────────
  { id: "rocket", label: "Rocket", cat: "processed", state: {} },

  // ── levels (gate recipe tiers, never consumed) ────────────────────────────
  { id: "basic_asm", label: "Machine I",  cat: "level", state: { level: 1 } },
  { id: "assembler", label: "Machine II", cat: "level", state: { level: 1 } },

  // ── stations (lock while a job runs) ──────────────────────────────────────
  { id: "blast_furnace",    label: "Blast Furnace",    cat: "station", state: { busy: false, durability: 1.0 } },
  { id: "circuit_fab",      label: "Circuit Fab",      cat: "station", state: { busy: false, durability: 1.0 } },
  { id: "cracking_unit",    label: "Cracking Unit",    cat: "station", state: { busy: false, durability: 1.0 } },
  { id: "reaction_chamber", label: "Reaction Chamber", cat: "station", state: { busy: false, durability: 1.0 } },
];

// Derived lookups — never hand-maintain these.
export const OBJ_MAP: Record<string, ObjectDef> = Object.fromEntries(
  OBJECTS.map((o) => [o.id, o]),
);
export const ITEMS: Record<string, string> = Object.fromEntries(
  OBJECTS.map((o) => [o.id, o.label]),
);
export const LEVEL_IDS = OBJECTS.filter((o) => o.cat === "level").map((o) => o.id);
export const STATION_IDS = OBJECTS.filter((o) => o.cat === "station").map((o) => o.id);
export const TOOL_IDS = OBJECTS.filter((o) => o.cat === "tool").map((o) => o.id);
export const EQUIPMENT_IDS = new Set([...LEVEL_IDS, ...STATION_IDS, ...TOOL_IDS]);

export const label = (k: string): string => ITEMS[k] || k;

// ── RECIPES ───────────────────────────────────────────────────────────────────

export type RecipeCat =
  | "mine"
  | "farm"
  | "t1"
  | "t2"
  | "gate"
  | "t3"
  | "t4"
  | "t5";

export type Mechanic = "pow" | "vdf" | "both" | "none";

export interface Recipe {
  id: string;
  label: string;
  cat: RecipeCat;
  inp: Record<string, number>;
  out: Record<string, number>;
  mechanic: Mechanic;
  pow_ms?: number;
  vdf_ms?: number;
  uses?: string;     // tool id
  station?: string;  // station id
  level?: string;    // "machine_1" | "machine_2"
}

export const RECIPES: Recipe[] = [
  // ── mines ─────────────────────────────────────────────────────────────────
  { id: "iron",   label: "Iron",   cat: "mine", inp: {}, out: { iron: 1 },   mechanic: "pow", pow_ms: 1000 },
  { id: "copper", label: "Copper", cat: "mine", inp: {}, out: { copper: 1 }, mechanic: "pow", pow_ms: 500 },
  { id: "oil",    label: "Oil",    cat: "mine", inp: {}, out: { oil: 1 },    mechanic: "pow", pow_ms: 3000 },
  { id: "sulfur", label: "Sulfur", cat: "mine", inp: {}, out: { sulfur: 1 }, mechanic: "vdf", vdf_ms: 0 },

  // ── farms ─────────────────────────────────────────────────────────────────
  { id: "water", label: "Water", cat: "farm", inp: {}, out: { water: 1 }, mechanic: "vdf", vdf_ms: 0 },
  { id: "cane",  label: "Cane",  cat: "farm", inp: {}, out: { cane: 1 },  mechanic: "vdf", vdf_ms: 1000 },
  { id: "hemp",  label: "Hemp",  cat: "farm", inp: {}, out: { hemp: 1 },  mechanic: "vdf", vdf_ms: 2000 },

  // ── t1 ────────────────────────────────────────────────────────────────────
  { id: "ingot",         label: "Ingot",            cat: "t1", inp: { iron: 1 },                           out: { ingot: 1 },                          mechanic: "vdf",  vdf_ms: 54_000 },
  { id: "ingot_flux",    label: "Ingot (flux)",     cat: "t1", inp: { iron: 1, flux: 1 },                  out: { ingot: 2 },                          mechanic: "vdf",  vdf_ms: 25_000 },
  { id: "ingot_drilled", label: "Ingot (drilled)",  cat: "t1", uses: "drill_bit", inp: { iron: 1 },        out: { ingot: 2 },                          mechanic: "vdf",  vdf_ms: 44_000 },
  { id: "plate",         label: "Plate",            cat: "t1", inp: { copper: 1 },                         out: { plate: 1 },                          mechanic: "vdf",  vdf_ms: 54_000 },
  { id: "pulp",          label: "Pulp",             cat: "t1", inp: { cane: 1 },                           out: { pulp: 3 },                           mechanic: "vdf",  vdf_ms: 24_000 },
  { id: "fiber",         label: "Fiber",            cat: "t1", inp: { hemp: 1 },                           out: { fiber: 3 },                          mechanic: "vdf",  vdf_ms: 28_000 },
  { id: "acid",          label: "Acid",             cat: "t1", inp: { sulfur: 1, water: 1 },               out: { acid: 2 },                           mechanic: "pow",  pow_ms: 100_000 },
  { id: "acid_flash",    label: "Acid (flash)",     cat: "t1", inp: { sulfur: 1, water: 1 },               out: { acid: 1 },                           mechanic: "pow",  pow_ms: 50_000 },
  { id: "acid_crude",    label: "Acid (crude)",     cat: "t1", inp: { sulfur: 1, water: 1 },               out: { acid: 3 },                           mechanic: "both", vdf_ms: 80_000,  pow_ms: 60_000 },
  { id: "refinery",         label: "Refinery",            cat: "t1", inp: { oil: 1, water: 1 },                                            out: { tar: 3, fuel: 1, gas: 1 },         mechanic: "both", vdf_ms: 72_000,  pow_ms: 48_000 },
  { id: "refinery_flash",   label: "Refinery (flash)",    cat: "t1", inp: { oil: 1, water: 1 },                                            out: { tar: 3, fuel: 1, gas: 1 },         mechanic: "pow",  pow_ms: 60_000 },
  { id: "refinery_crude",   label: "Refinery (crude)",    cat: "t1", inp: { oil: 2, water: 1 },                                            out: { tar: 3, fuel: 1, gas: 1, oil: 1 }, mechanic: "both", vdf_ms: 110_000, pow_ms: 60_000 },
  { id: "refinery_cracked", label: "Refinery (cracked)",  cat: "t1", station: "cracking_unit", inp: { oil: 1, water: 1 },                  out: { tar: 5, fuel: 3, gas: 2 },         mechanic: "both", vdf_ms: 72_000,  pow_ms: 48_000 },
  { id: "flux_recipe",      label: "Flux",                cat: "t1", inp: { slag: 2, water: 1 },                                           out: { flux: 2 },                         mechanic: "vdf",  vdf_ms: 15_000 },

  // ── t2 ────────────────────────────────────────────────────────────────────
  { id: "steel",       label: "Steel",        cat: "t2", inp: { ingot: 3 },                            out: { steel: 2 },             mechanic: "vdf", vdf_ms: 120_000 },
  { id: "steel_blast", label: "Steel (blast)", cat: "t2", station: "blast_furnace", inp: { ingot: 3 }, out: { steel: 2, slag: 1 },    mechanic: "vdf", vdf_ms: 70_000 },
  { id: "mold",        label: "Mold",          cat: "t2", inp: { slag: 2, tar: 1 },                    out: { mold: 2 },              mechanic: "vdf", vdf_ms: 20_000 },
  { id: "gear_cast",   label: "Gear (cast)",   cat: "t2", station: "blast_furnace", inp: { steel: 1, mold: 1 }, out: { gear: 5 },     mechanic: "vdf", vdf_ms: 30_000 },
  { id: "wire",        label: "Wire",          cat: "t2", inp: { plate: 1 },                           out: { wire: 3 },              mechanic: "vdf", vdf_ms: 24_000 },
  { id: "cloth",       label: "Cloth",         cat: "t2", inp: { fiber: 2 },                           out: { cloth: 1 },             mechanic: "vdf", vdf_ms: 52_000 },
  { id: "board",       label: "Board",         cat: "t2", inp: { pulp: 3, water: 1 },                  out: { board: 2, lye: 1 },     mechanic: "vdf", vdf_ms: 74_000 },
  { id: "wax",         label: "Wax",           cat: "t2", inp: { tar: 2 },                             out: { wax: 1 },               mechanic: "vdf", vdf_ms: 40_000 },
  { id: "grease",      label: "Grease",        cat: "t2", inp: { tar: 2 },                             out: { grease: 1 },            mechanic: "vdf", vdf_ms: 64_000 },
  { id: "solvent",     label: "Solvent",       cat: "t2", inp: { fuel: 1, acid: 1 },                   out: { solvent: 2 },           mechanic: "pow", pow_ms: 76_000 },
  { id: "solvent_lye", label: "Solvent (lye)", cat: "t2", inp: { lye: 2, fuel: 1, water: 1 },          out: { solvent: 2 },           mechanic: "vdf", vdf_ms: 66_000 },
  { id: "coating",     label: "Coating",       cat: "t2", inp: { fuel: 2, wax: 1 },                    out: { coating: 2 },           mechanic: "vdf", vdf_ms: 25_000 },
  { id: "rubber",       label: "Rubber",         cat: "t2", inp: { gas: 3 },                           out: { rubber: 1 },                  mechanic: "pow",  pow_ms: 96_000 },
  { id: "rubber_flash", label: "Rubber (flash)", cat: "t2", inp: { gas: 3 },                           out: { rubber: 1 },                  mechanic: "pow",  pow_ms: 48_000 },
  { id: "rubber_crude", label: "Rubber (crude)", cat: "t2", inp: { gas: 3 },                           out: { rubber: 1, gas: 1 },          mechanic: "both", vdf_ms: 80_000, pow_ms: 60_000 },
  { id: "extract",      label: "Extract",        cat: "t2", inp: { acid: 2 },                          out: { extract: 1, sludge: 1 },      mechanic: "pow",  pow_ms: 120_000 },
  { id: "extract_ctrl", label: "Extract (ctrl)", cat: "t2", station: "reaction_chamber", inp: { acid: 2 }, out: { extract: 1, sludge: 2 }, mechanic: "both", vdf_ms: 60_000, pow_ms: 25_000 },
  { id: "catalyst_recipe", label: "Catalyst",      cat: "t2", inp: { sludge: 3, wire: 1 },             out: { catalyst: 1 },          mechanic: "vdf", vdf_ms: 10_000 },
  { id: "binder",          label: "Binder",        cat: "t2", inp: { sludge: 3, solvent: 1 },          out: { binder: 1 },            mechanic: "vdf", vdf_ms: 18_000 },
  { id: "gear",            label: "Gear",          cat: "t2", inp: { steel: 2 },                       out: { gear: 3 },              mechanic: "vdf", vdf_ms: 36_000 },
  { id: "drill_bit_recipe",      label: "Drill Bit",      cat: "t2", inp: { iron: 1, gear: 1 },        out: { drill_bit: 1 },         mechanic: "vdf", vdf_ms: 20_000 },
  { id: "soldering_iron_recipe", label: "Soldering Iron", cat: "t2", inp: { wire: 2, acid: 1 },        out: { soldering_iron: 1 },    mechanic: "vdf", vdf_ms: 15_000 },
  { id: "pressure_valve_recipe", label: "Pressure Valve", cat: "t2", inp: { oil: 2, gear: 1 },         out: { pressure_valve: 1 },    mechanic: "vdf", vdf_ms: 20_000 },
  { id: "coil",            label: "Coil",          cat: "t2", inp: { wire: 3 },                        out: { coil: 1 },              mechanic: "vdf", vdf_ms: 60_000 },

  // ── gates ─────────────────────────────────────────────────────────────────
  { id: "basic_asm",     label: "Machine I",     cat: "gate",                     inp: { steel: 4, gear: 3, coil: 2 },           out: { basic_asm: 1 },     mechanic: "both", vdf_ms: 66_000, pow_ms: 44_000 },
  { id: "blast_furnace", label: "Blast Furnace", cat: "gate", level: "machine_1", inp: { steel: 3, gear: 2, coil: 1, acid: 2 }, out: { blast_furnace: 1 }, mechanic: "both", vdf_ms: 80_000, pow_ms: 40_000 },
  { id: "circuit_fab",   label: "Circuit Fab",   cat: "gate", level: "machine_1", inp: { bearing: 2, coil: 2, steel: 3, grease: 2 }, out: { circuit_fab: 1 }, mechanic: "both", vdf_ms: 90_000, pow_ms: 60_000 },
  { id: "cracking_unit", label: "Cracking Unit", cat: "gate", level: "machine_1", inp: { acid: 4, bearing: 3, grease: 3 },     out: { cracking_unit: 1 }, mechanic: "both", vdf_ms: 80_000, pow_ms: 60_000 },

  // ── t3 (requires Machine I) ───────────────────────────────────────────────
  { id: "bearing",            label: "Bearing",            cat: "t3", level: "machine_1", inp: { steel: 1, grease: 2 },                                      out: { bearing: 2 }, mechanic: "vdf",  vdf_ms: 40_000 },
  { id: "circuit",            label: "Circuit",            cat: "t3", level: "machine_1", inp: { wire: 2, steel: 1 },                                        out: { circuit: 1 }, mechanic: "both", vdf_ms: 126_000, pow_ms: 84_000 },
  { id: "circuit_soldered",   label: "Circuit (soldered)", cat: "t3", level: "machine_1", uses: "soldering_iron", inp: { wire: 2, steel: 1 },                out: { circuit: 1 }, mechanic: "vdf",  vdf_ms: 126_000 },
  { id: "circuit_fab_recipe", label: "Circuit (fab)",      cat: "t3", level: "machine_1", station: "circuit_fab", inp: { wire: 4, steel: 1 },                out: { circuit: 2 }, mechanic: "vdf",  vdf_ms: 100_000 },
  { id: "circuit_flash",      label: "Circuit (flash)",    cat: "t3", level: "machine_1", inp: { wire: 2, steel: 1 },                                        out: { circuit: 1 }, mechanic: "pow",  pow_ms: 110_000 },
  { id: "circuit_crude",      label: "Circuit (crude)",    cat: "t3", level: "machine_1", inp: { wire: 2, steel: 1 },                                        out: { circuit: 1, wire: 1 }, mechanic: "both", vdf_ms: 190_000, pow_ms: 50_000 },
  { id: "canvas",       label: "Canvas",         cat: "t3", level: "machine_1", inp: { cloth: 2, fiber: 1, wax: 1 }, out: { canvas: 1 },             mechanic: "vdf",  vdf_ms: 100_000 },
  { id: "canvas_lye",   label: "Canvas (lye)",   cat: "t3", level: "machine_1", inp: { cloth: 2, fiber: 1, lye: 1 }, out: { canvas: 1 },             mechanic: "vdf",  vdf_ms: 55_000 },
  { id: "canvas_flash", label: "Canvas (flash)", cat: "t3", level: "machine_1", inp: { cloth: 2, fiber: 1, wax: 1 }, out: { canvas: 1 },             mechanic: "pow",  pow_ms: 75_000 },
  { id: "canvas_crude", label: "Canvas (crude)", cat: "t3", level: "machine_1", inp: { cloth: 2, fiber: 1, wax: 1 }, out: { canvas: 1, cloth: 1 },   mechanic: "both", vdf_ms: 120_000, pow_ms: 30_000 },
  { id: "panel",        label: "Panel",          cat: "t3", level: "machine_1", inp: { board: 1, extract: 1 },      out: { panel: 2 },               mechanic: "none" },
  { id: "pistons",      label: "Pistons",        cat: "t3", level: "machine_1", inp: { bearing: 2, coil: 2, grease: 1 }, out: { pistons: 1 },        mechanic: "both", vdf_ms: 108_000, pow_ms: 62_000 },
  { id: "resin",            label: "Resin",              cat: "t3", level: "machine_1", inp: { grease: 1, solvent: 1, rubber: 1 },                              out: { resin: 1 },             mechanic: "both", vdf_ms: 198_000, pow_ms: 132_000 },
  { id: "resin_pressurized", label: "Resin (pressurized)", cat: "t3", level: "machine_1", uses: "pressure_valve", inp: { grease: 1, solvent: 1, rubber: 1 }, out: { resin: 1 },             mechanic: "vdf",  vdf_ms: 198_000 },
  { id: "resin_stable",     label: "Resin (stable)",     cat: "t3", level: "machine_1", station: "reaction_chamber", inp: { grease: 1, solvent: 1, rubber: 1, binder: 1 }, out: { resin: 2 }, mechanic: "vdf", vdf_ms: 230_000 },
  { id: "resin_flash",      label: "Resin (flash)",      cat: "t3", level: "machine_1", inp: { grease: 1, solvent: 1, rubber: 1 },                              out: { resin: 1 },             mechanic: "pow",  pow_ms: 165_000 },
  { id: "resin_crude",      label: "Resin (crude)",      cat: "t3", level: "machine_1", inp: { grease: 1, solvent: 1, rubber: 1 },                              out: { resin: 1, grease: 1 },  mechanic: "both", vdf_ms: 300_000, pow_ms: 110_000 },

  // ── more gates ────────────────────────────────────────────────────────────
  { id: "assembler",        label: "Machine II",       cat: "gate", level: "machine_1", inp: { circuit: 2, bearing: 2 }, out: { assembler: 1 },        mechanic: "both", vdf_ms: 114_000, pow_ms: 76_000 },
  { id: "reaction_chamber", label: "Reaction Chamber", cat: "gate", level: "machine_2", inp: { circuit: 2, grease: 2 }, out: { reaction_chamber: 1 }, mechanic: "both", vdf_ms: 100_000, pow_ms: 70_000 },

  // ── t4 (requires Machine II) ──────────────────────────────────────────────
  { id: "engine",        label: "Engine",          cat: "t4", level: "machine_2",                                inp: { pistons: 1, gear: 2, circuit: 2, canvas: 1 },                          out: { engine: 1 }, mechanic: "both", vdf_ms: 102_000, pow_ms: 68_000 },
  { id: "engine_tuned",  label: "Engine (tuned)",  cat: "t4", level: "machine_2", station: "reaction_chamber",  inp: { pistons: 1, gear: 2, circuit: 2, canvas: 1, catalyst: 1 },             out: { engine: 1 }, mechanic: "vdf",  vdf_ms: 80_000 },
  { id: "casing",        label: "Casing",          cat: "t4", level: "machine_2", inp: { steel: 3, canvas: 2, bearing: 2, coil: 1, wire: 2 },                                                  out: { casing: 1 }, mechanic: "vdf", vdf_ms: 250_000 },
  { id: "casing_coated", label: "Casing (coated)", cat: "t4", level: "machine_2", inp: { steel: 3, canvas: 2, bearing: 2, coil: 1, wire: 2, coating: 1 },                                      out: { casing: 1 }, mechanic: "vdf", vdf_ms: 180_000 },
  { id: "payload",       label: "Payload",         cat: "t4", level: "machine_2", inp: { panel: 3, circuit: 1, canvas: 1, wire: 1, grease: 1 },                                                out: { payload: 1 }, mechanic: "both", vdf_ms: 96_000, pow_ms: 64_000 },

  // ── t5 ────────────────────────────────────────────────────────────────────
  { id: "rocket",     label: "Rocket",       cat: "t5", level: "machine_2", inp: { engine: 1, casing: 1, payload: 1, resin: 2 },                              out: { rocket: 1 }, mechanic: "both", vdf_ms: 378_000, pow_ms: 252_000 },
  { id: "rocket_cat", label: "Rocket (cat)", cat: "t5", level: "machine_2", station: "reaction_chamber", inp: { engine: 1, casing: 1, payload: 1, resin: 2, catalyst: 1 }, out: { rocket: 1 }, mechanic: "vdf",  vdf_ms: 350_000 },
];

export const CAT_LABEL: Record<string, string> = {
  mine: "mine",
  farm: "farm",
  t1: "t1",
  t2: "t2",
  gate: "machines",
  t3: "t3",
  t4: "t4",
  t5: "t5",
};

export const RM: Record<string, Recipe> = Object.fromEntries(
  RECIPES.map((r) => [r.id, r]),
);

export const LEVEL_ORDER: Record<string, number> = {
  none: 0,
  machine_1: 1,
  machine_2: 2,
};

// ── BACKEND NAME MAPPING ──────────────────────────────────────────────────────
// The plugin (plugins/episode-1) declares classes and actions in PascalCase.
// We keep the spec's snake_case ids in this file to stay diff-clean against the
// upstream Bitcraft simulator, and translate at the boundary.

export const OBJ_TO_CLASS: Record<string, string> = {
  iron: "Iron",
  copper: "Copper",
  oil: "Oil",
  sulfur: "Sulfur",
  water: "Water",
  cane: "Cane",
  hemp: "Hemp",
  ingot: "Ingot",
  plate: "Plate",
  pulp: "Pulp",
  fiber: "Fiber",
  acid: "Acid",
  tar: "Tar",
  fuel: "Fuel",
  gas: "Gas",
  slag: "Slag",
  flux: "Flux",
  sludge: "Sludge",
  mold: "Mold",
  catalyst: "Catalyst",
  binder: "Binder",
  lye: "Lye",
  drill_bit: "DrillBit",
  soldering_iron: "SolderingIron",
  pressure_valve: "PressureValve",
  steel: "Steel",
  wire: "Wire",
  cloth: "Cloth",
  board: "Board",
  wax: "Wax",
  grease: "Grease",
  solvent: "Solvent",
  coating: "Coating",
  rubber: "Rubber",
  extract: "Extract",
  gear: "Gear",
  coil: "Coil",
  bearing: "Bearing",
  circuit: "Circuit",
  canvas: "Canvas",
  panel: "Panel",
  pistons: "Pistons",
  resin: "Resin",
  engine: "Engine",
  casing: "Casing",
  payload: "Payload",
  rocket: "Rocket",
  basic_asm: "MachineI",
  assembler: "MachineII",
  blast_furnace: "BlastFurnace",
  circuit_fab: "CircuitFab",
  cracking_unit: "CrackingUnit",
  reaction_chamber: "ReactionChamber",
};

export const CLASS_TO_OBJ: Record<string, string> = Object.fromEntries(
  Object.entries(OBJ_TO_CLASS).map(([k, v]) => [v, k]),
);

export const RECIPE_TO_ACTION: Record<string, string> = {
  // mines
  iron: "MineIron",
  copper: "MineCopper",
  oil: "MineOil",
  sulfur: "MineSulfur",
  // farms
  water: "FarmWater",
  cane: "FarmCane",
  hemp: "FarmHemp",
  // t1
  ingot: "CraftIngot",
  ingot_flux: "CraftIngotFlux",
  ingot_drilled: "CraftIngotDrilled",
  plate: "CraftPlate",
  pulp: "CraftPulp",
  fiber: "CraftFiber",
  acid: "CraftAcid",
  acid_flash: "CraftAcidFlash",
  acid_crude: "CraftAcidCrude",
  refinery: "CraftRefinery",
  refinery_flash: "CraftRefineryFlash",
  refinery_crude: "CraftRefineryCrude",
  refinery_cracked: "CraftRefineryCracked",
  flux_recipe: "CraftFlux",
  // t2
  steel: "CraftSteel",
  steel_blast: "CraftSteelBlast",
  mold: "CraftMold",
  gear_cast: "CraftGearCast",
  wire: "CraftWire",
  cloth: "CraftCloth",
  board: "CraftBoard",
  wax: "CraftWax",
  grease: "CraftGrease",
  solvent: "CraftSolvent",
  solvent_lye: "CraftSolventLye",
  coating: "CraftCoating",
  rubber: "CraftRubber",
  rubber_flash: "CraftRubberFlash",
  rubber_crude: "CraftRubberCrude",
  extract: "CraftExtract",
  extract_ctrl: "CraftExtractCtrl",
  catalyst_recipe: "CraftCatalyst",
  binder: "CraftBinder",
  gear: "CraftGear",
  drill_bit_recipe: "CraftDrillBit",
  soldering_iron_recipe: "CraftSolderingIron",
  pressure_valve_recipe: "CraftPressureValve",
  coil: "CraftCoil",
  // gates
  basic_asm: "CraftMachineI",
  blast_furnace: "CraftBlastFurnace",
  circuit_fab: "CraftCircuitFab",
  cracking_unit: "CraftCrackingUnit",
  assembler: "CraftMachineII",
  reaction_chamber: "CraftReactionChamber",
  // t3
  bearing: "CraftBearing",
  circuit: "CraftCircuit",
  circuit_soldered: "CraftCircuitSoldered",
  circuit_fab_recipe: "CraftCircuitFabbed",
  circuit_flash: "CraftCircuitFlash",
  circuit_crude: "CraftCircuitCrude",
  canvas: "CraftCanvas",
  canvas_lye: "CraftCanvasLye",
  canvas_flash: "CraftCanvasFlash",
  canvas_crude: "CraftCanvasCrude",
  panel: "CraftPanel",
  pistons: "CraftPistons",
  resin: "CraftResin",
  resin_pressurized: "CraftResinPressurized",
  resin_stable: "CraftResinStable",
  resin_flash: "CraftResinFlash",
  resin_crude: "CraftResinCrude",
  // t4
  engine: "CraftEngine",
  engine_tuned: "CraftEngineTuned",
  casing: "CraftCasing",
  casing_coated: "CraftCasingCoated",
  payload: "CraftPayload",
  // t5
  rocket: "CraftRocket",
  rocket_cat: "CraftRocketCat",
};

// ── BAR COLORS ────────────────────────────────────────────────────────────────
// Used by SegmentedBar and jobPhase.

export const BAR_GREY = "#bbb";
export const BAR_BLUE = "#7BB8E8";
export const BAR_AMBER = "#E8A84A";
