pub mod clients;

use alloy::eips::eip4844::FIELD_ELEMENT_BYTES_USIZE;
use anyhow::{anyhow, Result};

// From https://github.com/0xPARC/digital-objects-e2e-poc/blob/main/synchronizer/src/lib.rs
/// Extracts bytes from a blob in the 'simple' encoding.
pub fn bytes_from_simple_blob(blob_bytes: &[u8]) -> Result<Vec<u8>> {
    // Blob = [0x00] ++ 8_BYTE_LEN ++ [0x00,...,0x00] ++ X.
    let data_len = u64::from_be_bytes(std::array::from_fn(|i| blob_bytes[1 + i])) as usize;

    // Sanity check: Blob must be able to accommodate the specified data length.
    let max_data_len =
        (blob_bytes.len() / FIELD_ELEMENT_BYTES_USIZE - 1) * (FIELD_ELEMENT_BYTES_USIZE - 1);
    if data_len > max_data_len {
        return Err(anyhow!(
            "Given blob of length {} cannot accommodate {} bytes.",
            blob_bytes.len(),
            data_len
        ));
    }

    Ok(blob_bytes
        .chunks(FIELD_ELEMENT_BYTES_USIZE)
        .skip(1)
        .flat_map(|chunk| chunk[1..].to_vec())
        .take(data_len)
        .collect())
}
