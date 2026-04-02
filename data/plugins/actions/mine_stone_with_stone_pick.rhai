// mine_stone_with_stone_pick.rhai — Mine stone using a stone pick (consumes durability).

let plugin = #{
    name: "mine-stone-with-stone-pick",
    version: "0.1.0",
};

let imports = ["use-stone-pick"];

let dependencies = [];

let classes = [
    #{ name: "Stone", emoji: "🪨", description: "Mined stone used to craft stronger tools." },
];

let action_defs = [
    #{ name: "MineStoneWithStonePick", fn_name: "MineStoneWithStonePick", emoji: "🪨", description: "Mine stone using a stone pick (consumes durability).", cpu_cost: "15-35s", reads_block: false, hidden: false },
];

fn MineStoneWithStonePick() {
    depends("pick", "UseStonePick");
    let stone = output("stone", "Stone");
    set(stone, "blueprint", "Stone");
}
