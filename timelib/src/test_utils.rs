use pod2::middleware::{
    Hash, Value,
    containers::{Array, Set},
    hash_values,
};

/// Pre-computed components of a mock Global State Root.
pub struct MockStateRoot {
    pub hash: Hash,
    pub block_number: i64,
    pub txs: Set,
    pub nullifiers: Set,
    pub gsrs: Array,
    pub tx_nullifiers_hash: Hash,
    pub block_number_gsrs_hash: Hash,
}

impl MockStateRoot {
    pub fn new(block_number: i64, txs: Set, nullifiers: Set, gsrs: Array) -> Self {
        let tx_nullifiers_hash =
            hash_values(&[Value::from(txs.clone()), Value::from(nullifiers.clone())]);
        let block_number_gsrs_hash =
            hash_values(&[Value::from(block_number), Value::from(gsrs.clone())]);
        let hash = hash_values(&[
            Value::from(tx_nullifiers_hash),
            Value::from(block_number_gsrs_hash),
        ]);
        Self {
            hash,
            block_number,
            txs,
            nullifiers,
            gsrs,
            tx_nullifiers_hash,
            block_number_gsrs_hash,
        }
    }
}

/// Computes the txlib nullifier hash for an object given the object value and its key field value.
pub fn object_nullifier(obj: Value, key: Value) -> Hash {
    let keyed = hash_values(&[obj, key]);
    hash_values(&[Value::from(keyed), Value::from("txlib-nullifier-v1")])
}
