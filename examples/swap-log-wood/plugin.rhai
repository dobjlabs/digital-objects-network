// swap-log-wood's own class. A script can only ever declare and touch its
// own classes -- `action.output("Swapped")` means *this* plugin's Swapped,
// because a class is the OR over the actions of the script that defines it.
// That is exactly why the log and wood halves of the swap are craft-basics
// actions named in the recipe rather than code written here.
//
// This action runs as a third sibling in the same transaction, so the receipt
// lands if and only if both claims do.

fn MintSwapped(action) {
    var receipt = action.output("Swapped");
    receipt.set([
        ["swapped_at", state_header.block_timestamp]
    ]);
}
