// A plugin that reaches into two other plugins.
//
// `action.subaction("craft-basics::RekeyLog")` runs craft-basics' own action
// as a nested step of this one, which is what makes the log's class guard
// match: a class is the OR over the actions of the script that defines it, so
// `action.mutate("Log")` written here would mean *this* plugin's Log and
// could never spend a craft-basics one. The copper side works the same way
// through craft-rocket.
//
// Dependencies are declared in the manifest's [[imports]] -- each entry
// names the plugin, the pexe path to load it from, and the batch id to pin;
// the declaration must match the script's qualified calls exactly. The
// coupling is unavoidable either way: calling into a plugin puts its batch
// id inside this one's, so this plugin's own classes rehash whenever a
// dependency does.
//
// `Swapped` is a class this plugin owns, minted in the same transaction, so
// the receipt lands if and only if both rekeys do.

fn SwapLogCopper(action) {
    var log = action.subaction("craft-basics::RekeyLog");
    var copper = action.subaction("craft-rocket::RekeyCopper");
    var receipt = action.output("Swapped");
    receipt.set([
        ["swapped_at", state_header.block_timestamp]
    ]);
}
