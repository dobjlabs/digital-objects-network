use alloy::eips::eip4844::FIELD_ELEMENT_BYTES_USIZE;
use anyhow::{Result, anyhow};

pub const SIMPLE_BLOB_BYTES: usize = 4096 * FIELD_ELEMENT_BYTES_USIZE;
pub const MAX_SIMPLE_BLOB_PAYLOAD_BYTES: usize = (4096 - 1) * (FIELD_ELEMENT_BYTES_USIZE - 1);

/// Decodes bytes from a single blob using the "simple" coding scheme.
///
/// Encoding: `[0x00] ++ 8_BYTE_LEN_BE ++ padding ++ chunks`
/// where each chunk is `[0x00] ++ 31_DATA_BYTES`.
pub fn decode_simple_blob(blob_bytes: &[u8]) -> Result<Vec<u8>> {
    if blob_bytes.len() < 9 {
        return Err(anyhow!(
            "Invalid blob length {}; expected at least 9 bytes",
            blob_bytes.len()
        ));
    }
    if !blob_bytes.len().is_multiple_of(FIELD_ELEMENT_BYTES_USIZE) {
        return Err(anyhow!(
            "Invalid blob length {}; expected multiple of {}",
            blob_bytes.len(),
            FIELD_ELEMENT_BYTES_USIZE
        ));
    }

    let data_len = u64::from_be_bytes(std::array::from_fn(|i| blob_bytes[1 + i])) as usize;

    let field_elements = blob_bytes.len() / FIELD_ELEMENT_BYTES_USIZE;
    if field_elements < 1 {
        return Err(anyhow!("Invalid blob length {}", blob_bytes.len()));
    }
    let max_data_len = (field_elements - 1) * (FIELD_ELEMENT_BYTES_USIZE - 1);
    if data_len > max_data_len {
        return Err(anyhow!(
            "Given blob of length {} cannot accommodate {} bytes",
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

/// Encodes bytes into a full single-blob payload using the "simple" coding scheme.
pub fn encode_simple_blob(data: &[u8]) -> Result<[u8; SIMPLE_BLOB_BYTES]> {
    if data.len() > MAX_SIMPLE_BLOB_PAYLOAD_BYTES {
        return Err(anyhow!(
            "Payload length {} exceeds simple-blob maximum {}",
            data.len(),
            MAX_SIMPLE_BLOB_PAYLOAD_BYTES
        ));
    }

    let mut blob = [0u8; SIMPLE_BLOB_BYTES];

    // First field element: 0x00 | 8-byte length BE | zeros
    let len_bytes = (data.len() as u64).to_be_bytes();
    blob[1..9].copy_from_slice(&len_bytes);

    // Remaining field elements: 0x00 | up to 31 bytes of data
    for (i, chunk) in data.chunks(FIELD_ELEMENT_BYTES_USIZE - 1).enumerate() {
        let offset = (i + 1) * FIELD_ELEMENT_BYTES_USIZE;
        blob[offset] = 0x00;
        blob[offset + 1..offset + 1 + chunk.len()].copy_from_slice(chunk);
    }

    Ok(blob)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let data = b"hello, world! this is a test payload.";
        let blob = encode_simple_blob(data).unwrap();
        let decoded = decode_simple_blob(&blob).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_empty_data() {
        let blob = encode_simple_blob(b"").unwrap();
        let decoded = decode_simple_blob(&blob).unwrap();
        assert_eq!(decoded, b"");
    }
}
