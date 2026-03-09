use anyhow::{Result, anyhow};

const FIELD_ELEMENT_BYTES_USIZE: usize = 32;
pub const SIMPLE_BLOB_BYTES: usize = 4096 * FIELD_ELEMENT_BYTES_USIZE;
pub const MAX_SIMPLE_BLOB_PAYLOAD_BYTES: usize = (4096 - 1) * (FIELD_ELEMENT_BYTES_USIZE - 1);

/// Decodes bytes from a single blob using the "simple" coding scheme.
///
/// Layout:
/// - field element 0: `[0x00][len:8 bytes BE][padding zeros]`
/// - remaining field elements: `[0x00][31 data bytes]`
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
    let len_bytes = (data.len() as u64).to_be_bytes();
    blob[1..9].copy_from_slice(&len_bytes);

    for (i, chunk) in data.chunks(FIELD_ELEMENT_BYTES_USIZE - 1).enumerate() {
        let offset = (i + 1) * FIELD_ELEMENT_BYTES_USIZE;
        blob[offset] = 0;
        blob[offset + 1..offset + 1 + chunk.len()].copy_from_slice(chunk);
    }

    Ok(blob)
}

#[cfg(test)]
mod tests {
    use super::{decode_simple_blob, encode_simple_blob};

    #[test]
    fn roundtrip_simple_blob() {
        let data = b"hello from zk-craft";
        let blob = encode_simple_blob(data).unwrap();
        let decoded = decode_simple_blob(&blob).unwrap();
        assert_eq!(decoded, data);
    }
}
