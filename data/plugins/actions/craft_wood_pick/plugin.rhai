fn CraftWoodPick() {
    let _wood = input("wood", "Wood");
    let _stick = input("stick", "Stick");
    let pick = output("wood_pick", "WoodPick");
    set(pick, "blueprint", "WoodPick");
    set_int(pick, "durability", 100);
}
