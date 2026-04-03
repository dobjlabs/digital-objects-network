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

fn MineStoneWithWoodPick() {
    depends("pick", "UseWoodPick");
    let stone = output("stone", "Stone");
    set(stone, "blueprint", "Stone");
}

fn CraftStonePick() {
    let _stone = input("stone", "Stone");
    let _stick = input("stick", "Stick");
    let pick = output("stone_pick", "StonePick");
    set(pick, "blueprint", "StonePick");
    set_int(pick, "durability", 200);
}

fn UseStonePick() {
    let pick = mutate("stone_pick", "StonePick");
    use_pick(pick, 5);
}

fn MineStoneWithStonePick() {
    depends("pick", "UseStonePick");
    let stone = output("stone", "Stone");
    set(stone, "blueprint", "Stone");
}
