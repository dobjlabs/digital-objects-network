fn FindLog() {
    let log = output("log", "Log");
    set(log, "blueprint", "Log");
    let work = vdf(3, obj_raw(log));
    update(log, "work", work);
}
