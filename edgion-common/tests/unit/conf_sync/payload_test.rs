use super::*;

#[tokio::test]
async fn identity_shares_allocation_and_requires_exact_length() {
    let input = Bytes::from_static(b"private configuration");
    let encoded = encode(input.clone()).await.unwrap();
    assert_eq!(encoded.encoding, PayloadEncoding::Identity as i32);
    assert_eq!(encoded.data.as_ptr(), input.as_ptr());
    let output = decode(encoded.data.clone(), encoded.encoding, encoded.uncompressed_size)
        .await
        .unwrap();
    assert_eq!(output.as_ptr(), input.as_ptr());
    assert_eq!(decode(input, 0, 0).await.unwrap_err(), PayloadError::LengthMismatch);
    assert!(!format!("{encoded:?}").contains("private configuration"));
}

#[tokio::test]
async fn threshold_is_strict_and_large_payload_roundtrips() {
    let input = Bytes::from(vec![b'x'; COMPRESSION_THRESHOLD]);
    let encoded = encode(input.clone()).await.unwrap();
    assert_eq!(encoded.encoding, PayloadEncoding::Identity as i32);
    assert_eq!(encoded.data.as_ptr(), input.as_ptr());
    let input = Bytes::from(vec![b'x'; COMPRESSION_THRESHOLD + 1]);
    let encoded = encode(input.clone()).await.unwrap();
    assert_eq!(encoded.encoding, PayloadEncoding::Zstd as i32);
    assert!(encoded.data.len() < input.len());
    assert_eq!(
        decode(encoded.data, encoded.encoding, encoded.uncompressed_size)
            .await
            .unwrap(),
        input
    );
}

#[test]
fn compression_that_does_not_save_space_keeps_original_allocation() {
    let input = Bytes::from_static(b"abc");
    let encoded = compress(input.clone()).unwrap();
    assert_eq!(encoded.encoding, PayloadEncoding::Identity as i32);
    assert_eq!(encoded.data.as_ptr(), input.as_ptr());
}

#[tokio::test]
async fn rejects_unknown_encoding_and_oversized_declarations_before_allocation() {
    assert_eq!(
        decode(Bytes::new(), 42, 0).await.unwrap_err(),
        PayloadError::UnknownEncoding
    );
    for size in [MAX_PAYLOAD_BYTES as u64 + 1, u64::MAX] {
        assert_eq!(decode(Bytes::new(), 1, size).await.unwrap_err(), PayloadError::TooLarge);
    }
    assert_eq!(
        PayloadError::TooLarge.into_status().code(),
        tonic::Code::ResourceExhausted
    );
}

#[tokio::test]
async fn rejects_malformed_truncated_trailing_and_concatenated_frames() {
    let raw = vec![b'x'; 4096];
    let compressed = zstd::stream::encode_all(raw.as_slice(), 3).unwrap();
    let mut trailing = compressed.clone();
    trailing.push(0);
    let mut concatenated = compressed.clone();
    concatenated.extend_from_slice(&compressed);
    for input in [
        Vec::new(),
        b"not a zstd frame".to_vec(),
        compressed[..compressed.len() - 1].to_vec(),
        trailing,
        concatenated,
    ] {
        assert_eq!(
            decode(input.into(), 1, raw.len() as u64).await.unwrap_err(),
            PayloadError::InvalidCompressedData
        );
    }
}

#[tokio::test]
async fn validates_actual_size_both_above_and_below_declared_size() {
    let raw = vec![b'x'; 256 * 1024];
    let compressed = Bytes::from(zstd::stream::encode_all(raw.as_slice(), 3).unwrap());
    for expected in [0, raw.len() as u64 - 1, raw.len() as u64 + 1] {
        assert_eq!(
            decode(compressed.clone(), 1, expected).await.unwrap_err(),
            PayloadError::LengthMismatch
        );
    }
}

#[tokio::test]
async fn rejects_frame_window_above_budget() {
    // Standard frame, no content size, 512 MiB window, final empty raw block.
    let frame = Bytes::from_static(&[0x28, 0xb5, 0x2f, 0xfd, 0, 0x98, 1, 0, 0]);
    assert_eq!(
        decode(frame, 1, 0).await.unwrap_err(),
        PayloadError::InvalidCompressedData
    );
}
