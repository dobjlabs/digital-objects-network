// The asymmetric-count swap: one log crosses one way, two sticks the
// other. Each rekey rotates its object's key so the giver's copy dies;
// the proof budget (see manifest.toml) rules out a receipt output.
fn SwapLogSticks(action) {
    var log = action.subaction("craft-basics::RekeyLog");
    var stick_a = action.subaction("craft-basics::RekeyStick");
    var stick_b = action.subaction("craft-basics::RekeyStick");
}
