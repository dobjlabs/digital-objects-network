fn CraftWoodPick() {
    let _wood = input("wood", "Wood");
    let _stick = input("stick", "Stick");
    let pick = output("wood_pick", "WoodPick");
    set(pick, "blueprint", "WoodPick");
    set_int(pick, "durability", 100);
}

fn use_pick(pick, vdf_iters) {
    let dur = get_int(pick, "durability");
    gt(pick, "durability", 0);
    dur -= 1;
    sum_of(pick, "durability", dur, 1);
    update_int(pick, "durability", dur);
    let key = random_key(pick);
    update(pick, "key", key);
    let work = vdf(vdf_iters, obj_raw(pick));
    update(pick, "work", work);
}

fn UseWoodPick() {
    let pick = mutate("wood_pick", "WoodPick");
    use_pick(pick, 10);
}
