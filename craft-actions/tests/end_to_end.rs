//! End-to-end pipeline test: builds a realistic [`GuestInput`] (with proper
//! SMT-backed grounding proofs), runs [`craft_actions::validate`], and
//! checks the resulting [`GuestJournal`] is what the synchronizer would
//! accept.
//!
//! This is what the risc0 guest does on every action invocation — running
//! it on the host validates the whole pipeline without needing the actual
//! zkVM. Catches:
//! - hash recipe mismatches between client and server
//! - grounding-proof shape errors
//! - tx_final / live_root / nullifiers_root assembly bugs
//! - action predicate bugs

use craft_actions::*;
use txlib_core::abi::{ActionId, GuestInput, GuestJournal, InputObject, IntroWitness};
use txlib_core::hash::sha256;
use txlib_core::merkle::{MerkleProof, set_smt_root};
use txlib_core::merkle_store::{InMemoryNodeStore, PersistentSmt, empty_root};
use txlib_core::object;
use txlib_core::tx::{StateRoot, Tx, action_nonce, compute_nullifier};
use txlib_core::{Hash, Object};

/// Mini synchronizer: holds a transactions SMT and lets us mint tx_finals
/// at known canonical roots so we can build realistic grounding proofs.
struct MockChain {
    store: InMemoryNodeStore,
    transactions_root: Hash,
    block_number: i64,
}

impl MockChain {
    fn new() -> Self {
        Self {
            store: InMemoryNodeStore::new(),
            transactions_root: empty_root(),
            block_number: 1,
        }
    }

    fn finalize(&mut self, tx_final: Hash) -> MerkleProof {
        let mut smt = PersistentSmt::open(self.transactions_root, &self.store);
        smt.insert(tx_final, tx_final).unwrap();
        self.transactions_root = smt.root;
        smt.prove(tx_final).unwrap()
    }

    fn state_root(&self) -> StateRoot {
        StateRoot::new(
            self.block_number,
            self.transactions_root,
            empty_root(),
            Hash::default(),
        )
    }
}

/// Simulate "the prover prepared an InputObject for the new action by reading
/// the source tx's data + asking the synchronizer for a Merkle proof".
fn prepare_input(
    chain: &mut MockChain,
    obj: Object,
    source_action_id: ActionId,
    siblings: Vec<Hash>,
) -> InputObject {
    // 1. Build the source tx's live_root containing this object.
    let obj_commitment = obj.commitment();
    let mut live_smt = PersistentSmt::open(empty_root(), &chain.store);
    live_smt.insert(obj_commitment, obj_commitment).unwrap();
    let live_root = live_smt.root;
    let live_inclusion_proof = live_smt.prove(obj_commitment).unwrap();

    // 2. Compute the source tx's tx_final.
    let nonce = action_nonce(source_action_id, &[obj_commitment]);
    let source_tx = Tx {
        action_id: source_action_id,
        live_root,
        nullifiers_root: empty_root(),
        action_nonce: nonce,
    };
    let source_tx_final = source_tx.tx_final();

    // 3. Finalize the source tx in the chain.
    let tx_inclusion_proof = chain.finalize(source_tx_final);

    let _ = siblings; // arg retained for symmetry but not needed
    InputObject {
        obj,
        source_tx_action_id: source_action_id,
        source_tx_live_root: live_root,
        source_tx_nullifiers_root: empty_root(),
        source_tx_action_nonce: nonce,
        live_inclusion_proof,
        tx_inclusion_proof,
    }
}

fn empty_input(action_id: ActionId, state_root: StateRoot) -> GuestInput {
    GuestInput {
        action_id,
        state_root,
        inputs: Vec::new(),
        new_objects: Vec::new(),
        intro_witnesses: Vec::new(),
    }
}

fn run_vdf(iters: u32, input: Hash) -> Hash {
    let mut current = input;
    for _ in 0..iters {
        current = sha256(current.as_bytes());
    }
    current
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn find_log_full_pipeline() {
    let chain = MockChain::new();
    let mut input = empty_input(ACTION_FIND_LOG, chain.state_root());

    let mut log = object! {
        "blueprint" => "Log",
        "key" => sha256(b"my-log-key"),
    };
    let vdf_input = log.commitment();
    let work = run_vdf(3, vdf_input);
    log.insert("work", work);

    input.new_objects.push(log.clone());
    input.intro_witnesses.push(IntroWitness::Vdf {
        iters: 3,
        input: vdf_input,
        output: work,
    });

    let journal: GuestJournal = validate(&input);

    // Sanity: journal pieces are derived from the input deterministically.
    assert_eq!(journal.state_root_hash, input.state_root.hash());
    assert!(journal.nullifiers.is_empty()); // FindLog consumes nothing
    let expected_live = set_smt_root(&[log.commitment()]);
    let expected_nonce = action_nonce(ACTION_FIND_LOG, &[log.commitment()]);
    let expected_tx_final = Tx {
        action_id: ACTION_FIND_LOG,
        live_root: expected_live,
        nullifiers_root: empty_root(),
        action_nonce: expected_nonce,
    }
    .tx_final();
    assert_eq!(journal.tx_final, expected_tx_final);
}

#[test]
fn craft_wood_consumes_log_and_produces_grounded_wood() {
    let mut chain = MockChain::new();

    // Step 1: stage a finalized Log (would normally come from a prior
    // FindLog action).
    let log = object! {
        "blueprint" => "Log",
        "key" => sha256(b"log-k"),
        "work" => sha256(b"log-work"),
    };
    let log_input = prepare_input(&mut chain, log.clone(), ACTION_FIND_LOG, Vec::new());

    // Step 2: grind wood.key against the actual validator target (top limb
    // 2^53). Probability of pass per try is ~1/2048, so this takes a few
    // thousand SHA-256 calls on average.
    let target = craft_actions::intro::top_limb_u256(
        craft_actions::actions::CRAFT_WOOD_TARGET_TOP_LIMB,
    );
    let mut counter: u64 = 0;
    let wood = loop {
        let candidate = object! {
            "blueprint" => "Wood",
            "key" => sha256(&counter.to_le_bytes()),
        };
        if candidate.commitment().as_bytes() <= &target {
            break candidate;
        }
        counter += 1;
        assert!(
            counter < 100_000_000,
            "PoW didn't converge in {counter} attempts — target way too tight?"
        );
    };

    let mut input = empty_input(ACTION_CRAFT_WOOD, chain.state_root());
    input.inputs.push(log_input);
    input.new_objects.push(wood.clone());

    let journal = validate(&input);

    assert_eq!(journal.nullifiers.len(), 1);
    assert_eq!(journal.nullifiers[0], compute_nullifier(&log));
    assert_eq!(journal.state_root_hash, input.state_root.hash());
}

#[test]
#[should_panic(expected = "obj commitment not in source_tx.live_root")]
fn craft_wood_rejects_un_grounded_input() {
    let chain = MockChain::new();
    // Build an input whose grounding proofs don't actually belong to a
    // canonical source tx — simulate a forged Log certificate.
    let log = object! {
        "blueprint" => "Log",
        "key" => sha256(b"forged-log"),
    };
    let bogus_proof = MerkleProof {
        siblings: vec![Hash::default(); txlib_core::merkle::SMT_DEPTH],
    };
    let bogus_input = InputObject {
        obj: log.clone(),
        source_tx_action_id: ACTION_FIND_LOG,
        source_tx_live_root: sha256(b"unrelated-root"),
        source_tx_nullifiers_root: empty_root(),
        source_tx_action_nonce: sha256(b"forged-nonce"),
        live_inclusion_proof: bogus_proof.clone(),
        tx_inclusion_proof: bogus_proof,
    };

    let wood = object! {
        "blueprint" => "Wood",
        "key" => sha256(b"wk"),
    };

    let mut input = empty_input(ACTION_CRAFT_WOOD, chain.state_root());
    input.inputs.push(bogus_input);
    input.new_objects.push(wood);

    validate(&input);
}

#[test]
fn craft_sticks_pipeline() {
    let mut chain = MockChain::new();

    let wood = object! {
        "blueprint" => "Wood",
        "key" => sha256(b"wk"),
    };
    let wood_input = prepare_input(&mut chain, wood.clone(), ACTION_CRAFT_WOOD, Vec::new());

    let mut input = empty_input(ACTION_CRAFT_STICKS, chain.state_root());
    input.inputs.push(wood_input);
    input.new_objects.push(object! {
        "blueprint" => "Stick",
        "key" => sha256(b"s1"),
    });
    input.new_objects.push(object! {
        "blueprint" => "Stick",
        "key" => sha256(b"s2"),
    });

    let journal = validate(&input);
    assert_eq!(journal.nullifiers, vec![compute_nullifier(&wood)]);
    assert_eq!(journal.state_root_hash, input.state_root.hash());
}

#[test]
fn use_wood_pick_full_pipeline_with_vdf() {
    let mut chain = MockChain::new();

    let old_pick = object! {
        "blueprint" => "WoodPick",
        "key" => sha256(b"old-key"),
        "durability" => 50i64,
    };
    let pick_input = prepare_input(&mut chain, old_pick.clone(), ACTION_CRAFT_WOOD_PICK, Vec::new());

    let mut new_pick = object! {
        "blueprint" => "WoodPick",
        "key" => sha256(b"new-key"),
        "durability" => 49i64,
    };
    let vdf_in = new_pick.commitment();
    let work = run_vdf(craft_actions::actions::USE_WOOD_PICK_VDF_ITERS, vdf_in);
    new_pick.insert("work", work);

    let mut input = empty_input(ACTION_USE_WOOD_PICK, chain.state_root());
    input.inputs.push(pick_input);
    input.new_objects.push(new_pick.clone());
    input.intro_witnesses.push(IntroWitness::Vdf {
        iters: craft_actions::actions::USE_WOOD_PICK_VDF_ITERS,
        input: vdf_in,
        output: work,
    });

    let journal = validate(&input);
    assert_eq!(journal.nullifiers, vec![compute_nullifier(&old_pick)]);

    // Same action twice on the same input would collide on tx_final. Sanity.
    let nonce = action_nonce(ACTION_USE_WOOD_PICK, &[new_pick.commitment()]);
    let expected = Tx {
        action_id: ACTION_USE_WOOD_PICK,
        live_root: set_smt_root(&[new_pick.commitment()]),
        nullifiers_root: set_smt_root(&[compute_nullifier(&old_pick)]),
        action_nonce: nonce,
    }
    .tx_final();
    assert_eq!(journal.tx_final, expected);
}

#[test]
fn journal_borsh_roundtrips() {
    // Confirm the host can decode what the guest commits — same shape both
    // sides see.
    let chain = MockChain::new();
    let mut input = empty_input(ACTION_FIND_LOG, chain.state_root());
    let mut log = object! { "blueprint" => "Log", "key" => sha256(b"k") };
    let vin = log.commitment();
    let w = run_vdf(3, vin);
    log.insert("work", w);
    input.new_objects.push(log);
    input.intro_witnesses.push(IntroWitness::Vdf {
        iters: 3,
        input: vin,
        output: w,
    });

    let journal = validate(&input);
    let bytes = borsh::to_vec(&journal).unwrap();
    let decoded: GuestJournal = borsh::from_slice(&bytes).unwrap();
    assert_eq!(journal, decoded);
}

#[test]
fn guest_input_borsh_roundtrips() {
    // Same check the other direction: the guest decodes what the host encoded.
    let mut chain = MockChain::new();
    let wood = object! {
        "blueprint" => "Wood",
        "key" => sha256(b"k"),
    };
    let wood_input = prepare_input(&mut chain, wood, ACTION_CRAFT_WOOD, Vec::new());
    let mut input = empty_input(ACTION_CRAFT_STICKS, chain.state_root());
    input.inputs.push(wood_input);
    input.new_objects.push(object! { "blueprint" => "Stick", "key" => sha256(b"a") });
    input.new_objects.push(object! { "blueprint" => "Stick", "key" => sha256(b"b") });

    let bytes = borsh::to_vec(&input).unwrap();
    let decoded: GuestInput = borsh::from_slice(&bytes).unwrap();
    assert_eq!(input, decoded);
}
