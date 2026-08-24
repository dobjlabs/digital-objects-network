//! Wire messages for the swap protocol.
//!
//! Messages carry negotiated plan data or proven artifacts. Receive-side
//! checks validate disclosures, pods, and grounding data. Fields name the
//! accepter and initiator objects consistently: event 0 transfers the
//! accepter's object; event 1 transfers the initiator's.

use joint_tx::{JointTransaction, TransferAcceptance, TransferOffer};
use pod2::{
    frontend::MainPod,
    middleware::{EMPTY_VALUE, Hash, Statement, StrKey, Value, containers::Dictionary},
};
use serde::{Deserialize, Serialize};
use txlib::{StateHeader, compute_nullifier, erased_key_state};

/// Seat names embedded in the shared `JointTransaction`.
pub const INITIATOR: &str = "initiator";
pub const ACCEPTER: &str = "accepter";

/// Erased-key state, original commitment, and nullifier for one swap leg.
///
/// These values define the negotiated plan; subsequent proofs enforce it.
/// Disclosure lets the counterparty recognize a later spend of this state,
/// even if the trade is abandoned.
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

    /// Require an erased key and the negotiated class.
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

/// Initiator -> accepter: initiator disclosure and remaining plan data.
/// `header` grounds the transaction; `accepter_object_new` commits to
/// the accepter's object under the initiator's private new key.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDataMsg {
    pub initiator_object: LegDisclosure,
    pub header: StateHeader,
    pub accepter_object_new: Hash,
}

/// Accepter -> initiator: independently derived deal document.
/// The initiator requires exact equality before proving. Deserialization
/// validates the document.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanAckMsg {
    pub transaction: JointTransaction,
}

/// Initiator -> accepter: transfer offer and its pod.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferMsg {
    pub offer: TransferOffer,
    pub pod: MainPod,
}

/// Accepter -> initiator: offer, acceptance, class guard, and shared pod.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceMsg {
    pub offer: TransferOffer,
    pub acceptance: TransferAcceptance,
    pub guard: Statement,
    pub pod: MainPod,
}

/// Base58 invitation exchanged out of band.
/// Contains the endpoint, class guard hashes, and proving mode, but no
/// grounding-bound transaction data.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invitation {
    pub node: iroh::EndpointAddr,
    /// Guard hash of the class the initiator gives.
    pub offers: Hash,
    /// Guard hash of the class the initiator wants.
    pub wants: Hash,
    /// Proving mode selected by the initiator. Mock and real pods cannot mix.
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

/// One JSON-line message on the iroh stream.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WireMsg {
    Accept(AcceptMsg),
    PlanData(PlanDataMsg),
    PlanAck(PlanAckMsg),
    Offer(Box<OfferMsg>),
    Acceptance(Box<AcceptanceMsg>),
    /// Progress text for the counterparty's screen.
    Progress {
        note: String,
    },
    /// The transaction is with the relayer.
    Posted {
        tx_hash: Option<String>,
        block_number: Option<i64>,
    },
    /// Either side aborts the trade.
    Abort {
        reason: String,
    },
}
