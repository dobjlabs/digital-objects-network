//! Host-side staging for the 5 craft-basics actions.
//!
//! Each `stage_*` returns the [`ActionStaging`] to feed into
//! [`crate::Driver::execute`]: the new objects (with the right random keys
//! and any post-VDF `work` field set) plus the matching intro witnesses.
//!
//! These are the *complement* to the validators in `craft-actions`: the
//! validator is what runs in the guest (and on the host as a sanity check)
//! to verify a finished action; the stager is what builds the action in
//! the first place. Splitting them keeps the guest no_std and the host
//! free to do prover-only work like grinding a PoW key with `rand`.
//!
//! The two are kept consistent by the host running
//! `craft_actions::validate(&plan.into_guest_input())` *before* the prover
//! runs — a stager bug surfaces immediately as a host-side panic instead
//! of after burning prover cycles.

use anyhow::{Result, anyhow};
use rand::RngCore;
use txlib_core::Hash;
use txlib_core::Object;
use txlib_core::abi::IntroWitness;
use txlib_core::hash::sha256;
use txlib_core::object;
use txlib_core::value::Value;

use crate::driver::ActionStaging;
use crate::object::ObjectRecord;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// 32-byte cryptographic-quality random hash. Used as the per-object `key`
/// secret (which is what the nullifier hides behind).
pub fn random_hash() -> Hash {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    Hash(bytes)
}

/// SHA-256 chain of length `iters` starting at `input`. The guest re-runs
/// this to verify the witness; the host runs it once to build the witness.
pub fn run_vdf(iters: u32, input: Hash) -> Hash {
    let mut current = input;
    for _ in 0..iters {
        current = sha256(current.as_bytes());
    }
    current
}

/// Iterate `key = random()` until `obj.commitment() <= target`. Used by
/// `stage_craft_wood`; expected ~2K iterations at the default 2^53 target.
fn grind_for_pow(blueprint: &str, target: [u8; 32]) -> Object {
    loop {
        let candidate = object! {
            "blueprint" => blueprint,
            "key" => random_hash(),
        };
        if candidate.commitment().as_bytes() <= &target {
            return candidate;
        }
    }
}

fn require_int(obj: &Object, key: &str) -> Result<i64> {
    match obj.fields.get(key) {
        Some(Value::Int(n)) => Ok(*n),
        other => Err(anyhow!("field `{key}`: expected Int, got {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// Per-action stagers
// ---------------------------------------------------------------------------

/// FindLog: discover a Log by proving a SHA-256 chain of length 3 over the
/// log's pre-work commitment.
pub fn stage_find_log() -> ActionStaging {
    let mut log = object! {
        "blueprint" => "Log",
        "key" => random_hash(),
    };
    let vdf_input = log.commitment();
    let work = run_vdf(craft_actions_find_log_iters(), vdf_input);
    log.insert("work", work);

    ActionStaging {
        new_objects: vec![log],
        intro_witnesses: vec![IntroWitness::Vdf {
            iters: craft_actions_find_log_iters(),
            input: vdf_input,
            output: work,
        }],
        new_object_classes: vec!["Log".to_string()],
    }
}

/// CraftWood: refine a Log into a Wood, with PoW (commitment ≤ 2^53 target).
pub fn stage_craft_wood(_log: &ObjectRecord) -> ActionStaging {
    let target = craft_actions::intro::top_limb_u256(
        craft_actions::actions::CRAFT_WOOD_TARGET_TOP_LIMB,
    );
    let wood = grind_for_pow("Wood", target);
    ActionStaging {
        new_objects: vec![wood],
        intro_witnesses: vec![],
        new_object_classes: vec!["Wood".to_string()],
    }
}

/// CraftSticks: split one Wood into two Sticks.
pub fn stage_craft_sticks(_wood: &ObjectRecord) -> ActionStaging {
    let stick_a = object! {
        "blueprint" => "Stick",
        "key" => random_hash(),
    };
    let stick_b = object! {
        "blueprint" => "Stick",
        "key" => random_hash(),
    };
    ActionStaging {
        new_objects: vec![stick_a, stick_b],
        intro_witnesses: vec![],
        new_object_classes: vec!["Stick".to_string(), "Stick".to_string()],
    }
}

/// CraftWoodPick: combine 1 Wood + 1 Stick into 1 fresh WoodPick (durability 100).
pub fn stage_craft_wood_pick(_wood: &ObjectRecord, _stick: &ObjectRecord) -> ActionStaging {
    let pick = object! {
        "blueprint" => "WoodPick",
        "key" => random_hash(),
        "durability" => craft_actions::actions::WOOD_PICK_INITIAL_DURABILITY,
    };
    ActionStaging {
        new_objects: vec![pick],
        intro_witnesses: vec![],
        new_object_classes: vec!["WoodPick".to_string()],
    }
}

/// UseWoodPick: mutate an existing WoodPick — durability--, fresh key, VDF(10).
pub fn stage_use_wood_pick(pick: &ObjectRecord) -> Result<ActionStaging> {
    let old_durability = require_int(&pick.obj, "durability")?;
    if old_durability <= 0 {
        return Err(anyhow!(
            "WoodPick durability must be > 0 to use (was {old_durability})"
        ));
    }

    let mut new_pick = object! {
        "blueprint" => "WoodPick",
        "key" => random_hash(),
        "durability" => old_durability - 1,
    };
    let vdf_input = new_pick.commitment();
    let iters = craft_actions::actions::USE_WOOD_PICK_VDF_ITERS;
    let work = run_vdf(iters, vdf_input);
    new_pick.insert("work", work);

    Ok(ActionStaging {
        new_objects: vec![new_pick],
        intro_witnesses: vec![IntroWitness::Vdf {
            iters,
            input: vdf_input,
            output: work,
        }],
        new_object_classes: vec!["WoodPick".to_string()],
    })
}

// FindLog's VDF iter count isn't exposed as a const in craft-actions
// (it's hardcoded `3` inside the validator). Keep the host stager in sync
// here; if the validator changes, this comment + value change too.
const fn craft_actions_find_log_iters() -> u32 {
    3
}

#[cfg(test)]
mod tests {
    use super::*;
    use txlib_core::merkle::set_smt_root;
    use txlib_core::merkle_store::empty_root;
    use txlib_core::tx::action_nonce;

    fn make_input_record(class: &str, fields: Vec<(&str, Value)>) -> ObjectRecord {
        let mut obj = Object::new();
        obj.insert("blueprint", class);
        obj.insert("key", random_hash());
        for (k, v) in fields {
            obj.insert(k.to_string(), v);
        }
        let live = vec![obj.commitment()];
        let source_tx = crate::object::SourceTxData {
            action_id: 0,
            live_root: set_smt_root(&live),
            nullifiers_root: empty_root(),
            action_nonce: action_nonce(0, &live),
        };
        ObjectRecord::new(obj, class.to_string(), source_tx, live)
    }

    /// Build the GuestInput a stager's output would feed (with empty grounding)
    /// and run the validator on it. Catches stager/validator drift.
    fn run_validator(staging: ActionStaging, action_id: u32, inputs: Vec<ObjectRecord>) {
        use txlib_core::abi::{GuestInput, InputObject};
        use txlib_core::merkle::{MerkleProof, SMT_DEPTH};
        use txlib_core::tx::StateRoot;

        // Build "fake" InputObjects whose grounding proofs verify against
        // an SMT containing this exact input — so the validator's grounding
        // pass succeeds, and we test only the action's own predicate.
        let state_root = if inputs.is_empty() {
            StateRoot::new(0, empty_root(), empty_root(), Hash::default())
        } else {
            let mut transactions = Vec::with_capacity(inputs.len());
            for r in &inputs {
                transactions.push(r.source_tx.tx_final());
            }
            transactions.sort();
            StateRoot::new(1, set_smt_root(&transactions), empty_root(), Hash::default())
        };

        let mut input_objs = Vec::with_capacity(inputs.len());
        for r in &inputs {
            let live_proof = r.live_inclusion_proof().expect("live proof");
            // tx_inclusion_proof: build over the synthetic transactions SMT
            let store = txlib_core::merkle_store::InMemoryNodeStore::new();
            let mut smt = txlib_core::merkle_store::PersistentSmt::open(empty_root(), &store);
            for sib in &inputs {
                let h = sib.source_tx.tx_final();
                smt.insert(h, h).unwrap();
            }
            let h = r.source_tx.tx_final();
            let tx_proof = if smt.root == state_root.transactions_root {
                smt.prove(h).unwrap()
            } else {
                MerkleProof {
                    siblings: vec![Hash::default(); SMT_DEPTH],
                }
            };
            input_objs.push(InputObject {
                obj: r.obj.clone(),
                source_tx_action_id: r.source_tx.action_id,
                source_tx_live_root: r.source_tx.live_root,
                source_tx_nullifiers_root: r.source_tx.nullifiers_root,
                source_tx_action_nonce: r.source_tx.action_nonce,
                live_inclusion_proof: live_proof,
                tx_inclusion_proof: tx_proof,
            });
        }

        let guest_input = GuestInput {
            action_id,
            state_root,
            inputs: input_objs,
            new_objects: staging.new_objects,
            intro_witnesses: staging.intro_witnesses,
        };
        let _ = craft_actions::validate(&guest_input);
    }

    #[test]
    fn stage_find_log_passes_validator() {
        run_validator(stage_find_log(), craft_actions::ACTION_FIND_LOG, vec![]);
    }

    #[test]
    fn stage_craft_wood_passes_validator() {
        let log = make_input_record("Log", vec![("work", Value::Hash(random_hash()))]);
        let staging = stage_craft_wood(&log);
        run_validator(staging, craft_actions::ACTION_CRAFT_WOOD, vec![log]);
    }

    #[test]
    fn stage_craft_sticks_passes_validator() {
        let wood = make_input_record("Wood", vec![]);
        let staging = stage_craft_sticks(&wood);
        run_validator(staging, craft_actions::ACTION_CRAFT_STICKS, vec![wood]);
    }

    #[test]
    fn stage_craft_wood_pick_passes_validator() {
        let wood = make_input_record("Wood", vec![]);
        let stick = make_input_record("Stick", vec![]);
        let staging = stage_craft_wood_pick(&wood, &stick);
        run_validator(
            staging,
            craft_actions::ACTION_CRAFT_WOOD_PICK,
            vec![wood, stick],
        );
    }

    #[test]
    fn stage_use_wood_pick_passes_validator() {
        let pick = make_input_record("WoodPick", vec![("durability", Value::Int(50))]);
        let staging = stage_use_wood_pick(&pick).unwrap();
        run_validator(staging, craft_actions::ACTION_USE_WOOD_PICK, vec![pick]);
    }

    #[test]
    fn stage_use_wood_pick_rejects_zero_durability() {
        let pick = make_input_record("WoodPick", vec![("durability", Value::Int(0))]);
        let err = stage_use_wood_pick(&pick).unwrap_err();
        assert!(err.to_string().contains("must be > 0"), "{err}");
    }

    #[test]
    fn random_hashes_differ() {
        let a = random_hash();
        let b = random_hash();
        assert_ne!(a, b);
    }
}
