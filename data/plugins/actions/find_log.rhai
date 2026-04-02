// find_log.rhai — Discover a log object by proving a short VDF.

let plugin = #{
    name: "find-log",
    version: "0.1.0",
};

let imports = [];

let dependencies = [
    #{
        dep_type: "Intro",
        pred: "Vdf(count, input, output)",
        hash: "b77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06",
    },
];

let classes = [
    #{ name: "Log", emoji: "🌲", description: "A discovered log that can be refined into wood." },
];

let action_defs = [
    #{ name: "FindLog", fn_name: "FindLog", emoji: "🌲", description: "Discover a log object by proving a short VDF.", cpu_cost: "20-40s", reads_block: false, hidden: false },
];

fn FindLog() {
    let log = output("log", "Log");
    set(log, "blueprint", "Log");
    let work = vdf(3, obj_raw(log));
    update(log, "work", work);
}
