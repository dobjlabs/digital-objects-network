//! Wire messages for the swap protocol.
//!
//! Everything here is plan data or a proven artifact: pods verify,
//! disclosures check against commitments, and grounding checks against
//! the state root, so the messages are self-authenticating and the
//! transport needs no identity layer. Fields are named after the two
//! objects being swapped, never after a party-relative direction: the
//! accepter's object moves to the initiator (event 0), the initiator's
//! object moves to the accepter (event 1).

use joint_tx::{TransferAcceptance, TransferOffer};
use pod2::{
    frontend::MainPod,
    middleware::{EMPTY_VALUE, Hash, Statement, StrKey, Value, containers::Dictionary},
};
use serde::{Deserialize, Serialize};
use txlib::{StateHeader, compute_nullifier, erased_key_state};

/// One side's disclosure of the object it gives: the erased-key state
/// (every field except the key, which is set to the sentinel), the
/// commitment of the real current state, and that state's nullifier.
///
/// The nullifier and commitment are trusted as plan data here; a wrong
/// value cannot forge anything, it only produces a transaction the
/// guards or the double-spend rule reject. Disclosing is irrevocable
/// toward the counterparty even if the deal never lands: the nullifier
/// and commitment let them recognize this object's later spend.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegDisclosure {
    pub mid: Dictionary,
    pub old_commitment: Hash,
    pub nullifier: Hash,
}

impl LegDisclosure {
    pub fn of(obj: &Dictionary) -> Self {
        Self {
            mid: erased_key_state(obj),
            old_commitment: obj.commitment(),
            nullifier: compute_nullifier(obj),
        }
    }

    /// Receive-side sanity: the disclosed state must be key-erased and
    /// of the negotiated class. Everything deeper is carried by proofs.
    pub fn validate(&self, expected_class: Hash) -> anyhow::Result<()> {
        let key = self
            .mid
            .get(&StrKey::from("key"))
            .ok()
            .flatten()
            .ok_or_else(|| anyhow::anyhow!("disclosed state has no key entry"))?;
        anyhow::ensure!(
            key == Value::from(EMPTY_VALUE),
            "disclosed state's key is not erased"
        );
        let class = self
            .mid
            .get(&StrKey::from("type"))
            .ok()
            .flatten()
            .ok_or_else(|| anyhow::anyhow!("disclosed state has no type entry"))?;
        anyhow::ensure!(
            class == Value::from(expected_class),
            "disclosed object is of class {class}, negotiated class is {:#}",
            expected_class
        );
        Ok(())
    }
}

/// Accepter -> initiator: accepts the invitation and discloses the
/// object it gives.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptMsg {
    pub accepter_object: LegDisclosure,
}

/// Initiator -> accepter: the initiator's own disclosure plus the rest
/// of the plan data. `header` is the grounding state header the whole
/// transaction is contexted against; `accepter_object_new` is the
/// commitment of the accepter's object under the initiator's (never
/// disclosed) new key.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDataMsg {
    pub initiator_object: LegDisclosure,
    pub header: StateHeader,
    pub accepter_object_new: Hash,
}

/// Accepter -> initiator: the last plan datum (the initiator's object
/// under the accepter's new key) plus the accepter's derivation of
/// `tx_final`, so a divergent plan aborts before anyone proves.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanAckMsg {
    pub initiator_object_new: Hash,
    pub tx_final: Hash,
}

/// Initiator -> accepter, round 0: the offer of the initiator's object
/// and the pod proving it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferMsg {
    pub offer: TransferOffer,
    pub pod: MainPod,
}

/// Accepter -> initiator, round 1: the accepter's whole session in one
/// message. Its offer of its own object, its acceptance of the
/// initiator's object, the class guard wrapping that acceptance's
/// Rekey, and the pod proving all of it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceMsg {
    pub offer: TransferOffer,
    pub acceptance: TransferAcceptance,
    pub guard: Statement,
    pub pod: MainPod,
}

/// The out-of-band invitation, passed between users as a base58 blob
/// over whatever channel they like. Carries only durable data: node
/// identity plus the two class guard hashes. No grounding-bound field,
/// so an invitation never expires; only endorsements do.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invitation {
    pub node: iroh::EndpointAddr,
    /// Guard hash of the class the initiator gives.
    pub offers: Hash,
    /// Guard hash of the class the initiator wants.
    pub wants: Hash,
    /// Whether this is a mock dry run. The initiator's mode is the
    /// deal's mode: proofs from the two modes cannot compose, so the
    /// accepter adopts this rather than choosing its own.
    pub mock: bool,
}

impl Invitation {
    pub fn encode(&self) -> String {
        bs58::encode(serde_json::to_vec(self).expect("invitation serializes")).into_string()
    }

    pub fn decode(blob: &str) -> anyhow::Result<Self> {
        let bytes = bs58::decode(blob.trim())
            .into_vec()
            .map_err(|err| anyhow::anyhow!("not a base58 invitation: {err}"))?;
        serde_json::from_slice(&bytes)
            .map_err(|err| anyhow::anyhow!("not a trade invitation: {err}"))
    }
}

/// Envelope for everything that crosses the iroh stream, one JSON line
/// per message.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WireMsg {
    Accept(AcceptMsg),
    PlanData(PlanDataMsg),
    PlanAck(PlanAckMsg),
    Offer(Box<OfferMsg>),
    Acceptance(Box<AcceptanceMsg>),
    /// Executor-side narration for the counterparty's screen.
    Progress {
        note: String,
    },
    /// The transaction is with the relayer.
    Posted {
        tx_hash: Option<String>,
        block_number: Option<i64>,
    },
    /// Either side backs out; the deal dies harmlessly.
    Abort {
        reason: String,
    },
}
