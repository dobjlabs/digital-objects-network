fn CraftWood() {
    let _log = input("log", "Log");
    let wood = output("wood", "Wood");
    set(wood, "blueprint", "Wood");
    let key = pow_grind(wood, 9007199254740992);
    update(wood, "key", key);
    lt_eq_u256(wood, 9007199254740992);
}
