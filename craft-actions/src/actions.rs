//! Per-action predicate validators for the craft-basics world.
//!
//! Each function panics on a predicate violation. Direct ports of the rhai
//! actions in `plugins/craft-basics/plugin.rhai`. Mapping:
//!
//! | rhai          | rust                  |
//! | ------------- | --------------------- |
//! | `output(C)`   | check `new_objects[i].blueprint == C` |
//! | `input(C)`    | check `inputs[i].obj.blueprint == C`  |
//! | `mutate(C)`   | inputs has 1 obj of class C, outputs has 1 obj of class C |
//! | `intro_vdf(N, obj)` | re-run SHA-256 chain of length N, expecting `obj.work` |
//! | `pow_obj_grind(obj, target)` | (host-side; guest just verifies the resulting LtEqU256) |
//! | `intro_lt_eq_u256(obj, target)` | check `obj.commitment <= target` byte-wise |
//! | `update("k", v)` | check the new object has `k = v` (bound to whatever the SDK rule was) |

use txlib_core::Object;
use txlib_core::abi::GuestInput;
use txlib_core::value::Value;

use crate::intro::{check_le_u256, pull_vdf_witness, top_limb_u256, verify_vdf_chain};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn assert_input_count(input: &GuestInput, n: usize) {
    assert_eq!(
        input.inputs.len(),
        n,
        "expected {n} inputs, got {}",
        input.inputs.len()
    );
}

fn assert_output_count(input: &GuestInput, n: usize) {
    assert_eq!(
        input.new_objects.len(),
        n,
        "expected {n} new objects, got {}",
        input.new_objects.len()
    );
}

fn assert_blueprint(obj: &Object, expected: &str) {
    assert_eq!(
        obj.blueprint(),
        Some(expected),
        "blueprint mismatch: expected {expected}, got {:?}",
        obj.blueprint()
    );
}

/// Find an input by blueprint, returning its index. Panics if not found.
fn find_input_by_blueprint(input: &GuestInput, class: &str) -> usize {
    input
        .inputs
        .iter()
        .position(|i| i.obj.blueprint() == Some(class))
        .unwrap_or_else(|| panic!("missing input of class {class}"))
}

fn require_int(obj: &Object, key: &str) -> i64 {
    match obj.fields.get(key) {
        Some(Value::Int(n)) => *n,
        other => panic!("field `{key}`: expected Int, got {other:?}"),
    }
}

fn require_hash(obj: &Object, key: &str) -> txlib_core::Hash {
    match obj.fields.get(key) {
        Some(Value::Hash(h)) => *h,
        other => panic!("field `{key}`: expected Hash, got {other:?}"),
    }
}

/// Commitment of `obj` after removing the `work` field — what the VDF input
/// is. `intro_vdf(N, obj)` in rhai semantically means "VDF over the current
/// object state", and the action immediately calls `obj.update("work", work)`
/// after, so the FINAL object has `work = vdf_output` while the VDF was
/// computed over the state with `work = empty`.
///
/// For our SHA-256 system we treat the "no work" state as: the same object
/// minus the `work` field. Both the prover and verifier reconstruct this.
fn commitment_without_work(obj: &Object) -> txlib_core::Hash {
    let mut shadow = obj.clone();
    shadow.fields.remove("work");
    shadow.commitment()
}

// ---------------------------------------------------------------------------
// FindLog — discover a Log by proving a 3-step VDF
// ---------------------------------------------------------------------------

pub fn find_log(input: &GuestInput) {
    assert_input_count(input, 0);
    assert_output_count(input, 1);

    let log = &input.new_objects[0];
    assert_blueprint(log, "Log");

    let vdf_input = commitment_without_work(log);
    let work = require_hash(log, "work");
    let (witness_input, witness_output) = pull_vdf_witness(&input.intro_witnesses, 3);

    assert_eq!(witness_input, vdf_input, "Vdf input must match log commitment without work");
    assert_eq!(witness_output, work, "Vdf output must match log.work");
    verify_vdf_chain(3, vdf_input, work);
}

// ---------------------------------------------------------------------------
// CraftWood — refine 1 Log into 1 Wood, with PoW on the wood commitment
// ---------------------------------------------------------------------------

/// PoW difficulty target: top-limb ≤ 2^53. Matches the rhai literal in
/// `plugins/craft-basics/plugin.rhai`. Pub so callers (and tests) can grind
/// against the same target the validator checks.
pub const CRAFT_WOOD_TARGET_TOP_LIMB: u64 = 9_007_199_254_740_992;

pub fn craft_wood(input: &GuestInput) {
    assert_input_count(input, 1);
    assert_blueprint(&input.inputs[0].obj, "Log");

    assert_output_count(input, 1);
    let wood = &input.new_objects[0];
    assert_blueprint(wood, "Wood");

    // PoW: wood.commitment <= target. The host-side prover grinds `wood.key`
    // until the commitment satisfies this; the guest only re-verifies.
    let target = top_limb_u256(CRAFT_WOOD_TARGET_TOP_LIMB);
    check_le_u256(wood.commitment().as_bytes(), &target);
}

// ---------------------------------------------------------------------------
// CraftSticks — split 1 Wood into 2 Sticks
// ---------------------------------------------------------------------------

pub fn craft_sticks(input: &GuestInput) {
    assert_input_count(input, 1);
    assert_blueprint(&input.inputs[0].obj, "Wood");

    assert_output_count(input, 2);
    assert_blueprint(&input.new_objects[0], "Stick");
    assert_blueprint(&input.new_objects[1], "Stick");
}

// ---------------------------------------------------------------------------
// CraftWoodPick — combine 1 Wood + 1 Stick into 1 WoodPick (durability 100)
// ---------------------------------------------------------------------------

pub const WOOD_PICK_INITIAL_DURABILITY: i64 = 100;

pub fn craft_wood_pick(input: &GuestInput) {
    assert_input_count(input, 2);
    let _wood_idx = find_input_by_blueprint(input, "Wood");
    let _stick_idx = find_input_by_blueprint(input, "Stick");

    assert_output_count(input, 1);
    let pick = &input.new_objects[0];
    assert_blueprint(pick, "WoodPick");
    let durability = require_int(pick, "durability");
    assert_eq!(
        durability, WOOD_PICK_INITIAL_DURABILITY,
        "fresh WoodPick must have durability = {WOOD_PICK_INITIAL_DURABILITY}"
    );
}

// ---------------------------------------------------------------------------
// UseWoodPick — mutate a WoodPick (durability--, fresh key, VDF(10))
// ---------------------------------------------------------------------------

pub const USE_WOOD_PICK_VDF_ITERS: u32 = 10;

pub fn use_wood_pick(input: &GuestInput) {
    assert_input_count(input, 1);
    let old = &input.inputs[0].obj;
    assert_blueprint(old, "WoodPick");
    let old_durability = require_int(old, "durability");
    assert!(old_durability > 0, "WoodPick durability must be > 0 to use");

    assert_output_count(input, 1);
    let new_pick = &input.new_objects[0];
    assert_blueprint(new_pick, "WoodPick");
    let new_durability = require_int(new_pick, "durability");
    assert_eq!(
        new_durability,
        old_durability - 1,
        "durability must decrement by 1"
    );

    let old_key = require_hash(old, "key");
    let new_key = require_hash(new_pick, "key");
    assert_ne!(old_key, new_key, "mutation must rotate the `key` field");

    let vdf_input = commitment_without_work(new_pick);
    let work = require_hash(new_pick, "work");
    let (witness_input, witness_output) =
        pull_vdf_witness(&input.intro_witnesses, USE_WOOD_PICK_VDF_ITERS);

    assert_eq!(witness_input, vdf_input);
    assert_eq!(witness_output, work);
    verify_vdf_chain(USE_WOOD_PICK_VDF_ITERS, vdf_input, work);
}

#[cfg(test)]
mod tests {
    use super::*;
    use txlib_core::Hash;
    use txlib_core::abi::{GuestInput, IntroWitness};
    use txlib_core::hash::sha256;
    use txlib_core::object;
    use txlib_core::tx::StateRoot;

    fn empty_state_root() -> StateRoot {
        StateRoot::new(0, Hash::default(), Hash::default(), Hash::default())
    }

    fn empty_input(action_id: u32) -> GuestInput {
        GuestInput {
            action_id,
            state_root: empty_state_root(),
            inputs: alloc::vec::Vec::new(),
            new_objects: alloc::vec::Vec::new(),
            intro_witnesses: alloc::vec::Vec::new(),
        }
    }

    fn make_log_with_vdf() -> (txlib_core::Object, IntroWitness) {
        let key = sha256(b"log-key");
        let mut log = object! {
            "blueprint" => "Log",
            "key" => key,
        };
        // Compute VDF input from log without work.
        let vdf_input = log.commitment();
        // Run SHA-256 chain 3 times.
        let mut work = vdf_input;
        for _ in 0..3 {
            work = sha256(work.as_bytes());
        }
        log.insert("work", work);
        (
            log,
            IntroWitness::Vdf {
                iters: 3,
                input: vdf_input,
                output: work,
            },
        )
    }

    #[test]
    fn find_log_accepts_valid_witness() {
        let (log, witness) = make_log_with_vdf();
        let mut input = empty_input(crate::ACTION_FIND_LOG);
        input.new_objects.push(log);
        input.intro_witnesses.push(witness);
        find_log(&input); // shouldn't panic
    }

    #[test]
    #[should_panic(expected = "Vdf output must match log.work")]
    fn find_log_rejects_forged_log_work() {
        // Forging log.work to something else makes the witness/log binding
        // check fire before VDF re-computation.
        let (mut log, witness) = make_log_with_vdf();
        log.insert("work", sha256(b"forged work"));
        let mut input = empty_input(crate::ACTION_FIND_LOG);
        input.new_objects.push(log);
        input.intro_witnesses.push(witness);
        find_log(&input);
    }

    #[test]
    #[should_panic(expected = "VDF chain output mismatch")]
    fn find_log_rejects_forged_vdf_witness() {
        // Forge log.work to a wrong value AND forge the witness to claim
        // chain(commit_without_work, 3) = that wrong value. Both binding
        // checks pass; only the SHA-256 re-run catches it.
        let key = sha256(b"k");
        let mut log = object! { "blueprint" => "Log", "key" => key };
        let vdf_input = log.commitment();
        let bad_work = sha256(b"forged"); // not actually chain(vdf_input, 3)
        log.insert("work", bad_work);

        let mut input = empty_input(crate::ACTION_FIND_LOG);
        input.new_objects.push(log);
        input.intro_witnesses.push(IntroWitness::Vdf {
            iters: 3,
            input: vdf_input,
            output: bad_work,
        });
        find_log(&input);
    }

    #[test]
    #[should_panic(expected = "blueprint mismatch")]
    fn find_log_rejects_wrong_blueprint() {
        let (mut log, witness) = make_log_with_vdf();
        log.insert("blueprint", "NotALog");
        let mut input = empty_input(crate::ACTION_FIND_LOG);
        input.new_objects.push(log);
        input.intro_witnesses.push(witness);
        find_log(&input);
    }

    #[test]
    fn craft_sticks_accepts_2_sticks_from_1_wood() {
        let mut input = empty_input(crate::ACTION_CRAFT_STICKS);
        let wood = object! {
            "blueprint" => "Wood",
            "key" => sha256(b"wk"),
        };
        input.inputs.push(make_input(wood));
        input.new_objects.push(object! {
            "blueprint" => "Stick",
            "key" => sha256(b"s1"),
        });
        input.new_objects.push(object! {
            "blueprint" => "Stick",
            "key" => sha256(b"s2"),
        });
        craft_sticks(&input);
    }

    #[test]
    #[should_panic(expected = "expected 2 new objects")]
    fn craft_sticks_rejects_wrong_output_count() {
        let mut input = empty_input(crate::ACTION_CRAFT_STICKS);
        input.inputs.push(make_input(object! {
            "blueprint" => "Wood",
            "key" => sha256(b"wk"),
        }));
        input.new_objects.push(object! {
            "blueprint" => "Stick",
            "key" => sha256(b"s1"),
        });
        craft_sticks(&input);
    }

    #[test]
    fn craft_wood_pick_accepts_wood_plus_stick() {
        let mut input = empty_input(crate::ACTION_CRAFT_WOOD_PICK);
        input.inputs.push(make_input(object! {
            "blueprint" => "Wood",
            "key" => sha256(b"wk"),
        }));
        input.inputs.push(make_input(object! {
            "blueprint" => "Stick",
            "key" => sha256(b"sk"),
        }));
        input.new_objects.push(object! {
            "blueprint" => "WoodPick",
            "key" => sha256(b"pk"),
            "durability" => 100i64,
        });
        craft_wood_pick(&input);
    }

    #[test]
    #[should_panic(expected = "fresh WoodPick must have durability")]
    fn craft_wood_pick_rejects_wrong_durability() {
        let mut input = empty_input(crate::ACTION_CRAFT_WOOD_PICK);
        input.inputs.push(make_input(object! {
            "blueprint" => "Wood",
            "key" => sha256(b"wk"),
        }));
        input.inputs.push(make_input(object! {
            "blueprint" => "Stick",
            "key" => sha256(b"sk"),
        }));
        input.new_objects.push(object! {
            "blueprint" => "WoodPick",
            "key" => sha256(b"pk"),
            "durability" => 99i64,
        });
        craft_wood_pick(&input);
    }

    #[test]
    fn use_wood_pick_decrements_durability_and_verifies_vdf() {
        let mut input = empty_input(crate::ACTION_USE_WOOD_PICK);
        let old_pick = object! {
            "blueprint" => "WoodPick",
            "key" => sha256(b"old"),
            "durability" => 50i64,
        };
        let mut new_pick = object! {
            "blueprint" => "WoodPick",
            "key" => sha256(b"new"),
            "durability" => 49i64,
        };
        let vdf_input = new_pick.commitment();
        let mut work = vdf_input;
        for _ in 0..USE_WOOD_PICK_VDF_ITERS {
            work = sha256(work.as_bytes());
        }
        new_pick.insert("work", work);

        input.inputs.push(make_input(old_pick));
        input.new_objects.push(new_pick);
        input.intro_witnesses.push(IntroWitness::Vdf {
            iters: USE_WOOD_PICK_VDF_ITERS,
            input: vdf_input,
            output: work,
        });
        use_wood_pick(&input);
    }

    #[test]
    #[should_panic(expected = "WoodPick durability must be > 0")]
    fn use_wood_pick_rejects_zero_durability() {
        let mut input = empty_input(crate::ACTION_USE_WOOD_PICK);
        let old_pick = object! {
            "blueprint" => "WoodPick",
            "key" => sha256(b"old"),
            "durability" => 0i64,
        };
        let new_pick = object! {
            "blueprint" => "WoodPick",
            "key" => sha256(b"new"),
            "durability" => -1i64,
            "work" => Hash::default(),
        };
        input.inputs.push(make_input(old_pick));
        input.new_objects.push(new_pick);
        use_wood_pick(&input);
    }

    #[test]
    #[should_panic(expected = "mutation must rotate the `key` field")]
    fn use_wood_pick_rejects_unchanged_key() {
        let mut input = empty_input(crate::ACTION_USE_WOOD_PICK);
        let key = sha256(b"same-key");
        let old = object! {
            "blueprint" => "WoodPick",
            "key" => key,
            "durability" => 5i64,
        };
        let mut new_pick = object! {
            "blueprint" => "WoodPick",
            "key" => key,
            "durability" => 4i64,
        };
        let vdf_input = new_pick.commitment();
        let mut work = vdf_input;
        for _ in 0..USE_WOOD_PICK_VDF_ITERS {
            work = sha256(work.as_bytes());
        }
        new_pick.insert("work", work);
        input.inputs.push(make_input(old));
        input.new_objects.push(new_pick);
        input.intro_witnesses.push(IntroWitness::Vdf {
            iters: USE_WOOD_PICK_VDF_ITERS,
            input: vdf_input,
            output: work,
        });
        use_wood_pick(&input);
    }

    /// Helper: wrap an `Object` in a default `InputObject` (zeroed grounding
    /// proofs). The action validators don't care about grounding — that's
    /// done separately by [`crate::grounding::verify_all`].
    fn make_input(obj: txlib_core::Object) -> txlib_core::abi::InputObject {
        use txlib_core::abi::InputObject;
        use txlib_core::merkle::{MerkleProof, SMT_DEPTH};
        InputObject {
            obj,
            source_tx_action_id: 0,
            source_tx_live_root: Hash::default(),
            source_tx_nullifiers_root: Hash::default(),
            source_tx_action_nonce: Hash::default(),
            live_inclusion_proof: MerkleProof {
                siblings: alloc::vec![Hash::default(); SMT_DEPTH],
            },
            tx_inclusion_proof: MerkleProof {
                siblings: alloc::vec![Hash::default(); SMT_DEPTH],
            },
        }
    }
}
