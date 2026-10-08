// Compact example of the SDK features documented in README.md.
//
// Actions use literal lookup tables when authenticated object data selects a
// row. Player-selected choices remain separate actions because actions accept
// objects, not scalar parameters.

// ---- tables ---------------------------------------------------------------
// Tables are defined in functions because Rhai functions cannot access
// top-level constants. Rhai arrays and maps become pod2 Arrays and Dictionaries
// at Load. Each lookup emits one containment statement and binds one private
// wildcard.

fn ship_tiers() { [
    #{"step":   1, "extract_base":  10, "vdf":  3},
    #{"step":  10, "extract_base":  50, "vdf":  6},
    #{"step": 100, "extract_base": 250, "vdf":  9}
] }

fn sector_profiles() { [
    #{"category": 0, "slots": 1},
    #{"category": 1, "slots": 3},
    #{"category": 2, "slots": 5}
] }

// Container literals may contain only literals; `set_of` constructs nested sets.
fn body_types() { [
    #{"body_type": 1, "richness": 1, "primary_kind": "ore", "pool":  500, "vdf":  4,
      "kinds": set_of(["ore", "slag"])},
    #{"body_type": 2, "richness": 3, "primary_kind": "crystal", "pool": 2000, "vdf":  8,
      "kinds": set_of(["crystal", "dust", "shard"])},
    #{"body_type": 3, "richness": 7, "primary_kind": "ice", "pool": 9000, "vdf": 12,
      "kinds": set_of(["ice", "vapor"])}
] }

// Recipes use resource-kind strings as keys because the names do not form a
// dense integer index.
fn refine_recipes() { #{
    "ore":     #{"out": "ingot", "factor": 2},
    "crystal": #{"out": "lens",  "factor": 1},
    "ice":     #{"out": "water", "factor": 3}
} }

fn coordinate_catalog() { [
    #{"code": 101, "x":    793814733, "y": 968149119310, "z": 42019687806, "uses": 3, "floor":  50},
    #{"code": 102, "x": 327853439873, "y":    605502947, "z":         3102, "uses": 2, "floor": 100},
    #{"code": 103, "x":          105, "y":    465107091, "z":  86018776812, "uses": 1, "floor": 250}
] }

// ---- helpers --------------------------------------------------------------

// `Equal` accepts any pod2 value; the equivalent `Sum(x, 0, y)` form accepts
// only integers.
fn require_version(action, obj) {
    action.st_equal(obj.v, 1);
}

// Write the random key directly so repeated calls do not reuse a named wildcard.
fn rekey(action, obj) {
    obj.update("key", action.random());
}

// ---- ship -----------------------------------------------------------------

fn BuildShip(action) {
    var ship = action.output("ship");
    ship.set([
        ["v", 1],
        ["tier", 0],
        ["x", 0],
        ["y", 0],
        ["z", 0],
        ["ready_at", 0]
    ]);
    var work = action.intro_vdf(3, ship);
}

// Look up next-tier data using the authenticated tier. The row also provides
// the variable VDF iteration count.
fn UpgradeShip(action) {
    var ship = action.mutate("ship");
    require_version(action, ship);
    var next_tier = unsafe { ship.tier + 1 };
    action.st_sum(ship.tier, 1, next_tier);
    // Lookup failure rejects tiers beyond the literal table.
    var row = action.array_get(ship_tiers(), next_tier);
    ship.update("tier", next_tier);
    var work = action.intro_vdf(row.vdf, ship);
    rekey(action, ship);
}

// Tier selects the step; separate actions represent player-selected direction.
// Compute the witness in `unsafe` and constrain it with `st_sum`.
fn MoveShipX(action) {
    var ship = action.mutate("ship");
    require_version(action, ship);
    // `block_timestamp` comes from the grounding state, so this enforces only
    // a coarse cooldown.
    action.st_gt(state_header.block_timestamp, ship.ready_at);
    var row = action.array_get(ship_tiers(), ship.tier);
    var next_x = unsafe { ship.x + row.step };
    action.st_sum(ship.x, row.step, next_x);
    ship.update("x", next_x);
    ship.update("ready_at", state_header.block_timestamp);
    rekey(action, ship);
}

// ---- sector ---------------------------------------------------------------

// Copy authenticated ship coordinates directly into the sector and record the
// grounding state's block number.
fn ClaimSector(action) {
    var ship = action.mutate("ship");
    var sector = action.output("sector");
    require_version(action, ship);
    sector.set([
        ["v", 1],
        ["x", ship.x],
        ["y", ship.y],
        ["z", ship.z],
        ["profile", 0],
        ["slots_left", 0],
        ["next_slot", 0],
        ["claimed_at", state_header.block_number]
    ]);
    rekey(action, ship);
}

// Partition `stable_identifier` into bands because a 256-bit identity cannot
// be used as an array index. Apply each bound directly to the anchored field.
fn survey(action, sector, profile) {
    require_version(action, sector);
    action.st_equal(sector.profile, 0);
    // The band determines the profile during Load, so direct indexing avoids
    // an unnecessary statement and wildcard.
    let row = sector_profiles()[profile];
    sector.update("profile", profile);
    sector.update("slots_left", row.slots);
    rekey(action, sector);
}

// Adjacent bounds cover every identity without overlap.
fn SurveySectorSparse(action) {
    var sector = action.mutate("sector");
    let bound = action.top_limb_u256(6148914691236517205);
    action.intro_lt_eq_u256(sector.stable_identifier, bound);
    survey(action, sector, 1);
}

fn SurveySectorRich(action) {
    var sector = action.mutate("sector");
    let bound = action.top_limb_u256(6148914691236517206);
    action.intro_lt_eq_u256(bound, sector.stable_identifier);
    survey(action, sector, 2);
}

// Use `sector.profile` as the row index. Field reads in `set` capture the
// sector's pre-update values.
fn DetectSignal(action) {
    var ship = action.mutate("ship");
    var sector = action.mutate("sector");
    var signal = action.output("signal");
    require_version(action, ship);
    require_version(action, sector);
    action.st_equal(ship.x, sector.x);
    action.st_equal(ship.y, sector.y);
    action.st_equal(ship.z, sector.z);
    var row = action.array_get(sector_profiles(), sector.profile);
    var left = unsafe { sector.slots_left - 1 };
    action.st_sum(left, 1, sector.slots_left);
    action.st_gt_eq(left, 0);
    var next_slot = unsafe { sector.next_slot + 1 };
    action.st_sum(sector.next_slot, 1, next_slot);
    signal.set([
        ["v", 1],
        ["category", row.category],
        ["origin", sector.stable_identifier],
        ["slot", sector.next_slot]
    ]);
    sector.update("slots_left", left);
    sector.update("next_slot", next_slot);
    rekey(action, sector);
    rekey(action, ship);
}

// ---- body -----------------------------------------------------------------

// Declare inputs before the output so `set` can initialize every field once.
// `body.kinds` stores a nested set used by `ExtractResource`.
fn ScanBody(action) {
    var ship = action.mutate("ship");
    var signal = action.input("signal");
    var body = action.output("body");
    require_version(action, ship);
    require_version(action, signal);
    var row = action.array_get(body_types(), signal.category);
    body.set([
        ["v", 1],
        ["body_type", row.body_type],
        ["richness", row.richness],
        ["primary_kind", row.primary_kind],
        ["kinds", row.kinds],
        ["remaining", row.pool],
        ["origin", signal.stable_identifier],
        ["slot", signal.slot],
        ["next_coordinate", 0]
    ]);
    var work = action.intro_vdf(row.vdf, body);
    rekey(action, ship);
}

// Compute the yield as a witness and constrain it with `st_product`.
fn ExtractResource(action) {
    var ship = action.mutate("ship");
    var body = action.mutate("body");
    var resource = action.output("resource");
    require_version(action, ship);
    require_version(action, body);
    var tier = action.array_get(ship_tiers(), ship.tier);
    var amount = unsafe { tier.extract_base * body.richness };
    action.st_product(tier.extract_base, body.richness, amount);
    var left = unsafe { body.remaining - amount };
    action.st_sum(left, amount, body.remaining);
    action.st_gt_eq(left, 0);
    action.st_set_contains(body.kinds, body.primary_kind);
    resource.set([
        ["v", 1],
        ["kind", body.primary_kind],
        ["amount", amount]
    ]);
    body.update("remaining", left);
    rekey(action, body);
    rekey(action, ship);
}

// ---- coordinates ----------------------------------------------------------

// Store the body's counter as the catalog row, fixing the destination at mint.
fn ExtractCoordinate(action) {
    var body = action.mutate("body");
    var coordinate = action.output("coordinate");
    require_version(action, body);
    var next = unsafe { body.next_coordinate + 1 };
    action.st_sum(body.next_coordinate, 1, next);
    action.st_lt_eq(next, 3);
    coordinate.set([
        ["v", 1],
        ["row", body.next_coordinate],
        ["source_pool", body.remaining],
        ["revealed", 0],
        ["code", 0],
        ["x", 0],
        ["y", 0],
        ["z", 0],
        ["uses_left", 0]
    ]);
    body.update("next_coordinate", next);
    rekey(action, body);
}

// A variable-key lookup handles the entire catalog; `coordinate.row`
// authenticates the selected entry.
fn RevealCoordinate(action) {
    var coordinate = action.mutate("coordinate");
    require_version(action, coordinate);
    action.st_equal(coordinate.revealed, 0);
    var row = action.array_get(coordinate_catalog(), coordinate.row);
    action.st_gt(coordinate.source_pool, row.floor);
    coordinate.update("revealed", 1);
    coordinate.update("code", row.code);
    coordinate.update("x", row.x);
    coordinate.update("y", row.y);
    coordinate.update("z", row.z);
    coordinate.update("uses_left", row.uses);
    rekey(action, coordinate);
}

// Mutation and deletion are fixed by the action shape, so deleting the final
// use requires a separate action.
fn UseCoordinate(action) {
    var coordinate = action.mutate("coordinate");
    require_version(action, coordinate);
    action.st_equal(coordinate.revealed, 1);
    var left = unsafe { coordinate.uses_left - 1 };
    action.st_sum(left, 1, coordinate.uses_left);
    action.st_gt_eq(left, 0);
    coordinate.update("uses_left", left);
    rekey(action, coordinate);
}

// The sub-action returns the updated coordinate; the parent copies its
// destination into the ship.
fn WarpToCoordinate(action) {
    var coordinate = action.subaction("UseCoordinate");
    var ship = action.mutate("ship");
    require_version(action, ship);
    ship.update("x", coordinate.x);
    ship.update("y", coordinate.y);
    ship.update("z", coordinate.z);
    rekey(action, ship);
}

// ---- resources ------------------------------------------------------------

// Look up the recipe by `source.kind`; a string-keyed dictionary supports
// sparse resource names.
fn RefineResource(action) {
    var source = action.input("resource");
    var refined = action.output("resource");
    require_version(action, source);
    // Naming this literal with `var` does not create a wildcard.
    var recipes = refine_recipes();
    var row = action.dict_get(recipes, source.kind);
    var amount = unsafe { source.amount * row.factor };
    action.st_product(source.amount, row.factor, amount);
    refined.set([
        ["v", 1],
        ["kind", row.out],
        ["amount", amount]
    ]);
    // Reject zero output after reading the amount from the newly built object.
    action.st_gt(refined.amount, 0);
}

// Allow scrapping only when no recipe exists for `source.kind`.
fn ScrapResource(action) {
    var source = action.input("resource");
    require_version(action, source);
    var recipes = refine_recipes();
    action.st_dict_not_contains(recipes, source.kind);
}

// Fixed object slots require a separate action for each supported merge arity.
fn MergeResources(action) {
    var destination = action.mutate("resource");
    var source = action.input("resource");
    require_version(action, destination);
    require_version(action, source);
    action.st_equal(source.kind, destination.kind);
    action.st_gt_eq(source.amount, 0);
    var total = unsafe { destination.amount + source.amount };
    action.st_sum(destination.amount, source.amount, total);
    destination.update("amount", total);
    rekey(action, destination);
}
