// pub mod examples;
pub mod predicates;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use pod2::middleware::{
    EMPTY_VALUE, Hash, Key, Statement, Value,
    containers::{Array, Dictionary, Set},
    hash_values,
};
use pod2utils::{dict, dict_define, macros::BuildContext, rand_raw_value, set, st_custom};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateRoot {
    pub block_number: i64,
    pub transactions: Set,
    pub nullifiers: Set,
    pub gsrs: Array,
}

impl StateRoot {
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
        Self {
            block_number,
            transactions: Set::new(txns.iter().map(|h| Value::from(*h)).collect()),
            nullifiers: Set::new(nullifiers.iter().map(|h| Value::from(*h)).collect()),
            gsrs: Array::new(prior_gsrs.iter().map(|h| Value::from(*h)).collect()),
        }
    }

    pub fn hash(&self) -> Hash {
        let txn_nullifiers_hash = hash_values(&[
            Value::from(self.transactions.clone()),
            Value::from(self.nullifiers.clone()),
        ]);
        let block_number_gsrs_hash = hash_values(&[
            Value::from(self.block_number),
            Value::from(self.gsrs.clone()),
        ]);
        hash_values(&[
            Value::from(txn_nullifiers_hash),
            Value::from(block_number_gsrs_hash),
        ])
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

pub fn object_key_hash(obj: &Dictionary) -> Result<Hash, String> {
    let key = obj
        .get(&Key::from("key"))
        .cloned()
        .map_err(|err| format!("object missing required key field: {err}"))?;
    Ok(hash_values(&[Value::from(obj.commitment()), key]))
}

pub fn object_nullifier_from_key_hash(obj_key_hash: Hash) -> Hash {
    hash_values(&[
        Value::from(obj_key_hash),
        Value::from(OBJECT_NULLIFIER_VERSION),
    ])
}

pub fn object_nullifier_hash(obj: &Dictionary) -> Result<Hash, String> {
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
        state_root: Arc<StateRoot>,
    ) -> Self {
        let (st_inputs_grounded, inputs_set) = Self::st_inputs_grounded(ctx, inputs, &state_root);

        let tx = Tx {
            live: inputs_set.clone(),
            nullifiers: set!(),
            state_root: state_root.clone(),
        };

        let state_root_hash = state_root.hash();
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
        state_root: &StateRoot,
    ) -> (Statement, Set) {
        let state_root_hash = state_root.hash();
        let transactions = state_root.transactions.clone();
        let nullifiers = state_root.nullifiers.clone();
        let gsrs = state_root.gsrs.clone();
        let block_number = state_root.block_number;
        let txn_nullifiers_hash = hash_values(&[
            Value::from(transactions.clone()),
            Value::from(nullifiers.clone()),
        ]);
        let block_number_gsrs_hash =
            hash_values(&[Value::from(block_number), Value::from(gsrs.clone())]);
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
            let st_tx_in_state_root = st_custom!(
                ctx,
                TxInStateRoot() = (st_state_root, SetContains(transactions, source_tx.dict()))
            )
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

    use pod2::{
        backends::plonky2::{
            basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver,
        },
        frontend::{MainPod, MultiPodBuilder},
        middleware::{MainPodProver, Params, VDSet},
    };
    use pod2utils::{macros::BuildContext, set};

    use super::*;

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
        assert!(err.contains("missing required key field"));
    }

    fn prove(builder: MultiPodBuilder, prover: &dyn MainPodProver) -> MainPod {
        let solution = builder.solve().unwrap();
        solution.prove(prover).unwrap().pods.pop().unwrap()
    }

    #[test]
    fn test_tx_builder() {
        let txlib_mod = crate::predicates::module();
        let modules = vec![Arc::new(txlib_mod)];

        let mut state_root = StateRoot {
            block_number: 0,
            transactions: set!(),
            nullifiers: set!(),
            gsrs: Array::new(vec![]),
        };

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

        let mut tx_builder = TxBuilder::new(&mut ctx, &[], Arc::new(state_root.clone()));
        let obj0 = new_obj();
        tx_builder.insert(&mut ctx, obj0.clone());
        let (st, tx0) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st).unwrap();

        let tx_pod = prove(ctx.builder, prover);
        tx_pod.pod.verify().unwrap();

        state_root
            .transactions
            .insert(&Value::from(tx0.dict()))
            .unwrap();
        for nullifier in tx0.nullifiers.set() {
            state_root.transactions.insert(nullifier).unwrap();
        }

        // Mutate
        let builder = MultiPodBuilder::new(&params, vd_set);
        let mut ctx = BuildContext {
            builder,
            modules: modules.clone(),
        };

        let inputs = vec![(obj0.clone(), tx0)];
        let mut tx_builder = TxBuilder::new(&mut ctx, &inputs, Arc::new(state_root.clone()));
        let mut obj1 = obj0.clone();
        obj1.insert(&Key::from("foo"), &Value::from("bar")).unwrap();
        tx_builder.mutate(&mut ctx, obj1.clone(), obj0);
        let (st, tx1) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st).unwrap();

        let tx_pod = prove(ctx.builder, prover);
        tx_pod.pod.verify().unwrap();

        state_root
            .transactions
            .insert(&Value::from(tx1.dict()))
            .unwrap();
        for nullifier in tx1.nullifiers.set() {
            state_root.transactions.insert(nullifier).unwrap();
        }

        // Delete
        let builder = MultiPodBuilder::new(&params, vd_set);
        let mut ctx = BuildContext {
            builder,
            modules: modules.clone(),
        };

        let inputs = vec![(obj1.clone(), tx1)];
        let mut tx_builder = TxBuilder::new(&mut ctx, &inputs, Arc::new(state_root.clone()));
        tx_builder.delete(&mut ctx, obj1);
        let (st, tx2) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st).unwrap();

        let tx_pod = prove(ctx.builder, prover);
        tx_pod.pod.verify().unwrap();

        state_root
            .transactions
            .insert(&Value::from(tx2.dict()))
            .unwrap();
        for nullifier in tx2.nullifiers.set() {
            state_root.transactions.insert(nullifier).unwrap();
        }
    }
}
