use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaimValidity {
    Live,
    Nullified,
}

impl ClaimValidity {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            ClaimValidity::Live => "live",
            ClaimValidity::Nullified => "nullified",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    pub name: String,
    pub validity: ClaimValidity,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseDto {
    pub id: Uuid,
    pub post_id: Uuid,
    pub peer: String,
    pub time: DateTime<Utc>,
    pub desc: String,
    pub proofs: Vec<Claim>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostDto {
    pub id: Uuid,
    pub title: String,
    pub peer: String,
    pub time: DateTime<Utc>,
    pub description: String,
    pub proofs: Vec<Claim>,
    pub responses: Vec<ResponseDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPostsResponse {
    pub items: Vec<PostDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPostsQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub q: Option<String>,
    pub live_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePostRequest {
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub claims: Vec<Claim>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateResponseRequest {
    pub description: String,
    #[serde(default)]
    pub claims: Vec<Claim>,
}

#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

pub fn encode_cursor(cursor: Cursor) -> String {
    let payload = format!("{}|{}", cursor.created_at.to_rfc3339(), cursor.id);
    URL_SAFE_NO_PAD.encode(payload)
}

pub fn decode_cursor(raw: &str) -> Option<Cursor> {
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let mut parts = text.split('|');
    let created_at = parts.next()?.parse::<DateTime<Utc>>().ok()?;
    let id = parts.next()?.parse::<Uuid>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(Cursor { created_at, id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_roundtrip() {
        let cursor = Cursor {
            created_at: Utc::now(),
            id: Uuid::new_v4(),
        };
        let encoded = encode_cursor(cursor);
        let decoded = decode_cursor(&encoded).expect("decode");
        assert_eq!(decoded.id, cursor.id);
        assert_eq!(
            decoded.created_at.timestamp(),
            cursor.created_at.timestamp()
        );
    }
}
