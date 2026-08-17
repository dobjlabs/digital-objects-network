// MIT License Copyright (c) 2022 Blobscan <https://blobscan.com>
//
// Permission is hereby granted, free of charge,
// to any person obtaining a copy of this software and associated documentation
// files (the "Software"), to deal in the Software without restriction, including
// without limitation the rights to use, copy, modify, merge, publish, distribute,
// sublicense, and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above
// copyright notice and this permission notice (including the next paragraph) shall
// be included in all copies or substantial portions of the Software.
//
// THE SOFTWARE
// IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
// BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
// PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS
// BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
// CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
// SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

#![allow(dead_code)]

use std::{fmt, str::FromStr};

use alloy::{consensus::Bytes48, eips::eip4844::HeapBlob, primitives::B256};
use serde::{Deserialize, Serialize, Serializer};

use super::BeaconClient;
use crate::common::ClientError;

pub type KzgCommitment = Bytes48;

pub type Proof = Bytes48;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub enum BlockId {
    Head,
    Finalized,
    Slot(u32),
    Hash(B256),
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Head,
    FinalizedCheckpoint,
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Topic::Head => write!(f, "head"),
            Topic::FinalizedCheckpoint => write!(f, "finalized_checkpoint"),
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
pub struct SpecResponse {
    pub data: Spec,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct Spec {
    #[serde(
        rename = "DEPOSIT_NETWORK_ID",
        deserialize_with = "deserialize_u64",
        serialize_with = "serialize_u64"
    )]
    pub deposit_network_id: u64,
}
#[derive(Deserialize, Debug)]
pub struct Block {
    // Mandatory after "Deneb"
    pub blob_kzg_commitments: Vec<KzgCommitment>,
    // Mandatory after "Bellatrix/The Merge"
    pub execution_payload: ExecutionPayload,
    pub parent_root: B256,
    #[serde(deserialize_with = "deserialize_u32")]
    pub slot: u32,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct ExecutionPayload {
    pub block_hash: B256,
    #[serde(deserialize_with = "deserialize_u32", serialize_with = "serialize_u32")]
    pub block_number: u32,
    #[serde(deserialize_with = "deserialize_u64", serialize_with = "serialize_u64")]
    pub timestamp: u64,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct BlockBody {
    // Mandatory after "Deneb"
    pub execution_payload: ExecutionPayload,
    // Mandatory after "Bellatrix/The Merge"
    pub blob_kzg_commitments: Vec<KzgCommitment>,
}
#[derive(Deserialize, Serialize, Debug)]
pub struct BlockMessage {
    pub body: BlockBody,
    pub parent_root: B256,
    #[serde(deserialize_with = "deserialize_u32", serialize_with = "serialize_u32")]
    pub slot: u32,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct BlockData {
    pub message: BlockMessage,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct BlockResponse {
    pub data: BlockData,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BlobSidecar {
    #[serde(deserialize_with = "deserialize_u32", serialize_with = "serialize_u32")]
    pub index: u32,
    pub kzg_commitment: KzgCommitment,
    pub kzg_proof: Proof,
    pub blob: HeapBlob,
}

#[derive(Deserialize, Debug)]
pub struct BlobsSidecarsResponse {
    pub data: Vec<BlobSidecar>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BlobsResponse {
    pub data: Vec<HeapBlob>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct BlockHeaderResponse {
    pub data: BlockHeaderData,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockHeader {
    pub root: B256,
    pub parent_root: B256,
    pub slot: u32,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct BlockHeaderData {
    pub root: B256,
    pub header: InnerBlockHeader,
}
#[derive(Deserialize, Serialize, Debug)]
pub struct InnerBlockHeader {
    pub message: BlockHeaderMessage,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct BlockHeaderMessage {
    pub parent_root: B256,
    #[serde(deserialize_with = "deserialize_u32", serialize_with = "serialize_u32")]
    pub slot: u32,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct HeadEventData {
    #[serde(deserialize_with = "deserialize_u32", serialize_with = "serialize_u32")]
    pub slot: u32,
    #[allow(dead_code)]
    pub block: B256,
}

#[derive(Deserialize, Debug)]
pub struct FinalizedCheckpointEventData {
    pub block: B256,
}

fn serialize_u32<S>(v: &u32, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&v.to_string())
}

fn serialize_u64<S>(v: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&v.to_string())
}

fn deserialize_u32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;

    value.parse::<u32>().map_err(serde::de::Error::custom)
}

fn deserialize_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;

    value.parse::<u64>().map_err(serde::de::Error::custom)
}

impl BlockId {
    pub fn to_detailed_string(&self) -> String {
        match self {
            BlockId::Head => String::from("head"),
            BlockId::Finalized => String::from("finalized"),
            BlockId::Slot(slot) => slot.to_string(),
            BlockId::Hash(hash) => format!("0x{:x}", hash),
        }
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockId::Head => write!(f, "head"),
            BlockId::Finalized => write!(f, "finalized"),
            BlockId::Slot(slot) => write!(f, "{}", slot),
            BlockId::Hash(hash) => write!(f, "{}", hash),
        }
    }
}

impl FromStr for BlockId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "head" => Ok(BlockId::Head),
            "finalized" => Ok(BlockId::Finalized),
            _ => match s.parse::<u32>() {
                Ok(num) => Ok(BlockId::Slot(num)),
                Err(_) => {
                    if s.starts_with("0x") {
                        match B256::from_str(s) {
                            Ok(hash) => Ok(BlockId::Hash(hash)),
                            Err(_) => Err(format!("Invalid block ID hash: {s}")),
                        }
                    } else {
                        Err(format!(
                            "Invalid block ID: {s}. Expected 'head', 'finalized', a hash or a number."
                        ))
                    }
                }
            },
        }
    }
}

impl From<&Topic> for String {
    fn from(value: &Topic) -> Self {
        match value {
            Topic::Head => String::from("head"),
            Topic::FinalizedCheckpoint => String::from("finalized_checkpoint"),
        }
    }
}

impl From<B256> for BlockId {
    fn from(value: B256) -> Self {
        BlockId::Hash(value)
    }
}

impl From<u32> for BlockId {
    fn from(value: u32) -> Self {
        BlockId::Slot(value)
    }
}

impl From<BlockHeaderResponse> for BlockHeader {
    fn from(response: BlockHeaderResponse) -> Self {
        BlockHeader {
            root: response.data.root,
            parent_root: response.data.header.message.parent_root,
            slot: response.data.header.message.slot,
        }
    }
}

impl From<BlockResponse> for Block {
    fn from(response: BlockResponse) -> Self {
        Block {
            blob_kzg_commitments: response.data.message.body.blob_kzg_commitments,
            execution_payload: response.data.message.body.execution_payload,
            parent_root: response.data.message.parent_root,
            slot: response.data.message.slot,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BlockIdResolutionError {
    #[error("Block with id '{0}' not found")]
    BlockNotFound(BlockId),
    #[error("Failed to resolve block id '{block_id}'")]
    FailedBlockIdResolution {
        block_id: BlockId,
        #[source]
        error: ClientError,
    },
}

impl BlockId {
    async fn resolve_to_slot(
        &self,
        beacon_client: &BeaconClient,
    ) -> Result<u32, BlockIdResolutionError> {
        match self {
            BlockId::Slot(slot) => Ok(*slot),
            _ => match beacon_client
                .get_block_header(self.clone())
                .await
                .map_err(|err| BlockIdResolutionError::FailedBlockIdResolution {
                    block_id: self.clone(),
                    error: err,
                })? {
                Some(header) => Ok(header.slot),
                None => Err(BlockIdResolutionError::BlockNotFound(self.clone())),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The beacon API encodes these integers as JSON strings. Serializing them
    /// as numbers would produce a document this crate's own reader rejects, so
    /// the two halves are checked against each other rather than against a
    /// hand-written literal.
    #[test]
    fn response_types_round_trip_through_their_own_reader() {
        let json = serde_json::to_string(&BlockResponse {
            data: BlockData {
                message: BlockMessage {
                    body: BlockBody {
                        execution_payload: ExecutionPayload {
                            block_hash: B256::repeat_byte(0xaa),
                            block_number: 6,
                            timestamp: 1_786_703_077,
                        },
                        blob_kzg_commitments: vec![KzgCommitment::repeat_byte(0xbb)],
                    },
                    parent_root: B256::repeat_byte(0xcc),
                    slot: 6,
                },
            },
        })
        .expect("serialize");

        assert!(
            json.contains(r#""block_number":"6""#),
            "integers must serialize as strings: {json}"
        );

        let block: Block = serde_json::from_str::<BlockResponse>(&json)
            .expect("the reader must accept what the writer produced")
            .into();
        assert_eq!(block.slot, 6);
        assert_eq!(block.execution_payload.block_number, 6);
        assert_eq!(block.execution_payload.timestamp, 1_786_703_077);
        assert_eq!(block.blob_kzg_commitments.len(), 1);
    }

    #[test]
    fn header_and_spec_round_trip() {
        let json = serde_json::to_string(&BlockHeaderResponse {
            data: BlockHeaderData {
                root: B256::repeat_byte(0x11),
                header: InnerBlockHeader {
                    message: BlockHeaderMessage {
                        parent_root: B256::repeat_byte(0x22),
                        slot: 42,
                    },
                },
            },
        })
        .expect("serialize");
        let header: BlockHeader = serde_json::from_str::<BlockHeaderResponse>(&json)
            .expect("reader accepts writer")
            .into();
        assert_eq!(header.slot, 42);
        assert_eq!(header.root, B256::repeat_byte(0x11));

        let json = serde_json::to_string(&SpecResponse {
            data: Spec {
                deposit_network_id: 31337,
            },
        })
        .expect("serialize");
        assert!(json.contains(r#""DEPOSIT_NETWORK_ID":"31337""#), "{json}");
        let spec: SpecResponse = serde_json::from_str(&json).expect("reader accepts writer");
        assert_eq!(spec.data.deposit_network_id, 31337);
    }
}
