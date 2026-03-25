use anyhow::Result;
use pod2::{backends::plonky2::primitives::merkletree::MerkleProof, middleware::Hash};

use crate::{app_db::AppDb, head::CanonicalRoots};

#[derive(Debug, Clone)]
/// Membership proof for a source transaction against the current transactions set root.
pub struct TxMembershipProof {
    /// Source transaction hash the client asked about.
    pub tx_hash: Hash,
    /// Whether the transaction is present in the committed transactions set.
    pub present: bool,
    /// Merkle proof against the current transactions set root.
    pub proof: MerkleProof,
}

#[derive(Debug, Clone)]
/// Proof-bearing result used by txlib to ground action execution.
pub struct GroundingWitnessSnapshot {
    /// Per-source transaction membership proofs under the provided roots.
    pub source_tx_proofs: Vec<TxMembershipProof>,
}

#[derive(Debug, Clone)]
/// Membership result anchored to one caller-provided root set.
pub struct MembershipSnapshot {
    /// Per-request transaction membership bits under `roots.transactions`.
    pub tx_present: Vec<bool>,
    /// Per-request nullifier membership bits under `roots.nullifiers`.
    pub nullifier_present: Vec<bool>,
}

/// Read-side state queries and proof serving for API consumers.
pub struct StateReader {
    app_db: AppDb,
}

impl StateReader {
    pub fn new(app_db: AppDb) -> Self {
        Self { app_db }
    }

    #[cfg(test)]
    pub fn tx_exists(&self, roots: &CanonicalRoots, tx_hash: &Hash) -> Result<bool> {
        Ok(self
            .membership_snapshot(roots, std::slice::from_ref(tx_hash), &[])?
            .tx_present[0])
    }

    #[cfg(test)]
    pub fn nullifier_exists_batch(
        &self,
        roots: &CanonicalRoots,
        nullifiers: &[Hash],
    ) -> Result<Vec<bool>> {
        Ok(self
            .membership_snapshot(roots, &[], nullifiers)?
            .nullifier_present)
    }

    pub fn membership_snapshot(
        &self,
        roots: &CanonicalRoots,
        tx_hashes: &[Hash],
        nullifiers: &[Hash],
    ) -> Result<MembershipSnapshot> {
        Ok(MembershipSnapshot {
            tx_present: self.app_db.tx_exists_batch(roots, tx_hashes)?,
            nullifier_present: self.app_db.nullifier_exists_batch(roots, nullifiers)?,
        })
    }

    pub fn grounding_witness(
        &self,
        roots: &CanonicalRoots,
        source_tx_hashes: &[Hash],
    ) -> Result<GroundingWitnessSnapshot> {
        let source_tx_proofs = source_tx_hashes
            .iter()
            .map(|tx_hash| {
                let (present, proof) = self.app_db.prove_tx(roots, *tx_hash)?;
                Ok(TxMembershipProof {
                    tx_hash: *tx_hash,
                    present,
                    proof,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(GroundingWitnessSnapshot { source_tx_proofs })
    }
}
