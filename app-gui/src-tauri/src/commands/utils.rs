use crate::state::AppState;
use std::sync::atomic::Ordering;

pub(crate) fn new_id(state: &AppState) -> String {
    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    format!("post-{id}")
}

pub(crate) fn now_label() -> String {
    "mock-now".to_string()
}

pub(crate) fn fake_hash(seed: &str) -> String {
    let mut bytes = [0u8; 8];
    for (idx, b) in seed.bytes().enumerate() {
        bytes[idx % 8] = bytes[idx % 8].wrapping_add(b);
    }
    format!(
        "0x{:02x}{:02x}...{:02x}{:02x}",
        bytes[0], bytes[1], bytes[6], bytes[7]
    )
}
