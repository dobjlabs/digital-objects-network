// pub mod examples;
pub mod predicates;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Result, anyhow};
use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    frontend::Operation,
    middleware::{
        EMPTY_VALUE, Hash, Key, NativeOperation, OperationAux, OperationType, Statement, Value,
        containers::{Array, Dictionary, Set},
        hash_values,
    },
};
use pod2utils::{
    dict, dict_define,
    macros::{BuildContext, find_custom_pred_by_name},
    rand_raw_value, set, st_custom,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateRoot {
    pub block_number: i64,
    pub transactions_root: Hash,
    pub nullifiers_root: Hash,
    pub gsrs_root: Hash,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompactStateRootSerde {
    block_number: i64,
    transactions_root: Hash,
    nullifiers_root: Hash,
    gsrs_root: Hash,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyStateRootSerde {
    block_number: i64,
    transactions: Set,
    nullifiers: Set,
    gsrs: Array,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StateRootSerde {
    Compact(CompactStateRootSerde),
    Legacy(LegacyStateRootSerde),
}

impl Serialize for StateRoot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        CompactStateRootSerde {
            block_number: self.block_number,
            transactions_root: self.transactions_root,
            nullifiers_root: self.nullifiers_root,
            gsrs_root: self.gsrs_root,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StateRoot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match StateRootSerde::deserialize(deserializer)? {
            StateRootSerde::Compact(compact) => Ok(Self {
                block_number: compact.block_number,
                transactions_root: compact.transactions_root,
                nullifiers_root: compact.nullifiers_root,
                gsrs_root: compact.gsrs_root,
            }),
            StateRootSerde::Legacy(legacy) => Ok(Self {
                block_number: legacy.block_number,
                transactions_root: legacy.transactions.commitment(),
                nullifiers_root: legacy.nullifiers.commitment(),
                gsrs_root: legacy.gsrs.commitment(),
            }),
        }
    }
}

impl StateRoot {
    /// Construct a `StateRoot` from root commitments.
    pub fn from_roots(
        block_number: i64,
        transactions_root: Hash,
        nullifiers_root: Hash,
        gsrs_root: Hash,
    ) -> Self {
        Self {
            block_number,
            transactions_root,
            nullifiers_root,
            gsrs_root,
        }
    }

    /// Construct a `StateRoot` from raw accumulated sets, a block number, and prior GSR history.
    ///
    /// `txns` and `nullifiers` are sets of hashes; `prior_gsrs` is the ordered array of all
    /// GSRs computed before this one. Calling `.hash()` on the result gives the canonical GSR.
    pub fn new(
        block_number: i64,
        txns: &HashSet<Hash>,
        nullifiers: &HashSet<Hash>,
        prior_gsrs: &[Hash],
    ) -> Self {
        let transactions = Set::new(txns.iter().map(|h| Value::from(*h)).collect());
        let nullifiers = Set::new(nullifiers.iter().map(|h| Value::from(*h)).collect());
        let gsrs = Array::new(prior_gsrs.iter().map(|h| Value::from(*h)).collect());
        Self::from_roots(
            block_number,
            transactions.commitment(),
            nullifiers.commitment(),
            gsrs.commitment(),
        )
    }

    pub fn hash(&self) -> Hash {
        let txn_nullifiers_hash = hash_values(&[
            Value::from(self.transactions_root),
            Value::from(self.nullifiers_root),
        ]);
        let block_number_gsrs_hash =
            hash_values(&[Value::from(self.block_number), Value::from(self.gsrs_root)]);
        hash_values(&[
            Value::from(txn_nullifiers_hash),
            Value::from(block_number_gsrs_hash),
        ])
    }
}

#[derive(Clone, Debug)]
pub struct GroundingWitness {
    pub state_root: StateRoot,
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
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TxSerde {
    live: Set,
    state_root: StateRoot,
    nullifiers: Set,
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
        })
    }
}

impl Tx {
    pub fn dict(&self) -> Dictionary {
        dict!({
            "live" => self.live.clone(),
            "state_root_hash" => self.state_root.hash(),
            "nullifiers" => self.nullifiers.clone()
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
            state_root: Arc::new(grounding_witness.state_root.clone()),
        };

        let state_root_hash = grounding_witness.state_root.hash();
        let [s0, s1, s2, tx_after] = dict_define!({"live" => &inputs_set, "state_root_hash" => &state_root_hash, "nullifiers" => set!()});

        let st_tx_init = st_custom!(
            ctx,
            TxInit() = (
                Equal(dict!(), dict!()),
                DictInsert(s1, s0, "live", inputs_set),
                DictInsert(s2, s1, "state_root_hash", state_root_hash),
                DictInsert(tx_after, s2, "nullifiers", set!()),
                st_inputs_grounded
            )
        )
        .unwrap();
        let st_tx = st_custom!(
            ctx,
            Tx() = (
                st_tx_init,
                Statement::None,
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
        let st_tx = Statement::Custom(tx_pred, vec![Value::from(tx.dict())]);
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
        let block_number = state_root.block_number;
        let txn_nullifiers_hash =
            hash_values(&[Value::from(transactions), Value::from(nullifiers)]);
        let block_number_gsrs_hash = hash_values(&[Value::from(block_number), Value::from(gsrs)]);
        let tx_in_state_root_pred =
            find_custom_pred_by_name(&ctx.modules, "TxInStateRoot").expect("TxInStateRoot exists");
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
            let st_state_root = st_custom!(
                ctx,
                StateRoot() = (
                    HashOf(txn_nullifiers_hash, transactions, nullifiers),
                    HashOf(block_number_gsrs_hash, block_number, gsrs),
                    HashOf(state_root_hash, txn_nullifiers_hash, block_number_gsrs_hash)
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
            let st_tx_in_state_root = ctx
                .builder
                .priv_op(Operation::custom(
                    tx_in_state_root_pred.clone(),
                    [st_state_root, st_tx_membership],
                ))
                .unwrap();
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

    pub fn insert(&mut self, ctx: &mut BuildContext, new: Dictionary) -> Statement {
        let new = Value::from(new);
        let tx_before = self.tx.dict();
        self.tx.live.insert(&new).unwrap();
        let st_tx_inserted = st_custom!(
            ctx,
            TxInserted() = (
                self.st_tx.clone(),
                SetInsert(self.tx.live, (&tx_before, "live"), new),
                DictUpdate(self.tx.dict(), tx_before, "live", self.tx.live)
            )
        )
        .unwrap();
        self.st_tx = st_custom!(
            ctx,
            Tx() = (
                Statement::None,
                Statement::None,
                Statement::None,
                st_tx_inserted.clone()
            )
        )
        .unwrap();
        st_tx_inserted
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

    pub fn delete(&mut self, ctx: &mut BuildContext, obj: Dictionary) -> Statement {
        let st_tx_obj_nullified = self.st_tx_obj_nullified(ctx, &obj);
        let tx_after_nullified = self.tx.dict();
        self.tx.live.delete(&Value::from(obj.commitment())).unwrap();
        let st_tx_deleted = st_custom!(
            ctx,
            TxDeleted() = (
                self.st_tx.clone(),
                st_tx_obj_nullified,
                SetDelete(self.tx.live, (&tx_after_nullified, "live"), obj),
                DictUpdate(self.tx.dict(), tx_after_nullified, "live", self.tx.live)
            )
        )
        .unwrap();
        self.st_tx = st_custom!(
            ctx,
            Tx() = (
                Statement::None,
                st_tx_deleted.clone(),
                Statement::None,
                Statement::None
            )
        )
        .unwrap();
        st_tx_deleted
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
        let st_tx_mutated = st_custom!(
            ctx,
            TxMutated() = (
                self.st_tx.clone(),
                st_tx_obj_nullified,
                SetDelete(live_mid, (&tx_after_nullified, "live"), old),
                SetInsert(self.tx.live, live_mid, new),
                DictUpdate(self.tx.dict(), tx_after_nullified, "live", self.tx.live)
            )
        )
        .unwrap();
        self.st_tx = st_custom!(
            ctx,
            Tx() = (
                Statement::None,
                Statement::None,
                st_tx_mutated.clone(),
                Statement::None
            )
        )
        .unwrap();
        st_tx_mutated
    }

    pub fn finalize(self, ctx: &mut BuildContext) -> (Statement, Tx) {
        let tx_final = self.tx.dict();
        let st = st_custom!(
            ctx,
            TxFinalized() = (
                self.st_tx.clone(),
                DictContains(tx_final, "nullifiers", self.tx.nullifiers),
                DictContains(tx_final, "state_root_hash", self.tx.state_root.hash())
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
    Dictionary::new(map)
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use hex::FromHex;
    use pod2::{
        backends::plonky2::{
            basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver,
        },
        frontend::{MainPod, MultiPodBuilder},
        middleware::{MainPodProver, Params, VDSet},
    };
    use pod2utils::macros::BuildContext;

    use super::*;

    #[derive(Default)]
    struct TestState {
        block_number: i64,
        transactions: HashSet<Hash>,
        nullifiers: HashSet<Hash>,
        gsrs: Vec<Hash>,
    }

    fn test_hash(byte: u8) -> Hash {
        Hash::from_hex(hex::encode([byte; 32])).expect("valid test hash")
    }

    impl TestState {
        fn state_root(&self) -> StateRoot {
            StateRoot::new(
                self.block_number,
                &self.transactions,
                &self.nullifiers,
                &self.gsrs,
            )
        }

        fn grounding_witness(&self, inputs: &[Tx]) -> GroundingWitness {
            let tx_set = Set::new(
                self.transactions
                    .iter()
                    .map(|hash| Value::from(*hash))
                    .collect(),
            );
            let source_tx_proofs = inputs
                .iter()
                .map(|tx| {
                    let tx_hash = tx.dict().commitment();
                    let proof = tx_set.prove(&Value::from(tx_hash)).unwrap();
                    (tx_hash, proof)
                })
                .collect::<HashMap<_, _>>();
            GroundingWitness::new(self.state_root(), source_tx_proofs)
        }

        fn apply_tx(&mut self, tx: &Tx) {
            self.transactions.insert(tx.dict().commitment());
            for nullifier in tx.nullifiers.iter() {
                let nullifier = nullifier.unwrap();
                self.nullifiers.insert(Hash(nullifier.raw().0));
            }
            self.block_number += 1;
        }
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
    fn state_root_compact_hash_matches_legacy_commitments() {
        let txns = [test_hash(1), test_hash(2)]
            .into_iter()
            .collect::<HashSet<_>>();
        let nullifiers = [test_hash(3)].into_iter().collect::<HashSet<_>>();
        let prior_gsrs = vec![test_hash(4), test_hash(5)];

        let compact = StateRoot::new(7, &txns, &nullifiers, &prior_gsrs);
        let legacy_txs = Set::new(txns.iter().map(|hash| Value::from(*hash)).collect());
        let legacy_nullifiers =
            Set::new(nullifiers.iter().map(|hash| Value::from(*hash)).collect());
        let legacy_gsrs = Array::new(prior_gsrs.iter().map(|hash| Value::from(*hash)).collect());
        let legacy_hash = hash_values(&[
            Value::from(hash_values(&[
                Value::from(legacy_txs.commitment()),
                Value::from(legacy_nullifiers.commitment()),
            ])),
            Value::from(hash_values(&[
                Value::from(7_i64),
                Value::from(legacy_gsrs.commitment()),
            ])),
        ]);
        assert_eq!(compact.hash(), legacy_hash);
    }

    #[test]
    fn state_root_deserializes_legacy_container_shape() {
        let txs = Set::new([Value::from(test_hash(1))].into_iter().collect());
        let nullifiers = Set::new([Value::from(test_hash(2))].into_iter().collect());
        let gsrs = Array::new(vec![Value::from(test_hash(3))]);
        let legacy = serde_json::json!({
            "blockNumber": 9,
            "transactions": txs,
            "nullifiers": nullifiers,
            "gsrs": gsrs,
        });
        let decoded: StateRoot = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.block_number, 9);
        assert_eq!(decoded.transactions_root, txs.commitment());
        assert_eq!(decoded.nullifiers_root, nullifiers.commitment());
        assert_eq!(decoded.gsrs_root, gsrs.commitment());
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

        let mut tx_builder = TxBuilder::new(&mut ctx, &[], Arc::new(state.grounding_witness(&[])));
        let obj0 = new_obj();
        tx_builder.insert(&mut ctx, obj0.clone());
        let (st, tx0) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st).unwrap();

        let tx_pod = prove(ctx.builder, prover);
        tx_pod.pod.verify().unwrap();
        state.apply_tx(&tx0);

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
            Arc::new(state.grounding_witness(&[inputs[0].1.clone()])),
        );
        let mut obj1 = obj0.clone();
        obj1.insert(&Key::from("foo"), &Value::from("bar")).unwrap();
        tx_builder.mutate(&mut ctx, obj1.clone(), obj0);
        let (st, tx1) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st).unwrap();

        let tx_pod = prove(ctx.builder, prover);
        tx_pod.pod.verify().unwrap();
        state.apply_tx(&tx1);

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
            Arc::new(state.grounding_witness(&[inputs[0].1.clone()])),
        );
        tx_builder.delete(&mut ctx, obj1);
        let (st, tx2) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st).unwrap();

        let tx_pod = prove(ctx.builder, prover);
        tx_pod.pod.verify().unwrap();
        state.apply_tx(&tx2);
    }
}
