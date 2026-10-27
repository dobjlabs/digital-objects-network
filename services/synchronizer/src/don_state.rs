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
    pub number: u64,
    pub timestamp: u64,
    pub hash: B256,
}

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

/// The Digital Objects Network (DON) state derived at one execution block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DonState {
    pub roots: StateRoots,
    pub metadata: StateMetadata,
}

impl DonState {
    pub fn state_header(&self) -> StateHeader {
        state_header(
            self.metadata.block,
            self.roots.created,
            self.roots.nullifiers,
            self.roots.prior_state_history,
        )
    }
}

/// The `StateHeader` of a state derived at `block` with the given roots; its hash is the state's
/// root.
pub fn state_header(
    block: ExecutionBlock,
    created_root: Hash,
    nullifiers_root: Hash,
    prior_state_history_root: Hash,
) -> StateHeader {
    StateHeader::new(
        block.number as i64,
        block.timestamp as i64,
        b256_to_hash(block.hash),
        created_root,
        nullifiers_root,
        prior_state_history_root,
    )
}

/// Outcome of one blob of a target transaction during slot derivation.
#[derive(Debug, Default)]
pub struct BlobStatus {
    /// `None` until the blob parses as a `TxFinalized` payload.
    pub tx_final: Option<Hash>,
    /// Why the blob was skipped or rejected; `None` when it was applied.
    pub error: Option<String>,
}

/// Outcome of one target transaction during slot derivation.
#[derive(Debug, Default)]
pub struct TxStatus {
    pub tx_hash: B256,
    /// One entry per blob, in the transaction's blob order.
    pub blobs: Vec<BlobStatus>,
}
