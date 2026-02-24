use std::collections::HashSet;

use pod2::middleware::{
    containers::{Array, Set},
    hash_values, Hash, Value,
};

pub fn compute_global_state_root(
    txns: &HashSet<Hash>,
    nullifiers: &HashSet<Hash>,
    prior_global_state_roots: &[Hash],
    block_number: i64,
) -> Hash {
    let txns_set = Set::new(txns.iter().map(|h| Value::from(*h)).collect::<HashSet<_>>());
    let nullifiers_set = Set::new(
        nullifiers
            .iter()
            .map(|h| Value::from(*h))
            .collect::<HashSet<_>>(),
    );
    let global_state_roots_array = Array::new(
        prior_global_state_roots
            .iter()
            .map(|h| Value::from(*h))
            .collect::<Vec<_>>(),
    );
    let txn_nullifiers_hash = hash_values(&[Value::from(txns_set), Value::from(nullifiers_set)]);
    let block_number_global_state_roots_hash = hash_values(&[
        Value::from(block_number),
        Value::from(global_state_roots_array),
    ]);
    hash_values(&[
        Value::from(txn_nullifiers_hash),
        Value::from(block_number_global_state_roots_hash),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use pod2::middleware::EMPTY_HASH;

    #[test]
    fn test_deterministic() {
        let txns: HashSet<Hash> = [EMPTY_HASH].into_iter().collect();
        let nullifiers: HashSet<Hash> = HashSet::new();
        let prior: Vec<Hash> = vec![EMPTY_HASH];

        let h1 = compute_global_state_root(&txns, &nullifiers, &prior, 42);
        let h2 = compute_global_state_root(&txns, &nullifiers, &prior, 42);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_empty_state() {
        let txns: HashSet<Hash> = HashSet::new();
        let nullifiers: HashSet<Hash> = HashSet::new();
        let prior: Vec<Hash> = vec![];
        // Should not panic
        let _ = compute_global_state_root(&txns, &nullifiers, &prior, 0);
    }

    #[test]
    fn test_history_affects_hash() {
        let txns: HashSet<Hash> = HashSet::new();
        let nullifiers: HashSet<Hash> = HashSet::new();

        let h1 = compute_global_state_root(&txns, &nullifiers, &[], 1);
        let h2 = compute_global_state_root(&txns, &nullifiers, &[EMPTY_HASH], 1);
        assert_ne!(h1, h2);
    }
}
