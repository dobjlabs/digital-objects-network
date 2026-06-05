use pod2::middleware::{Hash, EMPTY_HASH};
use txlib::StateHeader;

/// The Merkle roots that reopen the canonical persistent containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateRoots {
    /// Root of the persistent global created-object set (a pod2 `Array`).
    pub created: Hash,
    /// Root of the persistent spent-nullifiers set.
    pub nullifiers: Hash,
    /// Root of the prior-state root array committed inside `txlib::StateHeader`.
    pub state_history: Hash,
    /// Root of the full state root history array after appending the current state root.
    pub next_state_history: Hash,
}

impl Default for StateRoots {
    fn default() -> Self {
        Self::empty()
    }
}

impl StateRoots {
    pub fn empty() -> Self {
        Self {
            created: EMPTY_HASH,
            nullifiers: EMPTY_HASH,
            state_history: EMPTY_HASH,
            next_state_history: EMPTY_HASH,
        }
    }
}

/// Non-root metadata tracked alongside the canonical roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateMetadata {
    /// Current canonical state root for this head, if one exists.
    pub current_state_root: Option<Hash>,
    /// Execution block number associated with `current_state_root`.
    pub current_block_number: Option<u32>,
    /// Number of objects in the canonical global created set. The array is
    /// 0-indexed, so this doubles as the next array slot.
    pub created_count: u64,
    /// Number of spent nullifiers in the canonical state.
    pub nullifier_count: u64,
    /// Number of state root entries in the persistent history array.
    pub state_root_count: u64,
}

impl Default for StateMetadata {
    fn default() -> Self {
        Self::empty()
    }
}

impl StateMetadata {
    pub fn empty() -> Self {
        Self {
            current_state_root: None,
            current_block_number: None,
            created_count: 0,
            nullifier_count: 0,
            state_root_count: 0,
        }
    }
}

/// Canonical head snapshot used across sync, query, and storage code.
///
/// `roots` are what reopen the persistent Merkle containers in RocksDB.
/// `metadata` is the auxiliary canonical state carried alongside those roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateHead {
    /// Merkle roots for the canonical persistent containers.
    pub roots: StateRoots,
    /// Non-root metadata associated with the same canonical head.
    pub metadata: StateMetadata,
}

impl Default for StateHead {
    fn default() -> Self {
        Self::empty()
    }
}

impl StateHead {
    pub fn empty() -> Self {
        Self {
            roots: StateRoots::empty(),
            metadata: StateMetadata::empty(),
        }
    }

    pub fn current_state_header(&self) -> Option<StateHeader> {
        self.metadata.current_block_number.map(|block_number| {
            StateHeader::new(
                block_number as i64,
                self.roots.created,
                self.roots.nullifiers,
                self.roots.state_history,
            )
        })
    }
}
