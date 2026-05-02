// subsidy-demo: B2B grain-subsidy flow
//
// Three actors (each a separate prover identity in the GUI):
//   - Employer issues income credentials off-chain (a SignedDict,
//     not an on-chain object) by signing a `{income, ...}` dict.
//   - Govt runs `IssueSubsidy` to verify an income credential and
//     mint a 5-bag `Subsidy` object signed by GOVT_PK.
//   - Farmer runs `RedeemGrainBag` to mutate the subsidy
//     (bags_remaining--) and produce a `Grain` object.
//   - Merchant runs `ConsumeGrain` to delete a Grain (receipt).
//
// The hardcoded public keys below are *demo* keys; matching secret
// keys live in dev_keys.toml. Regenerate via:
//   cargo test -p sdk gen_demo_keys -- --ignored --nocapture

fn employer_pk(action) {
    action.public_key("7xuKArQpDm5D2XgBALHdhRKQh6wvFtcD1tj9S1AY8GgkmAU1v12nsx")
}

fn govt_pk(action) {
    action.public_key("As6KpqvCCJ1Axw82BvqQ7P1UZGYMxcGm9HemrJWUUVDRHWRsaV89PG8")
}

fn IssueSubsidy(action) {
    // Consume an externally-issued income credential signed by the
    // employer. The dict stays private; only `SignedBy(income, EMPLOYER_PK)`
    // appears as a constraint in the action's predicate.
    var income = action.input_signed_dict(employer_pk(action));

    // TODO: re-add an income-threshold check once we have a `st_lt` /
    // `st_lt_eq` binding (or figure out the right Gt operand pattern
    // for `literal vs var.entry`).

    // Mint a fresh Subsidy with 5 bags. Govt is the prover, so the
    // SignedBy(subsidy, GOVT_PK) clause requires the govt's secret
    // key on the executor (`Executor::add_signer`).
    var subsidy = action.output("Subsidy");
    subsidy.set([
        ["blueprint", "Subsidy"],
        ["max_bags", 5],
        ["bags_remaining", 5]
    ]);
    action.signed_by(subsidy, govt_pk(action));
}

// Hidden helper: decrement an active Subsidy's bags_remaining by 1.
// Lifted out of `RedeemGrainBag` so the user-facing action only crosses
// pod2's per-segment public-arg budget once (subsidy mutate + grain output
// in the same scope hit the 5-arg limit).
fn DecrementSubsidy(action) {
    var subsidy = action.mutate("Subsidy");
    action.st_gt(subsidy.bags_remaining, 0);
    var new_remaining = unsafe { subsidy.bags_remaining - 1 };
    action.st_sum_of(subsidy.bags_remaining, new_remaining, 1);
    subsidy.update("bags_remaining", new_remaining);
}

fn RedeemGrainBag(action) {
    var subsidy = action.subaction("DecrementSubsidy");
    var grain = action.output("Grain");
    grain.set([["blueprint", "Grain"]]);
}

fn ConsumeGrain(action) {
    var grain = action.input("Grain");
}
