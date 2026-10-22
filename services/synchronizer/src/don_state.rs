use alloy::primitives::B256;
use pod2::middleware::{Hash, EMPTY_HASH};
use pod2utils::b256_to_hash;
use txlib::StateHeader;

/// The Merkle roots that reopen the persistent containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateRoots {
    /// Root of the persistent global created-object set (a pod2 `Array`).
    pub created: Hash,
    /// Root of the persistent spent-nullifiers set.
    pub nullifiers: Hash,
    /// Root of the prior-state root array committed inside `txlib::StateHeader`.
    pub prior_state_history: Hash,
    /// Root of the full state root history array after appending this state's root.
    pub next_state_history: Hash,
}

impl StateRoots {
    pub fn empty() -> Self {
        Self {
            created: EMPTY_HASH,
            nullifiers: EMPTY_HASH,
            prior_state_history: EMPTY_HASH,
            next_state_history: EMPTY_HASH,
        }
    }
}

/// Execution block that became canonical with a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionBlock {
    pub number: u32,
    pub timestamp: u64,
    pub hash: B256,
}

/// Non-root metadata tracked alongside the state roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateMetadata {
    /// Hash of this state's `StateHeader`.
    pub state_root: Hash,
    /// Execution block the state was derived at, committed inside `state_root`.
    pub block: ExecutionBlock,
    /// Number of objects in the global created set. The array is
    /// 0-indexed, so this doubles as the next array slot.
    pub created_count: u64,
    /// Number of spent nullifiers in the committed state.
    pub nullifier_count: u64,
    /// Number of state root entries in the persistent history array.
    pub state_root_count: u64,
}

/// The Digital Objects Network (DON) state derived at one slot, used across sync, query, and
/// storage code.
///
/// `roots` are what reopen the persistent Merkle containers in RocksDB.
/// `metadata` is the auxiliary state carried alongside those roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DonState {
    /// Merkle roots for the persistent containers.
    pub roots: StateRoots,
    /// Non-root metadata associated with the same state.
    pub metadata: StateMetadata,
}

impl DonState {
    pub fn state_header(&self) -> StateHeader {
        let block = self.metadata.block;
        StateHeader::new(
            block.number as i64,
            block.timestamp as i64,
            b256_to_hash(block.hash),
            self.roots.created,
            self.roots.nullifiers,
            self.roots.prior_state_history,
        )
    }
}
