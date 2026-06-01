// episode-1: full progression from raw mining/farming to a finished rocket.
//
// Every recipe is one fn here. Helpers below encode the four proof shapes:
//   pow      — pow_obj_grind + intro_lt_eq_u256 on the canonical output
//   vdf      — intro_vdf with a per-recipe iter count
//   both     — pow + vdf
//   none     — no proof, only consume/produce
//
// VDF iters are scaled from the spec's vdf_ms by /5000 (round to int, min 1)
// so the on-chain timing stays in roughly the same shape as the simulator.
//
// PoW difficulty scales with the spec's pow_ms: the top-limb target is
// 2^53 * 60000 / pow_ms, so pow_ms=60000 anchors at the existing
// craft-basics::CraftWood baseline (2^53, ~2^11 expected hashes), pow_ms=500
// gives ~2^60 (very easy), and pow_ms=252000 gives ~2^51 (~2^13 hashes).

// ── helpers ──────────────────────────────────────────────────────────────────

fn pow_proof(action, obj, pow_ms) {
    // Reorder the multiplication so the intermediate fits in i64:
    // (2^53 / pow_ms) * 60000 instead of (2^53 * 60000) / pow_ms.
    let target_n = 9007199254740992 / pow_ms * 60000;
    let target = action.top_limb_u256(target_n);
    var key = action.pow_obj_grind(obj, target);
    obj.update("key", key);
    action.intro_lt_eq_u256(obj, target);
}

fn vdf_proof(action, obj, iters) {
    var work = action.intro_vdf(iters, obj);
    obj.update("work", work);
}

// One use of a durability-bearing tool. The Gt(0) constraint fails when the
// tool is exhausted, matching the simulator's "remove on hit zero" behavior.
fn wear(action, tool) {
    action.st_gt(tool.durability, 0);
    var durability = unsafe { tool.durability - 1 };
    action.st_sum_of(tool.durability, durability, 1);
    tool.update("durability", durability);
    var key = action.random();
    tool.update("key", key);
}

// ── tool wear (hidden top-level actions, called via subaction) ───────────────
// Each is its own action so its `var key`/`var durability` live in their own
// scope and don't collide with the calling recipe's `var key` (e.g. from a
// pow_proof on the recipe's output).

fn UseDrillBit(action) {
    var tool = action.mutate("DrillBit");
    wear(action, tool);
}

fn UseSolderingIron(action) {
    var tool = action.mutate("SolderingIron");
    wear(action, tool);
}

fn UsePressureValve(action) {
    var tool = action.mutate("PressureValve");
    wear(action, tool);
}

// ── level/station possession (hidden, called via subaction) ──────────────────
// Mutating a machine or station with a fresh random key proves possession
// without changing semantic state. One hidden action per class so each call
// site has its own var scope.

fn TouchMachineI(action) {
    var m = action.mutate("MachineI");
    var key = action.random();
    m.update("key", key);
}

fn TouchMachineII(action) {
    var m = action.mutate("MachineII");
    var key = action.random();
    m.update("key", key);
}

fn TouchBlastFurnace(action) {
    var s = action.mutate("BlastFurnace");
    var key = action.random();
    s.update("key", key);
}

fn TouchCircuitFab(action) {
    var s = action.mutate("CircuitFab");
    var key = action.random();
    s.update("key", key);
}

fn TouchCrackingUnit(action) {
    var s = action.mutate("CrackingUnit");
    var key = action.random();
    s.update("key", key);
}

fn TouchReactionChamber(action) {
    var s = action.mutate("ReactionChamber");
    var key = action.random();
    s.update("key", key);
}

// ── mines (PoW) ──────────────────────────────────────────────────────────────

fn MineIron(action) {
    var iron = action.output("Iron", []);
    pow_proof(action, iron, 1000);
}

fn MineCopper(action) {
    var copper = action.output("Copper", []);
    pow_proof(action, copper, 500);
}

fn MineOil(action) {
    var oil = action.output("Oil", []);
    pow_proof(action, oil, 3000);
}

fn MineSulfur(action) {
    var sulfur = action.output("Sulfur", []);
    vdf_proof(action, sulfur, 2);
}

// ── farms (VDF) ──────────────────────────────────────────────────────────────

fn FarmWater(action) {
    var water = action.output("Water", []);
    vdf_proof(action, water, 2);
}

fn FarmCane(action) {
    var cane = action.output("Cane", []);
    vdf_proof(action, cane, 2);
}

fn FarmHemp(action) {
    var hemp = action.output("Hemp", []);
    vdf_proof(action, hemp, 2);
}

// ── t1 ───────────────────────────────────────────────────────────────────────

fn CraftIngot(action) {
    var iron = action.input("Iron");
    var ingot = action.output("Ingot", []);
    vdf_proof(action, ingot, 11);
}

fn CraftIngotFlux(action) {
    var iron = action.input("Iron");
    var flux = action.input("Flux");
    var ingot_a = action.output("Ingot", []);
    var ingot_b = action.output("Ingot", []);
    vdf_proof(action, ingot_a, 5);
}

fn CraftIngotDrilled(action) {
    var tool = action.subaction("UseDrillBit");
    var iron = action.input("Iron");
    var ingot_a = action.output("Ingot", []);
    var ingot_b = action.output("Ingot", []);
    vdf_proof(action, ingot_a, 9);
}

fn CraftPlate(action) {
    var copper = action.input("Copper");
    var plate = action.output("Plate", []);
    vdf_proof(action, plate, 11);
}

fn CraftPulp(action) {
    var cane = action.input("Cane");
    var pulp_a = action.output("Pulp", []);
    var pulp_b = action.output("Pulp", []);
    var pulp_c = action.output("Pulp", []);
    vdf_proof(action, pulp_a, 5);
}

fn CraftFiber(action) {
    var hemp = action.input("Hemp");
    var fiber_a = action.output("Fiber", []);
    var fiber_b = action.output("Fiber", []);
    var fiber_c = action.output("Fiber", []);
    vdf_proof(action, fiber_a, 6);
}

fn CraftAcid(action) {
    var sulfur = action.input("Sulfur");
    var water = action.input("Water");
    var acid_a = action.output("Acid", []);
    var acid_b = action.output("Acid", []);
    pow_proof(action, acid_a, 100000);
}

fn CraftAcidFlash(action) {
    var sulfur = action.input("Sulfur");
    var water = action.input("Water");
    var acid = action.output("Acid", []);
    pow_proof(action, acid, 50000);
}

fn CraftAcidCrude(action) {
    var sulfur = action.input("Sulfur");
    var water = action.input("Water");
    var acid_a = action.output("Acid", []);
    var acid_b = action.output("Acid", []);
    var acid_c = action.output("Acid", []);
    pow_proof(action, acid_a, 60000);
    vdf_proof(action, acid_a, 16);
}

fn CraftRefinery(action) {
    var oil = action.input("Oil");
    var water = action.input("Water");
    var tar_a = action.output("Tar", []);
    var tar_b = action.output("Tar", []);
    var tar_c = action.output("Tar", []);
    var fuel = action.output("Fuel", []);
    var gas = action.output("Gas", []);
    pow_proof(action, tar_a, 48000);
    vdf_proof(action, tar_a, 14);
}

fn CraftRefineryFlash(action) {
    var oil = action.input("Oil");
    var water = action.input("Water");
    var tar_a = action.output("Tar", []);
    var tar_b = action.output("Tar", []);
    var tar_c = action.output("Tar", []);
    var fuel = action.output("Fuel", []);
    var gas = action.output("Gas", []);
    pow_proof(action, tar_a, 60000);
}

fn CraftRefineryCrude(action) {
    var oil_a = action.input("Oil");
    var oil_b = action.input("Oil");
    var water = action.input("Water");
    var tar_a = action.output("Tar", []);
    var tar_b = action.output("Tar", []);
    var tar_c = action.output("Tar", []);
    var fuel = action.output("Fuel", []);
    var gas = action.output("Gas", []);
    var oil_recovered = action.output("Oil", []);
    pow_proof(action, tar_a, 60000);
    vdf_proof(action, tar_a, 22);
}

fn CraftRefineryCracked(action) {
    var cu = action.subaction("TouchCrackingUnit");
    var oil = action.input("Oil");
    var water = action.input("Water");
    var tar_a = action.output("Tar", []);
    var tar_b = action.output("Tar", []);
    var tar_c = action.output("Tar", []);
    var tar_d = action.output("Tar", []);
    var tar_e = action.output("Tar", []);
    var fuel_a = action.output("Fuel", []);
    var fuel_b = action.output("Fuel", []);
    var fuel_c = action.output("Fuel", []);
    var gas_a = action.output("Gas", []);
    var gas_b = action.output("Gas", []);
    pow_proof(action, tar_a, 48000);
    vdf_proof(action, tar_a, 14);
}

fn CraftFlux(action) {
    var slag_a = action.input("Slag");
    var slag_b = action.input("Slag");
    var water = action.input("Water");
    var flux_a = action.output("Flux", []);
    var flux_b = action.output("Flux", []);
    vdf_proof(action, flux_a, 3);
}

// ── t2 ───────────────────────────────────────────────────────────────────────

fn CraftSteel(action) {
    var ingot_a = action.input("Ingot");
    var ingot_b = action.input("Ingot");
    var ingot_c = action.input("Ingot");
    var steel_a = action.output("Steel", []);
    var steel_b = action.output("Steel", []);
    vdf_proof(action, steel_a, 24);
}

fn CraftSteelBlast(action) {
    var bf = action.subaction("TouchBlastFurnace");
    var ingot_a = action.input("Ingot");
    var ingot_b = action.input("Ingot");
    var ingot_c = action.input("Ingot");
    var steel_a = action.output("Steel", []);
    var steel_b = action.output("Steel", []);
    var slag = action.output("Slag", []);
    vdf_proof(action, steel_a, 14);
}

fn CraftMold(action) {
    var slag_a = action.input("Slag");
    var slag_b = action.input("Slag");
    var tar = action.input("Tar");
    var mold_a = action.output("Mold", []);
    var mold_b = action.output("Mold", []);
    vdf_proof(action, mold_a, 4);
}

fn CraftGearCast(action) {
    var bf = action.subaction("TouchBlastFurnace");
    var steel = action.input("Steel");
    var mold = action.input("Mold");
    var gear_a = action.output("Gear", []);
    var gear_b = action.output("Gear", []);
    var gear_c = action.output("Gear", []);
    var gear_d = action.output("Gear", []);
    var gear_e = action.output("Gear", []);
    vdf_proof(action, gear_a, 6);
}

fn CraftWire(action) {
    var plate = action.input("Plate");
    var wire_a = action.output("Wire", []);
    var wire_b = action.output("Wire", []);
    var wire_c = action.output("Wire", []);
    vdf_proof(action, wire_a, 5);
}

fn CraftCloth(action) {
    var fiber_a = action.input("Fiber");
    var fiber_b = action.input("Fiber");
    var cloth = action.output("Cloth", []);
    vdf_proof(action, cloth, 10);
}

fn CraftBoard(action) {
    var pulp_a = action.input("Pulp");
    var pulp_b = action.input("Pulp");
    var pulp_c = action.input("Pulp");
    var water = action.input("Water");
    var board_a = action.output("Board", []);
    var board_b = action.output("Board", []);
    var lye = action.output("Lye", []);
    vdf_proof(action, board_a, 15);
}

fn CraftWax(action) {
    var tar_a = action.input("Tar");
    var tar_b = action.input("Tar");
    var wax = action.output("Wax", []);
    vdf_proof(action, wax, 8);
}

fn CraftGrease(action) {
    var tar_a = action.input("Tar");
    var tar_b = action.input("Tar");
    var grease = action.output("Grease", []);
    vdf_proof(action, grease, 13);
}

fn CraftSolvent(action) {
    var fuel = action.input("Fuel");
    var acid = action.input("Acid");
    var solvent_a = action.output("Solvent", []);
    var solvent_b = action.output("Solvent", []);
    pow_proof(action, solvent_a, 76000);
}

fn CraftSolventLye(action) {
    var lye_a = action.input("Lye");
    var lye_b = action.input("Lye");
    var fuel = action.input("Fuel");
    var water = action.input("Water");
    var solvent_a = action.output("Solvent", []);
    var solvent_b = action.output("Solvent", []);
    vdf_proof(action, solvent_a, 13);
}

fn CraftCoating(action) {
    var fuel_a = action.input("Fuel");
    var fuel_b = action.input("Fuel");
    var wax = action.input("Wax");
    var coating_a = action.output("Coating", []);
    var coating_b = action.output("Coating", []);
    vdf_proof(action, coating_a, 5);
}

fn CraftRubber(action) {
    var gas_a = action.input("Gas");
    var gas_b = action.input("Gas");
    var gas_c = action.input("Gas");
    var rubber = action.output("Rubber", []);
    pow_proof(action, rubber, 96000);
}

fn CraftRubberFlash(action) {
    var gas_a = action.input("Gas");
    var gas_b = action.input("Gas");
    var gas_c = action.input("Gas");
    var rubber = action.output("Rubber", []);
    pow_proof(action, rubber, 48000);
}

fn CraftRubberCrude(action) {
    var gas_a = action.input("Gas");
    var gas_b = action.input("Gas");
    var gas_c = action.input("Gas");
    var rubber = action.output("Rubber", []);
    var gas_recovered = action.output("Gas", []);
    pow_proof(action, rubber, 60000);
    vdf_proof(action, rubber, 16);
}

fn CraftExtract(action) {
    var acid_a = action.input("Acid");
    var acid_b = action.input("Acid");
    var extract = action.output("Extract", []);
    var sludge = action.output("Sludge", []);
    pow_proof(action, extract, 120000);
}

fn CraftExtractCtrl(action) {
    var rc = action.subaction("TouchReactionChamber");
    var acid_a = action.input("Acid");
    var acid_b = action.input("Acid");
    var extract = action.output("Extract", []);
    var sludge_a = action.output("Sludge", []);
    var sludge_b = action.output("Sludge", []);
    pow_proof(action, extract, 25000);
    vdf_proof(action, extract, 12);
}

fn CraftCatalyst(action) {
    var sludge_a = action.input("Sludge");
    var sludge_b = action.input("Sludge");
    var sludge_c = action.input("Sludge");
    var wire = action.input("Wire");
    var catalyst = action.output("Catalyst", []);
    vdf_proof(action, catalyst, 2);
}

fn CraftBinder(action) {
    var sludge_a = action.input("Sludge");
    var sludge_b = action.input("Sludge");
    var sludge_c = action.input("Sludge");
    var solvent = action.input("Solvent");
    var binder = action.output("Binder", []);
    vdf_proof(action, binder, 4);
}

fn CraftGear(action) {
    var steel_a = action.input("Steel");
    var steel_b = action.input("Steel");
    var gear_a = action.output("Gear", []);
    var gear_b = action.output("Gear", []);
    var gear_c = action.output("Gear", []);
    vdf_proof(action, gear_a, 7);
}

fn CraftDrillBit(action) {
    var iron = action.input("Iron");
    var gear = action.input("Gear");
    var bit = action.output("DrillBit", [
        ["durability", 5]
    ]);
    vdf_proof(action, bit, 4);
}

fn CraftSolderingIron(action) {
    var wire_a = action.input("Wire");
    var wire_b = action.input("Wire");
    var acid = action.input("Acid");
    var iron = action.output("SolderingIron", [
        ["durability", 5]
    ]);
    vdf_proof(action, iron, 3);
}

fn CraftPressureValve(action) {
    var oil_a = action.input("Oil");
    var oil_b = action.input("Oil");
    var gear = action.input("Gear");
    var valve = action.output("PressureValve", [
        ["durability", 3]
    ]);
    vdf_proof(action, valve, 4);
}

fn CraftCoil(action) {
    var wire_a = action.input("Wire");
    var wire_b = action.input("Wire");
    var wire_c = action.input("Wire");
    var coil = action.output("Coil", []);
    vdf_proof(action, coil, 12);
}

// ── machines + stations (gates) ──────────────────────────────────────────────

fn CraftMachineI(action) {
    var steel_a = action.input("Steel");
    var steel_b = action.input("Steel");
    var steel_c = action.input("Steel");
    var steel_d = action.input("Steel");
    var gear_a = action.input("Gear");
    var gear_b = action.input("Gear");
    var gear_c = action.input("Gear");
    var coil_a = action.input("Coil");
    var coil_b = action.input("Coil");
    var machine = action.output("MachineI", [
        ["level", 1]
    ]);
    pow_proof(action, machine, 44000);
    vdf_proof(action, machine, 13);
}

fn CraftBlastFurnace(action) {
    var m1 = action.subaction("TouchMachineI");
    var steel_a = action.input("Steel");
    var steel_b = action.input("Steel");
    var steel_c = action.input("Steel");
    var gear_a = action.input("Gear");
    var gear_b = action.input("Gear");
    var coil = action.input("Coil");
    var acid_a = action.input("Acid");
    var acid_b = action.input("Acid");
    var bf = action.output("BlastFurnace", []);
    pow_proof(action, bf, 40000);
    vdf_proof(action, bf, 16);
}

fn CraftCircuitFab(action) {
    var m1 = action.subaction("TouchMachineI");
    var bearing_a = action.input("Bearing");
    var bearing_b = action.input("Bearing");
    var coil_a = action.input("Coil");
    var coil_b = action.input("Coil");
    var steel_a = action.input("Steel");
    var steel_b = action.input("Steel");
    var steel_c = action.input("Steel");
    var grease_a = action.input("Grease");
    var grease_b = action.input("Grease");
    var fab = action.output("CircuitFab", []);
    pow_proof(action, fab, 60000);
    vdf_proof(action, fab, 18);
}

fn CraftCrackingUnit(action) {
    var m1 = action.subaction("TouchMachineI");
    var acid_a = action.input("Acid");
    var acid_b = action.input("Acid");
    var acid_c = action.input("Acid");
    var acid_d = action.input("Acid");
    var bearing_a = action.input("Bearing");
    var bearing_b = action.input("Bearing");
    var bearing_c = action.input("Bearing");
    var grease_a = action.input("Grease");
    var grease_b = action.input("Grease");
    var grease_c = action.input("Grease");
    var unit = action.output("CrackingUnit", []);
    pow_proof(action, unit, 60000);
    vdf_proof(action, unit, 16);
}

fn CraftMachineII(action) {
    var m1 = action.subaction("TouchMachineI");
    var circuit_a = action.input("Circuit");
    var circuit_b = action.input("Circuit");
    var bearing_a = action.input("Bearing");
    var bearing_b = action.input("Bearing");
    var machine = action.output("MachineII", [
        ["level", 2]
    ]);
    pow_proof(action, machine, 76000);
    vdf_proof(action, machine, 23);
}

fn CraftReactionChamber(action) {
    var m2 = action.subaction("TouchMachineII");
    var circuit_a = action.input("Circuit");
    var circuit_b = action.input("Circuit");
    var grease_a = action.input("Grease");
    var grease_b = action.input("Grease");
    var chamber = action.output("ReactionChamber", []);
    pow_proof(action, chamber, 70000);
    vdf_proof(action, chamber, 20);
}

// ── t3 (require MachineI) ────────────────────────────────────────────────────

fn CraftBearing(action) {
    var m1 = action.subaction("TouchMachineI");
    var steel = action.input("Steel");
    var grease_a = action.input("Grease");
    var grease_b = action.input("Grease");
    var bearing_a = action.output("Bearing", []);
    var bearing_b = action.output("Bearing", []);
    vdf_proof(action, bearing_a, 8);
}

fn CraftCircuit(action) {
    var m1 = action.subaction("TouchMachineI");
    var wire_a = action.input("Wire");
    var wire_b = action.input("Wire");
    var steel = action.input("Steel");
    var circuit = action.output("Circuit", []);
    pow_proof(action, circuit, 84000);
    vdf_proof(action, circuit, 25);
}

fn CraftCircuitSoldered(action) {
    var m1 = action.subaction("TouchMachineI");
    var tool = action.subaction("UseSolderingIron");
    var wire_a = action.input("Wire");
    var wire_b = action.input("Wire");
    var steel = action.input("Steel");
    var circuit = action.output("Circuit", []);
    vdf_proof(action, circuit, 25);
}

fn CraftCircuitFabbed(action) {
    var m1 = action.subaction("TouchMachineI");
    var cf = action.subaction("TouchCircuitFab");
    var wire_a = action.input("Wire");
    var wire_b = action.input("Wire");
    var wire_c = action.input("Wire");
    var wire_d = action.input("Wire");
    var steel = action.input("Steel");
    var circuit_a = action.output("Circuit", []);
    var circuit_b = action.output("Circuit", []);
    vdf_proof(action, circuit_a, 20);
}

fn CraftCircuitFlash(action) {
    var m1 = action.subaction("TouchMachineI");
    var wire_a = action.input("Wire");
    var wire_b = action.input("Wire");
    var steel = action.input("Steel");
    var circuit = action.output("Circuit", []);
    pow_proof(action, circuit, 110000);
}

fn CraftCircuitCrude(action) {
    var m1 = action.subaction("TouchMachineI");
    var wire_a = action.input("Wire");
    var wire_b = action.input("Wire");
    var steel = action.input("Steel");
    var circuit = action.output("Circuit", []);
    var wire_recovered = action.output("Wire", []);
    pow_proof(action, circuit, 50000);
    vdf_proof(action, circuit, 38);
}

fn CraftCanvas(action) {
    var m1 = action.subaction("TouchMachineI");
    var cloth_a = action.input("Cloth");
    var cloth_b = action.input("Cloth");
    var fiber = action.input("Fiber");
    var wax = action.input("Wax");
    var canvas = action.output("Canvas", []);
    vdf_proof(action, canvas, 20);
}

fn CraftCanvasLye(action) {
    var m1 = action.subaction("TouchMachineI");
    var cloth_a = action.input("Cloth");
    var cloth_b = action.input("Cloth");
    var fiber = action.input("Fiber");
    var lye = action.input("Lye");
    var canvas = action.output("Canvas", []);
    vdf_proof(action, canvas, 11);
}

fn CraftCanvasFlash(action) {
    var m1 = action.subaction("TouchMachineI");
    var cloth_a = action.input("Cloth");
    var cloth_b = action.input("Cloth");
    var fiber = action.input("Fiber");
    var wax = action.input("Wax");
    var canvas = action.output("Canvas", []);
    pow_proof(action, canvas, 75000);
}

fn CraftCanvasCrude(action) {
    var m1 = action.subaction("TouchMachineI");
    var cloth_a = action.input("Cloth");
    var cloth_b = action.input("Cloth");
    var fiber = action.input("Fiber");
    var wax = action.input("Wax");
    var canvas = action.output("Canvas", []);
    var cloth_recovered = action.output("Cloth", []);
    pow_proof(action, canvas, 30000);
    vdf_proof(action, canvas, 24);
}

fn CraftPanel(action) {
    var m1 = action.subaction("TouchMachineI");
    var board = action.input("Board");
    var extract = action.input("Extract");
    var panel_a = action.output("Panel", []);
    var panel_b = action.output("Panel", []);
}

fn CraftPistons(action) {
    var m1 = action.subaction("TouchMachineI");
    var bearing_a = action.input("Bearing");
    var bearing_b = action.input("Bearing");
    var coil_a = action.input("Coil");
    var coil_b = action.input("Coil");
    var grease = action.input("Grease");
    var pistons = action.output("Pistons", []);
    pow_proof(action, pistons, 62000);
    vdf_proof(action, pistons, 22);
}

fn CraftResin(action) {
    var m1 = action.subaction("TouchMachineI");
    var grease = action.input("Grease");
    var solvent = action.input("Solvent");
    var rubber = action.input("Rubber");
    var resin = action.output("Resin", []);
    pow_proof(action, resin, 132000);
    vdf_proof(action, resin, 40);
}

fn CraftResinPressurized(action) {
    var m1 = action.subaction("TouchMachineI");
    var tool = action.subaction("UsePressureValve");
    var grease = action.input("Grease");
    var solvent = action.input("Solvent");
    var rubber = action.input("Rubber");
    var resin = action.output("Resin", []);
    vdf_proof(action, resin, 40);
}

fn CraftResinStable(action) {
    var m1 = action.subaction("TouchMachineI");
    var rc = action.subaction("TouchReactionChamber");
    var grease = action.input("Grease");
    var solvent = action.input("Solvent");
    var rubber = action.input("Rubber");
    var binder = action.input("Binder");
    var resin_a = action.output("Resin", []);
    var resin_b = action.output("Resin", []);
    vdf_proof(action, resin_a, 46);
}

fn CraftResinFlash(action) {
    var m1 = action.subaction("TouchMachineI");
    var grease = action.input("Grease");
    var solvent = action.input("Solvent");
    var rubber = action.input("Rubber");
    var resin = action.output("Resin", []);
    pow_proof(action, resin, 165000);
}

fn CraftResinCrude(action) {
    var m1 = action.subaction("TouchMachineI");
    var grease = action.input("Grease");
    var solvent = action.input("Solvent");
    var rubber = action.input("Rubber");
    var resin = action.output("Resin", []);
    var grease_recovered = action.output("Grease", []);
    pow_proof(action, resin, 110000);
    vdf_proof(action, resin, 60);
}

// ── t4 (require MachineII) ───────────────────────────────────────────────────

fn CraftEngine(action) {
    var m2 = action.subaction("TouchMachineII");
    var pistons = action.input("Pistons");
    var gear_a = action.input("Gear");
    var gear_b = action.input("Gear");
    var circuit_a = action.input("Circuit");
    var circuit_b = action.input("Circuit");
    var canvas = action.input("Canvas");
    var engine = action.output("Engine", []);
    pow_proof(action, engine, 68000);
    vdf_proof(action, engine, 20);
}

fn CraftEngineTuned(action) {
    var m2 = action.subaction("TouchMachineII");
    var rc = action.subaction("TouchReactionChamber");
    var pistons = action.input("Pistons");
    var gear_a = action.input("Gear");
    var gear_b = action.input("Gear");
    var circuit_a = action.input("Circuit");
    var circuit_b = action.input("Circuit");
    var canvas = action.input("Canvas");
    var catalyst = action.input("Catalyst");
    var engine = action.output("Engine", []);
    vdf_proof(action, engine, 16);
}

fn CraftCasing(action) {
    var m2 = action.subaction("TouchMachineII");
    var steel_a = action.input("Steel");
    var steel_b = action.input("Steel");
    var steel_c = action.input("Steel");
    var canvas_a = action.input("Canvas");
    var canvas_b = action.input("Canvas");
    var bearing_a = action.input("Bearing");
    var bearing_b = action.input("Bearing");
    var coil = action.input("Coil");
    var wire_a = action.input("Wire");
    var wire_b = action.input("Wire");
    var casing = action.output("Casing", []);
    vdf_proof(action, casing, 50);
}

fn CraftCasingCoated(action) {
    var m2 = action.subaction("TouchMachineII");
    var steel_a = action.input("Steel");
    var steel_b = action.input("Steel");
    var steel_c = action.input("Steel");
    var canvas_a = action.input("Canvas");
    var canvas_b = action.input("Canvas");
    var bearing_a = action.input("Bearing");
    var bearing_b = action.input("Bearing");
    var coil = action.input("Coil");
    var wire_a = action.input("Wire");
    var wire_b = action.input("Wire");
    var coating = action.input("Coating");
    var casing = action.output("Casing", []);
    vdf_proof(action, casing, 36);
}

fn CraftPayload(action) {
    var m2 = action.subaction("TouchMachineII");
    var panel_a = action.input("Panel");
    var panel_b = action.input("Panel");
    var panel_c = action.input("Panel");
    var circuit = action.input("Circuit");
    var canvas = action.input("Canvas");
    var wire = action.input("Wire");
    var grease = action.input("Grease");
    var payload = action.output("Payload", []);
    pow_proof(action, payload, 64000);
    vdf_proof(action, payload, 19);
}

// ── t5 (require MachineII) ───────────────────────────────────────────────────

fn CraftRocket(action) {
    var m2 = action.subaction("TouchMachineII");
    var engine = action.input("Engine");
    var casing = action.input("Casing");
    var payload = action.input("Payload");
    var resin_a = action.input("Resin");
    var resin_b = action.input("Resin");
    var rocket = action.output("Rocket", []);
    pow_proof(action, rocket, 252000);
    vdf_proof(action, rocket, 76);
}

fn CraftRocketCat(action) {
    var m2 = action.subaction("TouchMachineII");
    var rc = action.subaction("TouchReactionChamber");
    var engine = action.input("Engine");
    var casing = action.input("Casing");
    var payload = action.input("Payload");
    var resin_a = action.input("Resin");
    var resin_b = action.input("Resin");
    var catalyst = action.input("Catalyst");
    var rocket = action.output("Rocket", []);
    vdf_proof(action, rocket, 70);
}
