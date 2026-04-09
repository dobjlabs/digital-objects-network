// pub mod examples;
pub mod predicates;
use std::{collections::HashMap, sync::Arc};

use anyhow::{Result, anyhow};
use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    frontend::Operation,
    middleware::{
        EMPTY_VALUE, Hash, Key, NativeOperation, OperationAux, OperationType, Statement, Value,
        containers::{Dictionary, Set},
        hash_values,
    },
};
use pod2utils::{dict, dict_define, macros::BuildContext, rand_raw_value, set, st_custom};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Compact committed view of canonical app state used for grounding transactions.
///
/// This struct does not carry full containers. It stores only the root commitments needed to
/// recompute the canonical global state root hash and to verify synchronizer-supplied proofs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateRoot {
    /// Execution block number this state root is anchored to.
    pub block_number: i64,
    /// Root of the canonical transactions set.
    pub transactions_root: Hash,
    /// Root of the canonical spent-nullifiers set.
    pub nullifiers_root: Hash,
    /// Root of the prior-GSR history array committed into this state root.
    pub gsrs_root: Hash,
    /// Root of the canonical public objects Merkle Dictionary.
    pub public_objects_root: Hash,
}

impl StateRoot {
    /// Construct a `StateRoot` from committed root values.
    pub fn new(
        block_number: i64,
        transactions_root: Hash,
        nullifiers_root: Hash,
        gsrs_root: Hash,
        public_objects_root: Hash,
    ) -> Self {
        Self {
            block_number,
            transactions_root,
            nullifiers_root,
            gsrs_root,
            public_objects_root,
        }
    }

    /// Hash structure: H(H(txns_root, nullifiers_root), H(H(block_number, gsrs_root), public_objects_root))
    pub fn hash(&self) -> Hash {
        let txn_nullifiers_hash = hash_values(&[
            Value::from(self.transactions_root),
            Value::from(self.nullifiers_root),
        ]);
        let block_number_gsrs_hash =
            hash_values(&[Value::from(self.block_number), Value::from(self.gsrs_root)]);
        let block_gsrs_pubobj_hash = hash_values(&[
            Value::from(block_number_gsrs_hash),
            Value::from(self.public_objects_root),
        ]);
        hash_values(&[
            Value::from(txn_nullifiers_hash),
            Value::from(block_gsrs_pubobj_hash),
        ])
    }
}

/// Proof-bearing grounding data required to build a new transaction.
///
/// Callers use `state_root` as the committed global context and `source_tx_proofs` to prove that
/// each consumed source transaction is present in `state_root.transactions_root`.
#[derive(Clone, Debug)]
pub struct GroundingWitness {
    /// Canonical state root the new transaction is grounded against.
    pub state_root: StateRoot,
    /// Merkle proofs for source transaction inclusion keyed by source tx hash.
    pub source_tx_proofs: HashMap<Hash, MerkleProof>,
}

impl GroundingWitness {
    pub fn new(state_root: StateRoot, source_tx_proofs: HashMap<Hash, MerkleProof>) -> Self {
        Self {
            state_root,
            source_tx_proofs,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tx {
    pub live: Set,
    pub state_root: Arc<StateRoot>,
    pub nullifiers: Set,
    pub public_outputs: Set,
    pub public_inputs: Set,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TxSerde {
    live: Set,
    state_root: StateRoot,
    nullifiers: Set,
    public_outputs: Set,
    public_inputs: Set,
}

impl Serialize for Tx {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        TxSerde {
            live: self.live.clone(),
            state_root: (*self.state_root).clone(),
            nullifiers: self.nullifiers.clone(),
            public_outputs: self.public_outputs.clone(),
            public_inputs: self.public_inputs.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Tx {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let payload = TxSerde::deserialize(deserializer)?;
        Ok(Self {
            live: payload.live,
            state_root: Arc::new(payload.state_root),
            nullifiers: payload.nullifiers,
            public_outputs: payload.public_outputs,
            public_inputs: payload.public_inputs,
        })
    }
}

impl Tx {
    pub fn dict(&self) -> Dictionary {
        dict!({
            "live" => self.live.clone(),
            "state_root_hash" => self.state_root.hash(),
            "nullifiers" => self.nullifiers.clone(),
            "public_outputs" => self.public_outputs.clone(),
            "public_inputs" => self.public_inputs.clone()
        })
    }
}

pub fn rekey(obj: &mut Dictionary) {
    obj.update(&Key::from("key"), &Value::from(rand_raw_value()))
        .unwrap();
}

const OBJECT_NULLIFIER_VERSION: &str = "txlib-nullifier-v1";

pub fn object_key_hash(obj: &Dictionary) -> Result<Hash> {
    let key = obj
        .get(&Key::from("key"))?
        .ok_or_else(|| anyhow!("object missing required key field"))?;
    Ok(hash_values(&[Value::from(obj.commitment()), key]))
}

pub fn object_nullifier_from_key_hash(obj_key_hash: Hash) -> Hash {
    hash_values(&[
        Value::from(obj_key_hash),
        Value::from(OBJECT_NULLIFIER_VERSION),
    ])
}

pub fn object_nullifier_hash(obj: &Dictionary) -> Result<Hash> {
    object_key_hash(obj).map(object_nullifier_from_key_hash)
}

pub struct TxBuilder {
    st_tx: Statement,
    pub tx: Tx,
}

impl TxBuilder {
    pub fn new(
        ctx: &mut BuildContext,
        inputs: &[(Dictionary, Tx)],
        grounding_witness: Arc<GroundingWitness>,
    ) -> Self {
        let (st_inputs_grounded, inputs_set) =
            Self::st_inputs_grounded(ctx, inputs, &grounding_witness);

        let tx = Tx {
            live: inputs_set.clone(),
            nullifiers: set!(),
            public_outputs: set!(),
            public_inputs: set!(),
            state_root: Arc::new(grounding_witness.state_root.clone()),
        };

        let state_root_hash = grounding_witness.state_root.hash();
        let [s0, s1, s2, s3, s4, tx_after] = dict_define!({
            "live" => &inputs_set,
            "state_root_hash" => &state_root_hash,
            "nullifiers" => set!(),
            "public_outputs" => set!(),
            "public_inputs" => set!()
        });

        let st_tx_init = st_custom!(
            ctx,
            TxInit() = (
                Equal(dict!(), dict!()),
                DictInsert(s1, s0, "live", inputs_set),
                DictInsert(s2, s1, "state_root_hash", state_root_hash),
                DictInsert(s3, s2, "nullifiers", set!()),
                DictInsert(s4, s3, "public_outputs", set!()),
                DictInsert(tx_after, s4, "public_inputs", set!()),
                st_inputs_grounded
            )
        )
        .unwrap();
        let st_tx = st_custom!(
            ctx,
            Tx() = (
                st_tx_init,
                Statement::None,
                Statement::None
            )
        )
        .unwrap();
        Self { st_tx, tx }
    }

    pub fn new_from_tx(ctx: &BuildContext, tx: Tx) -> Self {
        let tx_pred = ctx
            .modules
            .iter()
            .find_map(|module| module.predicate_ref_by_name("Tx"))
            .unwrap();
        let st_tx = Statement::Custom(tx_pred, vec![pod2::middleware::ValueRef::Literal(Value::from(tx.dict()))]);
        Self { st_tx, tx }
    }

    pub fn st_tx(&self) -> &Statement {
        &self.st_tx
    }

    fn st_inputs_grounded(
        ctx: &mut BuildContext,
        inputs: &[(Dictionary, Tx)],
        grounding_witness: &GroundingWitness,
    ) -> (Statement, Set) {
        let state_root = &grounding_witness.state_root;
        let state_root_hash = state_root.hash();
        let transactions = state_root.transactions_root;
        let nullifiers = state_root.nullifiers_root;
        let gsrs = state_root.gsrs_root;
        let public_objects = state_root.public_objects_root;
        let block_number = state_root.block_number;
        let txn_nullifiers_hash =
            hash_values(&[Value::from(transactions), Value::from(nullifiers)]);
        let block_number_gsrs_hash = hash_values(&[Value::from(block_number), Value::from(gsrs)]);
        let block_gsrs_pubobj_hash = hash_values(&[
            Value::from(block_number_gsrs_hash),
            Value::from(public_objects),
        ]);
        let mut st = st_custom!(
            ctx,
            InputsGrounded(state_root_hash = state_root_hash) =
                (Equal(set!(), set!()), Statement::None)
        )
        .unwrap();
        let mut prev_inputs_set = set!();
        for (obj, source_tx) in inputs {
            let mut inputs_set = prev_inputs_set.clone();
            inputs_set.insert(&Value::from(obj.clone())).unwrap();
            // StateRoot has 5 public args; public_objects_root is private (hash-chain verified).
            // The st_custom! call provides the 4 HashOf sub-statements that prove the hash tree.
            // public_objects_root is implicitly provided as a private witness by the prover.
            let st_state_root = st_custom!(
                ctx,
                StateRoot() = (
                    HashOf(txn_nullifiers_hash, transactions, nullifiers),
                    HashOf(block_number_gsrs_hash, block_number, gsrs),
                    HashOf(block_gsrs_pubobj_hash, block_number_gsrs_hash, public_objects),
                    HashOf(state_root_hash, txn_nullifiers_hash, block_gsrs_pubobj_hash)
                )
            )
            .unwrap();
            let source_tx_hash = source_tx.dict().commitment();
            let source_tx_proof = grounding_witness
                .source_tx_proofs
                .get(&source_tx_hash)
                .cloned()
                .expect("missing source tx proof in grounding witness");
            let st_tx_membership = ctx
                .builder
                .priv_op(Operation(
                    OperationType::Native(NativeOperation::SetContainsFromEntries),
                    vec![transactions.into(), source_tx.dict().into()],
                    OperationAux::MerkleProof(source_tx_proof),
                ))
                .unwrap();
            let st_tx_in_state_root =
                st_custom!(ctx, TxInStateRoot() = (st_state_root, st_tx_membership)).unwrap();
            let st_rec = st_custom!(
                ctx,
                InputsGroundedRecursive() = (
                    st_tx_in_state_root,
                    SetContains((&source_tx.dict(), "live"), obj),
                    SetInsert(inputs_set, prev_inputs_set, obj),
                    st
                )
            )
            .unwrap();
            prev_inputs_set = inputs_set;

            st = st_custom!(ctx, InputsGrounded() = (Statement::None, st_rec)).unwrap();
        }
        (st, prev_inputs_set)
    }

    fn st_tx_obj_nullified(&mut self, ctx: &mut BuildContext, obj: &Dictionary) -> Statement {
        let obj_key_hash = object_key_hash(obj).expect("tx object must include required key field");
        let obj_nullifier = object_nullifier_from_key_hash(obj_key_hash);
        let tx_before = self.tx.dict();
        self.tx
            .nullifiers
            .insert(&Value::from(obj_nullifier))
            .unwrap();
        st_custom!(
            ctx,
            TxObjectStateNullified(tx_before = tx_before) = (
                HashOf(obj_key_hash, obj, (obj, "key")),
                HashOf(obj_nullifier, obj_key_hash, OBJECT_NULLIFIER_VERSION),
                SetInsert(
                    self.tx.nullifiers,
                    (&tx_before, "nullifiers"),
                    obj_nullifier
                ),
                DictUpdate(self.tx.dict(), tx_before, "nullifiers", self.tx.nullifiers)
            )
        )
        .unwrap()
    }

    // -----------------------------------------------------------------------
    // Private insert / delete / mutate
    // -----------------------------------------------------------------------

    pub fn insert(&mut self, ctx: &mut BuildContext, new: Dictionary) -> Statement {
        let new = Value::from(new);
        let tx_before = self.tx.dict();
        self.tx.live.insert(&new).unwrap();
        let st = st_custom!(
            ctx,
            TxInsertedPrivate() = (
                self.st_tx.clone(),
                DictContains(new, "public", false),
                SetInsert(self.tx.live, (&tx_before, "live"), new),
                DictUpdate(self.tx.dict(), tx_before, "live", self.tx.live)
            )
        )
        .unwrap();
        let st_private_op = st_custom!(
            ctx,
            TxPrivateOp() = (Statement::None, Statement::None, st.clone())
        )
        .unwrap();
        self.st_tx = st_custom!(
            ctx,
            Tx() = (Statement::None, st_private_op, Statement::None)
        )
        .unwrap();
        st
    }

    pub fn delete(&mut self, ctx: &mut BuildContext, obj: Dictionary) -> Statement {
        let st_tx_obj_nullified = self.st_tx_obj_nullified(ctx, &obj);
        let tx_after_nullified = self.tx.dict();
        self.tx.live.delete(&Value::from(obj.commitment())).unwrap();
        let st = st_custom!(
            ctx,
            TxDeletedPrivate() = (
                self.st_tx.clone(),
                DictContains(obj, "public", false),
                st_tx_obj_nullified,
                SetDelete(self.tx.live, (&tx_after_nullified, "live"), obj),
                DictUpdate(self.tx.dict(), tx_after_nullified, "live", self.tx.live)
            )
        )
        .unwrap();
        let st_private_op = st_custom!(
            ctx,
            TxPrivateOp() = (st.clone(), Statement::None, Statement::None)
        )
        .unwrap();
        self.st_tx = st_custom!(
            ctx,
            Tx() = (Statement::None, st_private_op, Statement::None)
        )
        .unwrap();
        st
    }

    pub fn mutate(
        &mut self,
        ctx: &mut BuildContext,
        new: Dictionary,
        old: Dictionary,
    ) -> Statement {
        let st_tx_obj_nullified = self.st_tx_obj_nullified(ctx, &old);
        let tx_after_nullified = self.tx.dict();
        self.tx.live.delete(&Value::from(old.commitment())).unwrap();
        let live_mid = self.tx.live.clone();
        self.tx.live.insert(&Value::from(new.clone())).unwrap();
        let st = st_custom!(
            ctx,
            TxMutatedPrivate() = (
                self.st_tx.clone(),
                DictContains(old, "public", false),
                DictContains(new, "public", false),
                st_tx_obj_nullified,
                SetDelete(live_mid, (&tx_after_nullified, "live"), old),
                SetInsert(self.tx.live, live_mid, new),
                DictUpdate(self.tx.dict(), tx_after_nullified, "live", self.tx.live)
            )
        )
        .unwrap();
        let st_private_op = st_custom!(
            ctx,
            TxPrivateOp() = (Statement::None, st.clone(), Statement::None)
        )
        .unwrap();
        self.st_tx = st_custom!(
            ctx,
            Tx() = (Statement::None, st_private_op, Statement::None)
        )
        .unwrap();
        st
    }

    // -----------------------------------------------------------------------
    // Public insert / delete / mutate
    // -----------------------------------------------------------------------

    pub fn insert_public(&mut self, ctx: &mut BuildContext, new: Dictionary) -> Statement {
        let new_val = Value::from(new);
        let tx_before = self.tx.dict();
        self.tx.live.insert(&new_val).unwrap();
        let tx_mid = self.tx.dict();
        self.tx.public_outputs.insert(&new_val).unwrap();
        let st = st_custom!(
            ctx,
            TxInsertedPublic() = (
                self.st_tx.clone(),
                DictContains(new_val, "public", true),
                SetInsert(self.tx.live, (&tx_before, "live"), new_val),
                DictUpdate(tx_mid, tx_before, "live", self.tx.live),
                SetInsert(self.tx.public_outputs, (&tx_mid, "public_outputs"), new_val),
                DictUpdate(self.tx.dict(), tx_mid, "public_outputs", self.tx.public_outputs)
            )
        )
        .unwrap();
        let st_public_op = st_custom!(
            ctx,
            TxPublicOp() = (Statement::None, Statement::None, st.clone())
        )
        .unwrap();
        self.st_tx = st_custom!(
            ctx,
            Tx() = (Statement::None, Statement::None, st_public_op)
        )
        .unwrap();
        st
    }

    pub fn delete_public(&mut self, ctx: &mut BuildContext, obj: Dictionary) -> Statement {
        let st_tx_obj_nullified = self.st_tx_obj_nullified(ctx, &obj);
        let tx_after_nullified = self.tx.dict();
        self.tx.live.delete(&Value::from(obj.commitment())).unwrap();
        let tx_mid = self.tx.dict();
        self.tx
            .public_inputs
            .insert(&Value::from(obj.clone()))
            .unwrap();
        let st = st_custom!(
            ctx,
            TxDeletedPublic() = (
                self.st_tx.clone(),
                DictContains(obj, "public", true),
                st_tx_obj_nullified,
                SetDelete(self.tx.live, (&tx_after_nullified, "live"), obj),
                DictUpdate(tx_mid, tx_after_nullified, "live", self.tx.live),
                SetInsert(self.tx.public_inputs, (&tx_mid, "public_inputs"), obj),
                DictUpdate(self.tx.dict(), tx_mid, "public_inputs", self.tx.public_inputs)
            )
        )
        .unwrap();
        let st_public_op = st_custom!(
            ctx,
            TxPublicOp() = (st.clone(), Statement::None, Statement::None)
        )
        .unwrap();
        self.st_tx = st_custom!(
            ctx,
            Tx() = (Statement::None, Statement::None, st_public_op)
        )
        .unwrap();
        st
    }

    pub fn mutate_public(
        &mut self,
        ctx: &mut BuildContext,
        new: Dictionary,
        old: Dictionary,
    ) -> Statement {
        let st_tx_obj_nullified = self.st_tx_obj_nullified(ctx, &old);
        let tx_after_nullified = self.tx.dict();
        self.tx.live.delete(&Value::from(old.commitment())).unwrap();
        let live_mid = self.tx.live.clone();
        self.tx.live.insert(&Value::from(new.clone())).unwrap();
        let tx_mid1 = self.tx.dict();
        self.tx
            .public_inputs
            .insert(&Value::from(old.clone()))
            .unwrap();
        let tx_mid2 = self.tx.dict();
        self.tx
            .public_outputs
            .insert(&Value::from(new.clone()))
            .unwrap();
        let st = st_custom!(
            ctx,
            TxMutatedPublic() = (
                self.st_tx.clone(),
                DictContains(old, "public", true),
                DictContains(new, "public", true),
                st_tx_obj_nullified,
                SetDelete(live_mid, (&tx_after_nullified, "live"), old),
                SetInsert(self.tx.live, live_mid, new),
                DictUpdate(tx_mid1, tx_after_nullified, "live", self.tx.live),
                SetInsert(self.tx.public_inputs, (&tx_mid1, "public_inputs"), old),
                DictUpdate(tx_mid2, tx_mid1, "public_inputs", self.tx.public_inputs),
                SetInsert(self.tx.public_outputs, (&tx_mid2, "public_outputs"), new),
                DictUpdate(self.tx.dict(), tx_mid2, "public_outputs", self.tx.public_outputs)
            )
        )
        .unwrap();
        let st_public_op = st_custom!(
            ctx,
            TxPublicOp() = (Statement::None, st.clone(), Statement::None)
        )
        .unwrap();
        self.st_tx = st_custom!(
            ctx,
            Tx() = (Statement::None, Statement::None, st_public_op)
        )
        .unwrap();
        st
    }

    // -----------------------------------------------------------------------
    // Finalize
    // -----------------------------------------------------------------------

    pub fn finalize(self, ctx: &mut BuildContext) -> (Statement, Tx) {
        let tx_final = self.tx.dict();
        let st = st_custom!(
            ctx,
            TxFinalized() = (
                self.st_tx.clone(),
                DictContains(tx_final, "nullifiers", self.tx.nullifiers),
                DictContains(tx_final, "state_root_hash", self.tx.state_root.hash()),
                DictContains(tx_final, "public_outputs", self.tx.public_outputs),
                DictContains(tx_final, "public_inputs", self.tx.public_inputs)
            )
        )
        .unwrap();
        (st, self.tx)
    }
}

pub fn new_obj() -> Dictionary {
    let mut map = HashMap::new();
    map.insert(Key::from("key"), Value::from(rand_raw_value()));
    map.insert(Key::from("work"), Value::from(EMPTY_VALUE));
    map.insert(Key::from("public"), Value::from(false));
    Dictionary::new(map)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use common::test_state::TestState;
    use hex::FromHex;
    use pod2::{
        backends::plonky2::{
            basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver,
        },
        frontend::{MainPod, MultiPodBuilder},
        middleware::{MainPodProver, Params, VDSet, containers::Array},
    };
    use pod2utils::macros::BuildContext;

    use super::*;

    fn tx_hash(tx: &Tx) -> Hash {
        tx.dict().commitment()
    }

    fn tx_nullifiers(tx: &Tx) -> Vec<Hash> {
        tx.nullifiers
            .iter()
            .map(|nullifier| {
                let nullifier = nullifier.expect("tx nullifier should decode");
                Hash(nullifier.raw().0)
            })
            .collect()
    }

    fn apply_tx(state: &mut TestState, tx: &Tx) {
        state.apply_tx(tx_hash(tx), tx_nullifiers(tx));
    }

    fn test_hash(byte: u8) -> Hash {
        Hash::from_hex(hex::encode([byte; 32])).expect("valid test hash")
    }

    fn grounding_witness(state: &TestState, inputs: &[Tx]) -> Arc<GroundingWitness> {
        state.build_grounding_witness(
            inputs,
            tx_hash,
            |block_number,
             transactions_root,
             nullifiers_root,
             gsrs_root,
             public_objects_root,
             source_tx_proofs| {
                Arc::new(GroundingWitness::new(
                    StateRoot::new(
                        block_number,
                        transactions_root,
                        nullifiers_root,
                        gsrs_root,
                        public_objects_root,
                    ),
                    source_tx_proofs,
                ))
            },
        )
    }

    #[test]
    fn object_nullifier_hash_matches_key_hash_path() {
        let obj = new_obj();
        let key_hash = object_key_hash(&obj).expect("new_obj should always set key");
        let nullifier = object_nullifier_hash(&obj).expect("new_obj should always set key");
        assert_eq!(nullifier, object_nullifier_from_key_hash(key_hash));
    }

    #[test]
    fn object_nullifier_hash_errors_without_key() {
        let mut obj = new_obj();
        obj.delete(&Key::from("key"))
            .expect("deleting key from dictionary should succeed");
        let err = object_nullifier_hash(&obj).expect_err("missing key must fail");
        assert!(format!("{err}").contains("missing required key field"));
    }

    #[test]
    fn state_root_compact_hash_matches_expected_structure() {
        let txns = [test_hash(1), test_hash(2)]
            .into_iter()
            .collect::<HashSet<_>>();
        let nullifiers = [test_hash(3)].into_iter().collect::<HashSet<_>>();
        let prior_gsrs = vec![test_hash(4), test_hash(5)];
        let pub_objs_root = test_hash(6);

        let txs_set = Set::new(txns.iter().map(|hash| Value::from(*hash)).collect());
        let nullifiers_set =
            Set::new(nullifiers.iter().map(|hash| Value::from(*hash)).collect());
        let gsrs_array = Array::new(prior_gsrs.iter().map(|hash| Value::from(*hash)).collect());
        let compact = StateRoot::new(
            7,
            txs_set.commitment(),
            nullifiers_set.commitment(),
            gsrs_array.commitment(),
            pub_objs_root,
        );
        // H(H(txns, nulls), H(H(block, gsrs), pub_objs))
        let txn_nullifiers_hash = hash_values(&[
            Value::from(txs_set.commitment()),
            Value::from(nullifiers_set.commitment()),
        ]);
        let block_number_gsrs_hash = hash_values(&[
            Value::from(7_i64),
            Value::from(gsrs_array.commitment()),
        ]);
        let block_gsrs_pubobj_hash = hash_values(&[
            Value::from(block_number_gsrs_hash),
            Value::from(pub_objs_root),
        ]);
        let expected_hash = hash_values(&[
            Value::from(txn_nullifiers_hash),
            Value::from(block_gsrs_pubobj_hash),
        ]);
        assert_eq!(compact.hash(), expected_hash);
    }

    #[test]
    fn state_root_serializes_and_deserializes_compact_shape() {
        let original =
            StateRoot::new(9, test_hash(1), test_hash(2), test_hash(3), test_hash(4));
        let encoded = serde_json::to_value(&original).unwrap();
        assert_eq!(encoded["blockNumber"], serde_json::json!(9));
        assert_eq!(
            encoded["transactionsRoot"],
            serde_json::json!(hex::encode([1_u8; 32]))
        );
        assert_eq!(
            encoded["nullifiersRoot"],
            serde_json::json!(hex::encode([2_u8; 32]))
        );
        assert_eq!(
            encoded["gsrsRoot"],
            serde_json::json!(hex::encode([3_u8; 32]))
        );
        assert_eq!(
            encoded["publicObjectsRoot"],
            serde_json::json!(hex::encode([4_u8; 32]))
        );

        let decoded: StateRoot = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, original);
    }

    fn prove(builder: MultiPodBuilder, prover: &dyn MainPodProver) -> MainPod {
        let solution = builder.solve().unwrap();
        solution.prove(prover).unwrap().pods.pop().unwrap()
    }

    #[test]
    fn test_tx_builder() {
        let txlib_mod = crate::predicates::module();
        let modules = vec![Arc::new(txlib_mod)];
        let mut state = TestState::default();

        let mock = true;

        let mock_prover = MockProver {};
        let real_prover = Prover {};
        let (vd_set, prover): (_, &dyn MainPodProver) = if mock {
            (&VDSet::new(&[]), &mock_prover)
        } else {
            let vd_set = &*DEFAULT_VD_SET;
            (vd_set, &real_prover)
        };
        let params = Params::default();

        // Insert
        let builder = MultiPodBuilder::new(&params, vd_set);
        let mut ctx = BuildContext {
            builder,
            modules: modules.clone(),
        };

        let mut tx_builder = TxBuilder::new(&mut ctx, &[], grounding_witness(&state, &[]));
        let obj0 = new_obj();
        tx_builder.insert(&mut ctx, obj0.clone());
        let (st, tx0) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st).unwrap();

        let tx_pod = prove(ctx.builder, prover);
        tx_pod.pod.verify().unwrap();
        apply_tx(&mut state, &tx0);

        // Mutate
        let builder = MultiPodBuilder::new(&params, vd_set);
        let mut ctx = BuildContext {
            builder,
            modules: modules.clone(),
        };

        let inputs = vec![(obj0.clone(), tx0)];
        let mut tx_builder = TxBuilder::new(
            &mut ctx,
            &inputs,
            grounding_witness(&state, &[inputs[0].1.clone()]),
        );
        let mut obj1 = obj0.clone();
        obj1.insert(&Key::from("foo"), &Value::from("bar")).unwrap();
        tx_builder.mutate(&mut ctx, obj1.clone(), obj0);
        let (st, tx1) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st).unwrap();

        let tx_pod = prove(ctx.builder, prover);
        tx_pod.pod.verify().unwrap();
        apply_tx(&mut state, &tx1);

        // Delete
        let builder = MultiPodBuilder::new(&params, vd_set);
        let mut ctx = BuildContext {
            builder,
            modules: modules.clone(),
        };

        let inputs = vec![(obj1.clone(), tx1)];
        let mut tx_builder = TxBuilder::new(
            &mut ctx,
            &inputs,
            grounding_witness(&state, &[inputs[0].1.clone()]),
        );
        tx_builder.delete(&mut ctx, obj1);
        let (st, tx2) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st).unwrap();

        let tx_pod = prove(ctx.builder, prover);
        tx_pod.pod.verify().unwrap();
        apply_tx(&mut state, &tx2);
    }
}
