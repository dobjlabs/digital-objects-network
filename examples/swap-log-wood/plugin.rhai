// A plugin that reaches into another one.
//
// `action.subaction("craft-basics::RekeyLog")` runs craft-basics' own action
// as a nested step of this one, which is what makes the log's class guard
// match: a class is the OR over the actions of the script that defines it, so
// `action.mutate("Log")` written here would mean *this* plugin's Log and
// could never spend a craft-basics one.
//
// Declaring the dependency is the call itself -- nothing to keep in step in
// the manifest. The cost is coupling: calling into craft-basics puts its
// batch id inside this plugin's, so this plugin's own classes rehash whenever
// craft-basics does.
//
// `Swapped` is a class this plugin owns, minted in the same transaction, so
// the receipt lands if and only if both rekeys do.

fn SwapLogWood(action) {
    var log = action.subaction("craft-basics::RekeyLog");
    var wood = action.subaction("craft-basics::RekeyWood");
    var receipt = action.output("Swapped");
    receipt.set([
        ["swapped_at", state_header.block_timestamp]
    ]);
}
