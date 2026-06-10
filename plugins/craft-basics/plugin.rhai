fn FindLog(action) {
    var log = action.output("Log", []);
    var work = action.intro_vdf(3, log);
    log.update("work", work);
}

fn CraftWood(action) {
    var log = action.input("Log");
    var wood = action.output("Wood", []);
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
    var stick_a = action.output("Stick", []);
    var stick_b = action.output("Stick", []);
}

fn CraftWoodPick(action) {
    var wood = action.input("Wood");
    var stick = action.input("Stick");
    var pick = action.output("WoodPick", [
        ["durability", 100]
    ]);
}

fn use_pick(action, pick, vdf_iters) {
    action.st_gt(pick.durability, 0);
    var durability = unsafe { pick.durability - 1 };
    action.st_sum_of(pick.durability, durability, 1);
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
    var stone = action.output("Stone", []);
}

fn CraftStonePick(action) {
    var stone = action.input("Stone");
    var stick = action.input("Stick");
    var pick = action.output("StonePick", [
        ["durability", 200]
    ]);
}

fn UseStonePick(action) {
    var stone_pick = action.mutate("StonePick");
    use_pick(action, stone_pick, 5);
}

fn MineStoneWithStonePick(action) {
    var pick = action.subaction("UseStonePick");
    var stone = action.output("Stone", []);
}
