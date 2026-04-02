// craft_sticks.rhai — Split one wood object into two stick objects.

let plugin = #{
    name: "craft-sticks",
    version: "0.1.0",
};

let imports = ["craft-wood"];

let dependencies = [];

let classes = [
    #{ name: "Stick", emoji: "🥢", description: "A stick used as a handle in tool crafting." },
];

let action_defs = [
    #{ name: "CraftSticks", fn_name: "CraftSticks", emoji: "🥢", description: "Split one wood object into two stick objects.", cpu_cost: "5-10s", reads_block: false, hidden: false },
];

fn CraftSticks() {
    let _wood = input("wood", "Wood");
    let a = output("stick_a", "Stick");
    let b = output("stick_b", "Stick");
    set(a, "blueprint", "Stick");
    set(b, "blueprint", "Stick");
}
