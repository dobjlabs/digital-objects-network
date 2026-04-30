//! `.dobj` file shape post-migration.
//!
//! Each file is a JSON document representing one object snapshot. Compared to
//! the pod2-era format:
//! - the `pod` field is gone (no recursive proof composition)
//! - in its place we store everything needed to *re-derive* a grounding
//!   proof when this object is later consumed: the source tx's components
//!   plus the full live commitment list
//!
//! ## File shape
//! ```json
//! {
//!   "id": "0x<obj.commitment hex>",
//!   "className": "WoodPick",
//!   "status": "live",
//!   "txHash": "0x<eth tx hash>",
//!   "obj": { "fields": { "key": {"Hash": "0x..."}, ... } },
//!   "sourceTx": {
//!     "actionId": 4,
//!     "liveRoot": "0x...",
//!     "nullifiersRoot": "0x...",
//!     "actionNonce": "0x..."
//!   },
//!   "sourceTxLive": ["0x...", "0x...", ...]
//! }
//! ```
//!
//! `sourceTxLive` is a sorted list of all `obj.commitment()` values that were
//! live in the source tx — used to rebuild that tx's `live_root` SMT when
//! this object gets consumed later.

use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use txlib_core::Object;
use txlib_core::abi::{ActionId, InputObject};
use txlib_core::merkle::MerkleProof;
use txlib_core::merkle_store::{InMemoryNodeStore, PersistentSmt, empty_root};
use txlib_core::{Hash, Tx};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectStatus {
    /// Locally produced; relayer has not yet confirmed the blob.
    Unknown,
    /// Relayer confirmed; synchronizer has not yet observed the tx_final.
    Pending,
    /// Synchronizer has indexed the source tx — object is spendable.
    Live,
    /// Object has been consumed by an action; nullifier is on-chain.
    Nullified,
}

/// Components of the source tx that produced this object. Together with
/// `source_tx_live` they let us rebuild the inclusion proof when this
/// object is later consumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTxData {
    pub action_id: ActionId,
    pub live_root: Hash,
    pub nullifiers_root: Hash,
    pub action_nonce: Hash,
}

impl SourceTxData {
    pub fn tx_final(&self) -> Hash {
        Tx {
            action_id: self.action_id,
            live_root: self.live_root,
            nullifiers_root: self.nullifiers_root,
            action_nonce: self.action_nonce,
        }
        .tx_final()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRecord {
    /// `obj.commitment()` rendered as `0x`-prefixed hex.
    pub id: String,
    pub class_name: String,
    pub status: ObjectStatus,
    /// Ethereum transaction hash from the relayer; set once the blob is
    /// confirmed. `None` while `status == Unknown`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    pub obj: Object,
    pub source_tx: SourceTxData,
    /// All live commitments from the source tx, in canonical (sorted) order.
    /// Used to rebuild `live_root` and prove this object's inclusion when
    /// it gets consumed later.
    pub source_tx_live: Vec<Hash>,
}

impl ObjectRecord {
    pub fn new(
        obj: Object,
        class_name: String,
        source_tx: SourceTxData,
        source_tx_live: Vec<Hash>,
    ) -> Self {
        let id = format!("{}", obj.commitment());
        Self {
            id,
            class_name,
            status: ObjectStatus::Unknown,
            tx_hash: None,
            obj,
            source_tx,
            source_tx_live,
        }
    }

    pub fn commitment(&self) -> Hash {
        self.obj.commitment()
    }

    pub fn is_nullified(&self) -> bool {
        self.status == ObjectStatus::Nullified
    }

    /// Build the live-set inclusion proof for this object's commitment
    /// against `source_tx.live_root`. Re-derives the SMT from
    /// `source_tx_live` (cheap — typically 1-3 leaves).
    pub fn live_inclusion_proof(&self) -> Result<MerkleProof> {
        let store = InMemoryNodeStore::new();
        let mut smt = PersistentSmt::open(empty_root(), &store);
        for c in &self.source_tx_live {
            smt.insert(*c, *c).map_err(|e| anyhow!("smt insert: {e}"))?;
        }
        // Sanity: the rebuilt root must match what the file claims.
        if smt.root != self.source_tx.live_root {
            return Err(anyhow!(
                "source_tx_live does not hash to source_tx.live_root: \
                 expected {}, got {}",
                self.source_tx.live_root,
                smt.root
            ));
        }
        let commitment = self.commitment();
        smt.prove(commitment).map_err(|e| anyhow!("smt prove: {e}"))
    }

    /// Build the [`InputObject`] needed to feed this object as input to the
    /// next action. Caller supplies the `tx_inclusion_proof` (fetched from
    /// the synchronizer's grounding-witness API).
    pub fn to_input_object(&self, tx_inclusion_proof: MerkleProof) -> Result<InputObject> {
        Ok(InputObject {
            obj: self.obj.clone(),
            source_tx_action_id: self.source_tx.action_id,
            source_tx_live_root: self.source_tx.live_root,
            source_tx_nullifiers_root: self.source_tx.nullifiers_root,
            source_tx_action_nonce: self.source_tx.action_nonce,
            live_inclusion_proof: self.live_inclusion_proof()?,
            tx_inclusion_proof,
        })
    }
}

/// `<class>_0x<commitment_hex>.dobj`. Deterministic for a given object —
/// two records with the same content share the same name. Matches the
/// pre-migration naming convention from `docs/digital-objects.md`.
pub fn file_name_for(class_name: &str, commitment: Hash) -> String {
    let mut s = String::with_capacity(class_name.len() + 3 + 64 + 5);
    s.push_str(class_name);
    s.push_str("_0x");
    for b in commitment.as_bytes() {
        s.push_str(&format!("{b:02x}"));
    }
    s.push_str(".dobj");
    s
}

pub fn parse_object_record_file(path: &Path) -> Result<ObjectRecord> {
    let raw = fs::read_to_string(path)
        .map_err(|e| anyhow!("read {}: {e}", path.display()))?;
    let r: ObjectRecord = serde_json::from_str(&raw)
        .map_err(|e| anyhow!("parse {}: {e}", path.display()))?;
    // Validity check: file's id must match the recomputed commitment.
    let expected_id = format!("{}", r.obj.commitment());
    if r.id != expected_id {
        return Err(anyhow!(
            "{}: id mismatch (file says {}, recomputed {})",
            path.display(),
            r.id,
            expected_id
        ));
    }
    Ok(r)
}

pub fn write_object_record_file(path: &Path, record: &ObjectRecord) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("object record path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| anyhow!("mkdir {}: {e}", parent.display()))?;
    let s = serde_json::to_string_pretty(record)?;
    fs::write(path, s).map_err(|e| anyhow!("write {}: {e}", path.display()))?;
    Ok(())
}

/// Convenience for callers that build a tx and need to store its outputs as
/// individual `.dobj` records: returns the canonical sorted commitment list
/// for one set of new objects.
pub fn sorted_commitments(objs: &[Object]) -> Vec<Hash> {
    let mut v: Vec<Hash> = objs.iter().map(|o| o.commitment()).collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use txlib_core::hash::sha256;
    use txlib_core::merkle::set_smt_root;
    use txlib_core::object;

    fn sample_record() -> ObjectRecord {
        let obj = object! {
            "blueprint" => "Wood",
            "key" => sha256(b"k"),
        };
        let live = sorted_commitments(&[obj.clone()]);
        let live_root = set_smt_root(&live);
        let source_tx = SourceTxData {
            action_id: 2,
            live_root,
            nullifiers_root: empty_root(),
            action_nonce: sha256(b"nonce"),
        };
        ObjectRecord::new(obj, "Wood".to_string(), source_tx, live)
    }

    #[test]
    fn record_roundtrip() {
        let dir = tempdir().unwrap();
        let r = sample_record();
        let path = dir.path().join(file_name_for("Wood", r.commitment()));
        write_object_record_file(&path, &r).unwrap();
        let parsed = parse_object_record_file(&path).unwrap();
        assert_eq!(r, parsed);
    }

    #[test]
    fn record_id_mismatch_rejected() {
        let dir = tempdir().unwrap();
        let mut r = sample_record();
        let path = dir.path().join("bad.dobj");
        r.id = "0x0000".to_string(); // tampered
        write_object_record_file(&path, &r).unwrap();
        let err = parse_object_record_file(&path).unwrap_err();
        assert!(err.to_string().contains("id mismatch"), "{err}");
    }

    #[test]
    fn live_inclusion_proof_verifies() {
        let r = sample_record();
        let proof = r.live_inclusion_proof().unwrap();
        let commitment = r.commitment();
        assert!(txlib_core::merkle::verify_inclusion(
            r.source_tx.live_root,
            commitment,
            commitment,
            &proof,
        ));
    }

    #[test]
    fn live_inclusion_fails_if_source_tx_live_lies() {
        let mut r = sample_record();
        r.source_tx_live = vec![sha256(b"unrelated")];
        let err = r.live_inclusion_proof().unwrap_err();
        assert!(err.to_string().contains("does not hash to"), "{err}");
    }

    #[test]
    fn file_name_is_deterministic() {
        let r = sample_record();
        let n1 = file_name_for(&r.class_name, r.commitment());
        let n2 = file_name_for(&r.class_name, r.commitment());
        assert_eq!(n1, n2);
        assert!(n1.starts_with("Wood_0x"));
        assert!(n1.ends_with(".dobj"));
    }
}
