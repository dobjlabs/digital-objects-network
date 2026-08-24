use std::sync::Arc;

use payload::test_state::TestState;
use pod2::middleware::{Hash, StrKey, Value};
use serde::{Serialize, de::DeserializeOwned};
use txlib::{GroundingWitness, StateHeader, erased_key_state};

use crate::engine::{Accepter, ClassDirectory, Initiator, SwapDeps};

/// Serialize and deserialize, so every message crosses a real wire
/// boundary: no Rust value made by one engine reaches the other.
fn wire<T: Serialize + DeserializeOwned>(value: &T) -> T {
    serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
}

fn grounding_witness(state: &TestState, input_commitments: &[Hash]) -> Arc<GroundingWitness> {
    state.build_grounding_witness(
        input_commitments,
        |block_meta, created_root, nullifiers_root, prior_state_history_root, created_proofs| {
            Arc::new(GroundingWitness::new(
                StateHeader::new(
                    block_meta.number as i64,
                    block_meta.timestamp as i64,
                    block_meta.hash,
                    created_root,
                    nullifiers_root,
                    prior_state_history_root,
                ),
                created_proofs,
            ))
        },
    )
}

const CRAFT_SRC: &str = r#"
    fn SpawnFoo(action) {
        var foo = action.output("Foo");
        foo.set([["quality", 7]]);
    }

    fn SpawnBar(action) {
        var bar = action.output("Bar");
        bar.set([["weight", 3]]);
    }
"#;

struct Fixture {
    state: TestState,
    deps: SwapDeps,
    foo: pod2::middleware::containers::Dictionary,
    bar: pod2::middleware::containers::Dictionary,
    foo_class: Hash,
    bar_class: Hash,
}

fn fixture() -> Fixture {
    let sdk = sdk::Sdk::default();
    let module = sdk
        .load_module_from_src_actions(CRAFT_SRC, &["SpawnFoo", "SpawnBar"])
        .unwrap();

    let mut state = TestState::default();
    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("SpawnFoo", vec![]).unwrap();
    let [foo] = res.objs();
    state.apply_tx(
        res.tx.live_commitments().unwrap(),
        res.tx.nullifier_hashes().unwrap(),
    );
    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("SpawnBar", vec![]).unwrap();
    let [bar] = res.objs();
    state.apply_tx(
        res.tx.live_commitments().unwrap(),
        res.tx.nullifier_hashes().unwrap(),
    );

    let deps = SwapDeps {
        modules: vec![
            Arc::new(txlib::predicates::events_module()),
            Arc::new(txlib::predicates::rekey_module()),
            Arc::new(txlib::predicates::module()),
            module.module().clone(),
        ],
        classes: ClassDirectory::from_sdk_module(&module),
        mock: true,
    };
    Fixture {
        state,
        deps,
        foo: foo.obj.clone(),
        bar: bar.obj.clone(),
        foo_class: module.class_hash("Foo").unwrap(),
        bar_class: module.class_hash("Bar").unwrap(),
    }
}

/// The full swap protocol between two engines over serialized
/// messages: a Foo held by the initiator against a Bar held by the
/// accepter, both of SDK-generated classes.
#[test]
fn swap_engines_complete_a_trade() {
    let fx = fixture();

    let initiator = Initiator::new(fx.deps.clone(), fx.foo.clone(), fx.bar_class);
    let accepter = Accepter::new(fx.deps.clone(), fx.bar.clone(), fx.foo_class);

    // Data round.
    let (accepter, accept_msg) = accepter.accept();
    let accept_msg = wire(&accept_msg);
    let witness = grounding_witness(
        &fx.state,
        &[
            accept_msg.accepter_object.old_commitment,
            fx.foo.commitment(),
        ],
    );
    let (initiator, plan_data) = initiator.on_accept(&accept_msg, witness).unwrap();
    let (accepter, plan_ack) = accepter.on_plan_data(&wire(&plan_data)).unwrap();

    // Round 0 and round 1.
    let (initiator, offer_msg) = initiator.on_plan_ack(&wire(&plan_ack)).unwrap();
    let (acceptance_msg, accepter_expect) = accepter.on_offer(wire(&offer_msg)).unwrap();

    // Round 2.
    let outcome = initiator.on_acceptance(wire(&acceptance_msg)).unwrap();

    // Both sides agree on the effect, and it is the recorded one.
    assert_eq!(outcome.expectation.tx_final, accepter_expect.tx_final);
    assert_eq!(outcome.tx.dict().commitment(), outcome.expectation.tx_final);
    for commitment in &outcome.expectation.new_commitments {
        assert!(outcome.tx.live.contains(&Value::from(*commitment)).unwrap());
    }
    for nullifier in &outcome.expectation.nullifiers {
        assert!(
            outcome
                .tx
                .nullifiers
                .contains(&Value::from(*nullifier))
                .unwrap()
        );
    }

    // Each party now controls the other's former object: same fields,
    // different key.
    let initiator_received = &outcome.expectation.received;
    assert_eq!(
        initiator_received.get(&StrKey::from("type")).unwrap(),
        Some(Value::from(fx.bar_class))
    );
    assert_eq!(
        erased_key_state(initiator_received).commitment(),
        erased_key_state(&fx.bar).commitment()
    );
    let accepter_received = &accepter_expect.received;
    assert_eq!(
        accepter_received.get(&StrKey::from("type")).unwrap(),
        Some(Value::from(fx.foo_class))
    );
    assert_eq!(
        erased_key_state(accepter_received).commitment(),
        erased_key_state(&fx.foo).commitment()
    );
    assert_ne!(
        initiator_received.get(&StrKey::from("key")).unwrap(),
        fx.bar.get(&StrKey::from("key")).unwrap()
    );
}

/// A disclosure of the wrong class is rejected at receipt with a
/// readable error, before any proving.
#[test]
fn wrong_class_disclosure_is_rejected() {
    let fx = fixture();

    // The initiator wants a Bar, but the accepter discloses a Foo.
    let initiator = Initiator::new(fx.deps.clone(), fx.foo.clone(), fx.bar_class);
    let accepter = Accepter::new(fx.deps.clone(), fx.foo.clone(), fx.foo_class);
    let (_accepter, accept_msg) = accepter.accept();
    let witness = grounding_witness(&fx.state, &[fx.foo.commitment()]);
    let err = initiator
        .on_accept(&wire(&accept_msg), witness)
        .err()
        .expect("wrong-class disclosure must be rejected");
    assert!(err.to_string().contains("class"), "unexpected error: {err}");
}
