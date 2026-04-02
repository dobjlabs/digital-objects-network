// use_stone_pick.rhai — Internal durability/work update for stone pick usage.

let plugin = #{
    name: "use-stone-pick",
    version: "0.1.0",
};

let imports = ["craft-stone-pick"];

let dependencies = [
    #{
        dep_type: "Intro",
        pred: "Vdf(count, input, output)",
        hash: "b77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06",
    },
];

let classes = [];

let action_defs = [
    #{ name: "UseStonePick", fn_name: "UseStonePick", emoji: "⛏️", description: "Internal durability/work update for stone pick usage.", cpu_cost: "5-20s", reads_block: false, hidden: true },
];

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
