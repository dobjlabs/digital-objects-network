fn use_pick(pick, vdf_iters) {
    gt(pick, "durability", 0);
    let dur = decrement(pick, "durability");
    sum_of(pick, "durability", dur, 1);
    update(pick, "durability", dur);
    let key = random_key(pick);
    update(pick, "key", key);
    let work = vdf(vdf_iters, obj_raw(pick));
    update(pick, "work", work);
}

fn UseWoodPick() {
    let pick = mutate("wood_pick", "WoodPick");
    use_pick(pick, 10);
}
