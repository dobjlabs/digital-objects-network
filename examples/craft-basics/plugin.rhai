fn FindLog(action) {
    var log = action.output("Log");
    var work = action.intro_vdf(3, log);
    log.update("work", work);
}

fn CraftWood(action) {
    var log = action.input("Log");
    var wood = action.output("Wood");
    // Difficulty target: top limb ≤ 2^53. Both pow_obj_grind and
    // intro_lt_eq_u256 consume the same u256, so the proved statement
    // matches what grinding produced.
    let target = action.top_limb_u256(9007199254740992);
    var key = action.pow_obj_grind(wood, target);
    wood.update("key", key);
    action.intro_lt_eq_u256(wood, target);
}

fn CraftSticks(action) {
    var wood = action.input("Wood");
    var stick_a = action.output("Stick");
    var stick_b = action.output("Stick");
}

fn CraftWoodPick(action) {
    var wood = action.input("Wood");
    var stick = action.input("Stick");
    var pick = action.output("WoodPick");
    pick.set([
        ["durability", 100]
    ]);
}

fn use_pick(action, pick, vdf_iters) {
    action.st_gt(pick.durability, 0);
    var durability = unsafe { pick.durability - 1 };
    action.st_sum(durability, 1, pick.durability);
    pick.update("durability", durability);
    var key = action.random();
    pick.update("key", key);
    var work = action.intro_vdf(vdf_iters, pick);
    pick.update("work", work);
}

fn UseWoodPick(action) {
    var wood_pick = action.mutate("WoodPick");
    use_pick(action, wood_pick, 10);
}

fn MineStoneWithWoodPick(action) {
    var pick = action.subaction("UseWoodPick");
    var stone = action.output("Stone");
}

fn CraftStonePick(action) {
    var stone = action.input("Stone");
    var stick = action.input("Stick");
    var pick = action.output("StonePick");
    pick.set([
        ["durability", 200]
    ]);
}

fn UseStonePick(action) {
    var stone_pick = action.mutate("StonePick");
    use_pick(action, stone_pick, 5);
}

fn MineStoneWithStonePick(action) {
    var pick = action.subaction("UseStonePick");
    var stone = action.output("Stone");
}

// ── rekeys ───────────────────────────────────────────────────────────────────
// Rotating `key` moves an object's commitment and its nullifier, so a holder
// who was sent an object can spend the sender's copy out from under them.
// Nothing else about the state changes, and the stable identifier carries
// across, so the object stays the same object under new custody.
//
// These are also this plugin's extension surface: another plugin's script can
// only reach the actions exposed here.

fn rekey(action, obj) {
    var key = action.random();
    obj.update("key", key);
}

fn RekeyLog(action) {
    var log = action.mutate("Log");
    rekey(action, log);
}

fn RekeyWood(action) {
    var wood = action.mutate("Wood");
    rekey(action, wood);
}

fn RekeyStick(action) {
    var stick = action.mutate("Stick");
    rekey(action, stick);
}

fn RekeyStone(action) {
    var stone = action.mutate("Stone");
    rekey(action, stone);
}

fn RekeyWoodPick(action) {
    var pick = action.mutate("WoodPick");
    rekey(action, pick);
}

fn RekeyStonePick(action) {
    var pick = action.mutate("StonePick");
    rekey(action, pick);
}
