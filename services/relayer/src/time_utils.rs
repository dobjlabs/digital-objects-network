use std::time::{SystemTime, UNIX_EPOCH};

/// Current unix timestamp in seconds.
pub fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
