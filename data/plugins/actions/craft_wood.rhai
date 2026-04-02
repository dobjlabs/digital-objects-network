// craft_wood.rhai — Refine one log into a wood object with PoW quality checks.

let plugin = #{
    name: "craft-wood",
    version: "0.1.0",
};

let imports = ["find-log"];

let dependencies = [
    #{
        dep_type: "Intro",
        pred: "LtEqU256(lhs, rhs)",
        hash: "2e79114ee823f4783ab5b6eb93b49abba87fb69b4d14de4cf1d78648ade73529",
    },
];

let classes = [
    #{ name: "Wood", emoji: "🪵", description: "Refined wood used for sticks and basic tools." },
];

let action_defs = [
    #{ name: "CraftWood", fn_name: "CraftWood", emoji: "🪵", description: "Refine one log into a wood object with PoW quality checks.", cpu_cost: "15-30s", reads_block: false, hidden: false },
];

fn CraftWood() {
    let _log = input("log", "Log");
    let wood = output("wood", "Wood");
    set(wood, "blueprint", "Wood");
    let key = pow_grind(wood, 9007199254740992);
    update(wood, "key", key);
    lt_eq_u256(wood, 9007199254740992);
}
