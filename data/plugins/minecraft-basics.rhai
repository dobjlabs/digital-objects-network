// minecraft-basics.rhai — Portable plugin for the minecraft basics world.
//
// Defines 6 object classes, 9 actions (7 visible + 2 hidden), and 2 intro pod
// dependencies.  The host runs each action function in "recording mode" at load
// time to extract metadata, and later uses the recorded metadata to generate
// native ZK proof closures.

// ---------------------------------------------------------------------------
// Plugin identity
// ---------------------------------------------------------------------------

let plugin = #{
    name: "minecraft-basics",
    version: "0.1.0",
};

// ---------------------------------------------------------------------------
// Dependencies — intro pods this plugin needs (referenced by VK hash)
// ---------------------------------------------------------------------------

let dependencies = [
    #{
        dep_type: "Intro",
        pred: "Vdf(count, input, output)",
        hash: "b77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06",
    },
    #{
        dep_type: "Intro",
        pred: "LtEqU256(lhs, rhs)",
        hash: "2e79114ee823f4783ab5b6eb93b49abba87fb69b4d14de4cf1d78648ade73529",
    },
];

// ---------------------------------------------------------------------------
// Classes — object types this plugin works with
// ---------------------------------------------------------------------------

let classes = [
    #{ name: "Log",       emoji: "🌲", description: "A discovered log that can be refined into wood." },
    #{ name: "Wood",      emoji: "🪵", description: "Refined wood used for sticks and basic tools." },
    #{ name: "Stick",     emoji: "🥢", description: "A stick used as a handle in tool crafting." },
    #{ name: "WoodPick",  emoji: "⛏️",  description: "A wood pick that can mine stone while durability remains." },
    #{ name: "Stone",     emoji: "🪨", description: "Mined stone used to craft stronger tools." },
    #{ name: "StonePick", emoji: "⛏️",  description: "A sturdier pick with higher starting durability." },
];

// ---------------------------------------------------------------------------
// Action definitions — UI metadata + function name for each action
// ---------------------------------------------------------------------------

let action_defs = [
    #{ name: "FindLog",                fn_name: "FindLog",                emoji: "🌲", description: "Discover a log object by proving a short VDF.",               cpu_cost: "20-40s",  reads_block: false, hidden: false },
    #{ name: "CraftWood",              fn_name: "CraftWood",              emoji: "🪵", description: "Refine one log into a wood object with PoW quality checks.", cpu_cost: "15-30s",  reads_block: false, hidden: false },
    #{ name: "CraftSticks",            fn_name: "CraftSticks",            emoji: "🥢", description: "Split one wood object into two stick objects.",              cpu_cost: "5-10s",   reads_block: false, hidden: false },
    #{ name: "CraftWoodPick",          fn_name: "CraftWoodPick",          emoji: "⛏️",  description: "Combine wood and a stick to craft a wood pick.",           cpu_cost: "10-20s",  reads_block: false, hidden: false },
    #{ name: "CraftStonePick",         fn_name: "CraftStonePick",         emoji: "⛏️",  description: "Combine stone and a stick to craft a stronger stone pick.", cpu_cost: "10-20s", reads_block: false, hidden: false },
    #{ name: "UseWoodPick",            fn_name: "UseWoodPick",            emoji: "⛏️",  description: "Internal durability/work update for wood pick usage.",    cpu_cost: "10-30s",  reads_block: false, hidden: true  },
    #{ name: "MineStoneWithWoodPick",  fn_name: "MineStoneWithWoodPick",  emoji: "🪨", description: "Mine stone using a wood pick (consumes durability).",        cpu_cost: "25-45s",  reads_block: false, hidden: false },
    #{ name: "UseStonePick",           fn_name: "UseStonePick",           emoji: "⛏️",  description: "Internal durability/work update for stone pick usage.",   cpu_cost: "5-20s",   reads_block: false, hidden: true  },
    #{ name: "MineStoneWithStonePick", fn_name: "MineStoneWithStonePick", emoji: "🪨", description: "Mine stone using a stone pick (consumes durability).",       cpu_cost: "15-35s",  reads_block: false, hidden: false },
];

// ---------------------------------------------------------------------------
// Shared helper: pick usage pattern (durability check + decrement + VDF)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Action functions
// ---------------------------------------------------------------------------

fn FindLog() {
    let log = output("log", "Log");
    set(log, "blueprint", "Log");
    let work = vdf(3, obj_raw(log));
    update(log, "work", work);
}

fn CraftWood() {
    let _log = input("log", "Log");
    let wood = output("wood", "Wood");
    set(wood, "blueprint", "Wood");
    let key = pow_grind(wood, 9007199254740992);
    update(wood, "key", key);
    lt_eq_u256(wood, 9007199254740992);
}

fn CraftSticks() {
    let _wood = input("wood", "Wood");
    let a = output("stick_a", "Stick");
    let b = output("stick_b", "Stick");
    set(a, "blueprint", "Stick");
    set(b, "blueprint", "Stick");
}

fn CraftWoodPick() {
    let _wood = input("wood", "Wood");
    let _stick = input("stick", "Stick");
    let pick = output("wood_pick", "WoodPick");
    set(pick, "blueprint", "WoodPick");
    set_int(pick, "durability", 100);
}

fn CraftStonePick() {
    let _stone = input("stone", "Stone");
    let _stick = input("stick", "Stick");
    let pick = output("stone_pick", "StonePick");
    set(pick, "blueprint", "StonePick");
    set_int(pick, "durability", 200);
}

fn UseWoodPick() {
    let pick = mutate("wood_pick", "WoodPick");
    use_pick(pick, 10);
}

fn MineStoneWithWoodPick() {
    depends("pick", "UseWoodPick");
    let stone = output("stone", "Stone");
    set(stone, "blueprint", "Stone");
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
