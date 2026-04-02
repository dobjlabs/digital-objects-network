// craft_stone_pick.rhai — Combine stone and a stick to craft a stronger stone pick.

let plugin = #{
    name: "craft-stone-pick",
    version: "0.1.0",
};

let imports = ["mine-stone-with-wood-pick", "craft-sticks"];

let dependencies = [];

let classes = [
    #{ name: "StonePick", emoji: "⛏️", description: "A sturdier pick with higher starting durability." },
];

let action_defs = [
    #{ name: "CraftStonePick", fn_name: "CraftStonePick", emoji: "⛏️", description: "Combine stone and a stick to craft a stronger stone pick.", cpu_cost: "10-20s", reads_block: false, hidden: false },
];

fn CraftStonePick() {
    let _stone = input("stone", "Stone");
    let _stick = input("stick", "Stick");
    let pick = output("stone_pick", "StonePick");
    set(pick, "blueprint", "StonePick");
    set_int(pick, "durability", 200);
}
