// stone_tools.rhai — Stone-tier actions: mining stone, crafting & using stone picks.
//
// These actions share the Stone and StonePick classes, so they must live in one
// module to produce a single IsStone / IsStonePick predicate hash.

let plugin = #{
    name: "stone-tools",
    version: "0.1.0",
};

let imports = ["use-wood-pick", "craft-sticks"];

let dependencies = [
    #{
        dep_type: "Intro",
        pred: "Vdf(count, input, output)",
        hash: "b77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06",
    },
];

let classes = [
    #{ name: "Stone", emoji: "🪨", description: "Mined stone used to craft stronger tools." },
    #{ name: "StonePick", emoji: "⛏️", description: "A sturdier pick with higher starting durability." },
];

let action_defs = [
    #{ name: "MineStoneWithWoodPick", fn_name: "MineStoneWithWoodPick", emoji: "🪨", description: "Mine stone using a wood pick (consumes durability).", cpu_cost: "25-45s", reads_block: false, hidden: false },
    #{ name: "CraftStonePick", fn_name: "CraftStonePick", emoji: "⛏️", description: "Combine stone and a stick to craft a stronger stone pick.", cpu_cost: "10-20s", reads_block: false, hidden: false },
    #{ name: "UseStonePick", fn_name: "UseStonePick", emoji: "⛏️", description: "Internal durability/work update for stone pick usage.", cpu_cost: "5-20s", reads_block: false, hidden: true },
    #{ name: "MineStoneWithStonePick", fn_name: "MineStoneWithStonePick", emoji: "🪨", description: "Mine stone using a stone pick (consumes durability).", cpu_cost: "15-35s", reads_block: false, hidden: false },
];

// --- Action implementations ---

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

fn UseStonePick() {
    let pick = mutate("stone_pick", "StonePick");
    use_pick(pick, 5);
}

fn MineStoneWithStonePick() {
    depends("pick", "UseStonePick");
    let stone = output("stone", "Stone");
    set(stone, "blueprint", "Stone");
}
