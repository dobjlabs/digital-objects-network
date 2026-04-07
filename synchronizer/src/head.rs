use pod2::middleware::{Hash, EMPTY_HASH};
use txlib::StateRoot;

/// The Merkle roots that reopen the canonical persistent containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalRoots {
    /// Root of the persistent transactions set.
    pub transactions: Hash,
    /// Root of the persistent spent-nullifiers set.
    pub nullifiers: Hash,
    /// Root of the prior-GSR array committed inside `txlib::StateRoot`.
    pub state_root_gsrs: Hash,
    /// Root of the full GSR history array after appending the current GSR.
    pub gsr_history: Hash,
    /// Root of the persistent public objects Merkle Dictionary.
    pub public_objects: Hash,
}

impl Default for CanonicalRoots {
    fn default() -> Self {
        Self::empty()
    }
}

impl CanonicalRoots {
    pub fn empty() -> Self {
        Self {
            transactions: EMPTY_HASH,
            nullifiers: EMPTY_HASH,
            state_root_gsrs: EMPTY_HASH,
            gsr_history: EMPTY_HASH,
            public_objects: EMPTY_HASH,
        }
    }
}

/// Non-root metadata tracked alongside the canonical roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadMetadata {
    /// Current canonical global state root for this head, if one exists.
    pub current_gsr: Option<Hash>,
    /// Execution block number associated with `current_gsr`.
    pub current_block_number: Option<u32>,
    /// Number of accepted transactions in the canonical state.
    pub tx_count: u64,
    /// Number of spent nullifiers in the canonical state.
    pub nullifier_count: u64,
    /// Number of GSR entries in the persistent history array.
    pub gsr_count: u64,
}

impl Default for HeadMetadata {
    fn default() -> Self {
        Self::empty()
    }
}

impl HeadMetadata {
    pub fn empty() -> Self {
        Self {
            current_gsr: None,
            current_block_number: None,
            tx_count: 0,
            nullifier_count: 0,
            gsr_count: 0,
        }
    }
}

/// Canonical head snapshot used across sync, query, and storage code.
///
/// `roots` are what reopen the persistent Merkle containers in RocksDB.
/// `metadata` is the auxiliary canonical state carried alongside those roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalHead {
    /// Merkle roots for the canonical persistent containers.
    pub roots: CanonicalRoots,
    /// Non-root metadata associated with the same canonical head.
    pub metadata: HeadMetadata,
}

impl Default for CanonicalHead {
    fn default() -> Self {
        Self::empty()
    }
}

impl CanonicalHead {
    pub fn empty() -> Self {
        Self {
            roots: CanonicalRoots::empty(),
            metadata: HeadMetadata::empty(),
        }
    }

    pub fn current_state_root(&self) -> Option<StateRoot> {
        self.metadata.current_block_number.map(|block_number| {
            StateRoot::new(
                block_number as i64,
                self.roots.transactions,
                self.roots.nullifiers,
                self.roots.state_root_gsrs,
                self.roots.public_objects,
            )
        })
    }
}
