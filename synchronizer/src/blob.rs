use anyhow::Result;

/// Backward-compatible wrapper around the shared blob decoder.
pub fn bytes_from_simple_blob(blob_bytes: &[u8]) -> Result<Vec<u8>> {
    common::blob_codec::decode_simple_blob(blob_bytes)
}

/// Test-only helper that uses the shared encoder.
#[cfg(test)]
pub fn bytes_to_simple_blob(data: &[u8]) -> Vec<u8> {
    common::blob_codec::encode_simple_blob(data)
        .expect("valid blob encoding")
        .to_vec()
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
}
