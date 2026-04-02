// craft_wood_pick.rhai — Combine wood and a stick to craft a wood pick.

let plugin = #{
    name: "craft-wood-pick",
    version: "0.1.0",
};

let imports = ["craft-wood", "craft-sticks"];

let dependencies = [];

let classes = [
    #{ name: "WoodPick", emoji: "⛏️", description: "A wood pick that can mine stone while durability remains." },
];

let action_defs = [
    #{ name: "CraftWoodPick", fn_name: "CraftWoodPick", emoji: "⛏️", description: "Combine wood and a stick to craft a wood pick.", cpu_cost: "10-20s", reads_block: false, hidden: false },
];

fn CraftWoodPick() {
    let _wood = input("wood", "Wood");
    let _stick = input("stick", "Stick");
    let pick = output("wood_pick", "WoodPick");
    set(pick, "blueprint", "WoodPick");
    set_int(pick, "durability", 100);
}
