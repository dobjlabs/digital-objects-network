use alloy::eips::eip4844::FIELD_ELEMENT_BYTES_USIZE;
use anyhow::{anyhow, Result};

/// Extracts bytes from a blob in the "simple" encoding.
///
/// Encoding: `[0x00] ++ 8_BYTE_LEN_BE ++ padding ++ chunks`
/// where each chunk is `[0x00] ++ 31_DATA_BYTES`.
pub fn bytes_from_simple_blob(blob_bytes: &[u8]) -> Result<Vec<u8>> {
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
    if blob_bytes[0] != 0x00 {
        return Err(anyhow!("Invalid blob: first field-element marker must be 0x00"));
    }
    if blob_bytes[9..FIELD_ELEMENT_BYTES_USIZE]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(anyhow!(
            "Invalid blob: length field padding bytes must be zero"
        ));
    }

    let field_elements = blob_bytes.len() / FIELD_ELEMENT_BYTES_USIZE;
    if field_elements < 1 {
        return Err(anyhow!("Invalid blob length {}", blob_bytes.len()));
    }
    let max_data_len = (field_elements - 1) * (FIELD_ELEMENT_BYTES_USIZE - 1);
    if data_len > max_data_len {
        return Err(anyhow!(
            "Given blob of length {} cannot accommodate {} bytes.",
            blob_bytes.len(),
            data_len
        ));
    }

    let mut out = Vec::with_capacity(data_len);
    for (field_idx, chunk) in blob_bytes.chunks(FIELD_ELEMENT_BYTES_USIZE).enumerate().skip(1) {
        if chunk[0] != 0x00 {
            return Err(anyhow!(
                "Invalid blob: field element {} marker must be 0x00",
                field_idx
            ));
        }
        out.extend_from_slice(&chunk[1..]);
    }
    out.truncate(data_len);
    Ok(out)
}

/// Encodes bytes into the "simple" blob encoding, producing a full 4096-field-element blob.
#[cfg(test)]
pub fn bytes_to_simple_blob(data: &[u8]) -> Vec<u8> {
    let capacity = 4096 * FIELD_ELEMENT_BYTES_USIZE;
    let mut blob = vec![0u8; capacity];

    // First field element: 0x00 | 8-byte length BE | zeros
    let len_bytes = (data.len() as u64).to_be_bytes();
    blob[1..9].copy_from_slice(&len_bytes);

    // Remaining field elements: 0x00 | up to 31 bytes of data
    for (i, chunk) in data.chunks(FIELD_ELEMENT_BYTES_USIZE - 1).enumerate() {
        let offset = (i + 1) * FIELD_ELEMENT_BYTES_USIZE;
        if offset >= capacity {
            break;
        }
        blob[offset] = 0x00;
        blob[offset + 1..offset + 1 + chunk.len()].copy_from_slice(chunk);
    }

    blob
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let data = b"hello, world! this is a test payload.";
        let blob = bytes_to_simple_blob(data);
        let decoded = bytes_from_simple_blob(&blob).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_empty_data() {
        let blob = bytes_to_simple_blob(b"");
        let decoded = bytes_from_simple_blob(&blob).unwrap();
        assert_eq!(decoded, b"");
    }

    #[test]
    fn test_rejects_nonzero_first_marker() {
        let mut blob = bytes_to_simple_blob(b"abc");
        blob[0] = 1;
        assert!(bytes_from_simple_blob(&blob).is_err());
    }

    #[test]
    fn test_rejects_nonzero_length_padding() {
        let mut blob = bytes_to_simple_blob(b"abc");
        blob[10] = 1;
        assert!(bytes_from_simple_blob(&blob).is_err());
    }

    #[test]
    fn test_rejects_nonzero_data_field_marker() {
        let mut blob = bytes_to_simple_blob(b"abc");
        blob[FIELD_ELEMENT_BYTES_USIZE] = 1;
        assert!(bytes_from_simple_blob(&blob).is_err());
    }
}
