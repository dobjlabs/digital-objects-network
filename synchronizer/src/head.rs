use txlib_core::Hash;
use txlib_core::merkle_store::empty_root;
use txlib_core::tx::StateRoot;

/// The Merkle roots that reopen the canonical persistent containers.
///
/// `transactions` and `nullifiers` are SMT roots over the
/// SHA-256 sparse Merkle tree maintained in RocksDB.
///
/// `state_root_gsrs` and `gsr_history` track the GSR-history hash chain:
/// `state_root_gsrs` is the chain hash *before* the current slot's GSR was
/// appended (committed inside [`StateRoot`] for grounding), and
/// `gsr_history` is the chain hash *after* the append (advances by one per
/// canonical slot). The chain recipe is in [`crate::state_machine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalRoots {
    pub transactions: Hash,
    pub nullifiers: Hash,
    pub state_root_gsrs: Hash,
    pub gsr_history: Hash,
}

impl Default for CanonicalRoots {
    fn default() -> Self {
        Self::empty()
    }
}

impl CanonicalRoots {
    /// Empty SMT roots for the sets, zero for the GSR history chain.
    pub fn empty() -> Self {
        Self {
            transactions: empty_root(),
            nullifiers: empty_root(),
            state_root_gsrs: Hash::default(),
            gsr_history: Hash::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadMetadata {
    pub current_gsr: Option<Hash>,
    pub current_block_number: Option<u32>,
    pub tx_count: u64,
    pub nullifier_count: u64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalHead {
    pub roots: CanonicalRoots,
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

    /// Reconstruct the [`StateRoot`] used inside the receipt's grounding
    /// proof. Returns `None` if no slot has been processed yet
    /// (`current_block_number` is unset).
    pub fn current_state_root(&self) -> Option<StateRoot> {
        self.metadata.current_block_number.map(|block_number| {
            StateRoot::new(
                block_number as i64,
                self.roots.transactions,
                self.roots.nullifiers,
                self.roots.state_root_gsrs,
            )
        })
    }
}
