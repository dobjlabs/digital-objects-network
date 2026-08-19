fn CraftTotem(action) {
    // Cross-plugin call: craft-basics's CraftWood runs as a sub-action,
    // consuming a Log from the caller's inputs and producing a Wood
    // under craft-basics's own rules. The totem seals in the wood's
    // PoW key.
    var wood = action.subaction("craft-basics::CraftWood");
    var totem = action.output("Totem");
    totem.set([
        ["seal", wood.key]
    ]);
}
