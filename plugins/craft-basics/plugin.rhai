fn FindLog(action) {
    var log = action.output("Log");
    log.set([["blueprint", "Log"]]);
    var work = action.intro_vdf(3, log);
    log.update("work", work);
}

fn CraftWood(action) {
    var log = action.input("Log");
    var wood = action.output("Wood");
    wood.set([["blueprint", "Wood"]]);
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
    stick_a.set([["blueprint", "Stick"]]);
    stick_b.set([["blueprint", "Stick"]]);
}

fn CraftWoodPick(action) {
    var wood = action.input("Wood");
    var stick = action.input("Stick");
    var pick = action.output("WoodPick");
    pick.set([
        ["blueprint", "WoodPick"],
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
