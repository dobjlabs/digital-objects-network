// mine_stone_with_wood_pick.rhai — Mine stone using a wood pick (consumes durability).

let plugin = #{
    name: "mine-stone-with-wood-pick",
    version: "0.1.0",
};

let imports = ["use-wood-pick"];

let dependencies = [];

let classes = [
    #{ name: "Stone", emoji: "🪨", description: "Mined stone used to craft stronger tools." },
];

let action_defs = [
    #{ name: "MineStoneWithWoodPick", fn_name: "MineStoneWithWoodPick", emoji: "🪨", description: "Mine stone using a wood pick (consumes durability).", cpu_cost: "25-45s", reads_block: false, hidden: false },
];

fn MineStoneWithWoodPick() {
    depends("pick", "UseWoodPick");
    let stone = output("stone", "Stone");
    set(stone, "blueprint", "Stone");
}
