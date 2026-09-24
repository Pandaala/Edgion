use super::*;
use pingora_core::protocols::http::RequestBodyBuffer;

async fn captured(body: &'static [u8]) -> RequestBodyState {
    let (mut storage, view) = super::super::BodyStorage::new(64, 1024);
    storage.write(&Bytes::from_static(body)).await.unwrap();
    storage.finish().await.unwrap();
    let mut state = RequestBodyState::default();
    state.install(view);
    state
}

async fn read_snapshot(mut reader: Box<dyn RequestBodySnapshotReader>) -> Bytes {
    let mut body = Vec::new();
    while let Some(chunk) = reader.next_chunk().await.unwrap() {
        body.extend_from_slice(&chunk);
    }
    Bytes::from(body)
}

#[tokio::test]
async fn bounded_text_read_preserves_missing_known_empty_captured_and_released_states() {
    let mut state = RequestBodyState::default();
    assert_eq!(
        state.read_text_range(0, 0, 0, 0).await,
        Err(BodyOperationError::Body(BodyReadError::NotBuffered))
    );
    state.mark_known_empty();
    for max_bytes in [0, u64::MAX] {
        assert_eq!(
            state.read_text_range(0, max_bytes, 0, 0).await.unwrap(),
            BodyScanResult {
                value: BoundedTextResult::Complete(String::new()),
                bytes_scanned: 0
            }
        );
    }
    assert_eq!(
        state.read_text_range(1, 0, 0, 0).await,
        Err(BodyOperationError::InvalidRange)
    );

    let mut state = captured(b"text").await;
    assert_eq!(
        state.read_text_range(1, 2, 2, 2).await.unwrap(),
        BodyScanResult {
            value: BoundedTextResult::Complete("ex".to_string()),
            bytes_scanned: 2
        }
    );
    assert_eq!(
        state.read_text_range(4, u64::MAX, 0, 0).await.unwrap().value,
        BoundedTextResult::Complete(String::new())
    );
    state.release_view();
    assert_eq!(
        state.read_text_range(0, 0, 0, 0).await,
        Err(BodyOperationError::Body(BodyReadError::NotBuffered))
    );

    let empty = captured(b"").await;
    assert_eq!(
        empty.read_text_range(0, u64::MAX, 0, 0).await.unwrap(),
        BodyScanResult {
            value: BoundedTextResult::Complete(String::new()),
            bytes_scanned: 0
        }
    );
}

#[tokio::test]
async fn known_empty_remains_distinct_from_missing_capture() {
    let mut state = RequestBodyState::default();
    assert_eq!(state.available_len(false), Err(BodyReadError::NotBuffered));
    assert!(state.snapshot_reader(false).is_err());
    assert_eq!(state.read_all().await.unwrap(), None);
    assert!(matches!(
        state.sha256_scan(0).await,
        Err(BodyOperationError::Body(BodyReadError::NotBuffered))
    ));
    assert_eq!(
        state.replace_slice(0..0, Bytes::new()).await,
        Err(BodyReadError::NotBuffered)
    );
    assert_eq!(state.overwrite(Bytes::new()), Err(BodyReadError::NotBuffered));

    state.mark_known_empty();
    assert_eq!(state.available_len(false), Ok(0));
    let reader = state.snapshot_reader(false).unwrap();
    assert_eq!(reader.len(), 0);
    assert_eq!(read_snapshot(reader).await, Bytes::new());
    assert!(!state.has_cached_body());
    assert_eq!(state.cached_len(), Ok(None));
    let digest = state.sha256_scan(0).await.unwrap();
    assert_eq!(digest.value, super::super::EMPTY_SHA256);
    assert_eq!(digest.bytes_scanned, 0);
}

#[tokio::test]
async fn snapshot_reader_is_independent_of_later_mutation() {
    let state = captured(b"original").await;
    let original = state.snapshot_reader(false).unwrap();
    state.overwrite(Bytes::from_static(b"replacement")).unwrap();

    assert_eq!(read_snapshot(original).await, Bytes::from_static(b"original"));
    assert_eq!(
        read_snapshot(state.snapshot_reader(false).unwrap()).await,
        Bytes::from_static(b"replacement")
    );
}

#[tokio::test]
async fn release_drops_plugin_access_but_retains_mutation_framing() {
    let mut state = captured(b"original").await;
    state.overwrite(Bytes::from_static(b"new")).unwrap();

    state.release_view();
    assert!(!state.has_cached_body());
    assert_eq!(state.cached_len(), Ok(None));
    assert_eq!(state.available_len(false), Err(BodyReadError::NotBuffered));
    assert_eq!(state.generation(), None);
    assert_eq!(state.modified_len_for_framing().unwrap(), Some(3));
}
