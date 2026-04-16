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
    var key = action.pow_obj_grind(wood, 9007199254740992);
    wood.update("key", key);
    action.intro_lt_eq_u256(wood, 9007199254740992);
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
