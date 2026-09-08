//! Request-scoped body state exposed to the rest of the Gateway.

use async_trait::async_trait;
use bytes::Bytes;

use super::operation::{BodyOperationError, BodyScanResult, BodyTextOperation, BodyTextResult, RawBodyScanner};
use crate::plugins::runtime::traits::session::{BodyReadError, BodyReadResult, RequestBodySnapshotReader};

mod enabled;

use enabled::Backend;

use super::BodyStorageView;
use std::sync::Arc;

/// Request-scoped handle to a completely captured proxied request body.
#[derive(Default)]
pub struct RequestBodyState(Backend);

impl RequestBodyState {
    /// Whether a complete request-body cache is available.
    pub fn has_cached_body(&self) -> bool {
        self.0.has_cached_body()
    }

    /// Length of the cached body, or `None` when no capture exists.
    pub fn cached_len(&self) -> Result<Option<u64>, BodyReadError> {
        self.0.cached_len()
    }

    /// Validate that an opaque Body value has a stable source without
    /// materializing it. Callers may identify a request as known bodyless;
    /// every other missing capture is an error.
    pub fn available_len(&self, known_bodyless: bool) -> Result<u64, BodyReadError> {
        self.0.available_len(known_bodyless)
    }

    /// Capture an owned reader over the current immutable logical snapshot.
    ///
    /// This never initiates body capture or contiguous materialization.
    pub fn snapshot_reader(&self, known_bodyless: bool) -> Result<Box<dyn RequestBodySnapshotReader>, BodyReadError> {
        self.0.snapshot_reader(known_bodyless)
    }

    /// Record a transport-proven empty body. This is the only cached-body
    /// state stock Pingora can represent without buffering support.
    pub fn mark_known_empty(&mut self) {
        self.0.mark_known_empty();
    }

    /// Materialize the cached body, or return `None` when no capture exists.
    pub async fn read_all(&self) -> Result<Option<Bytes>, BodyReadError> {
        self.0.read_all().await
    }

    /// Replace a byte range in the captured body.
    pub async fn replace_slice(&self, range: std::ops::Range<usize>, replacement: Bytes) -> BodyReadResult {
        self.0.replace_slice(range, replacement).await
    }

    /// Replace the complete captured body.
    pub fn overwrite(&self, body: Bytes) -> Result<(), BodyReadError> {
        self.0.overwrite(body)
    }

    /// Hash the cached body without forcing disk-backed storage to materialize.
    pub async fn sha256(&self) -> Result<Option<[u8; 32]>, BodyReadError> {
        self.0.sha256().await
    }

    /// Run an owned raw scanner over an immutable logical snapshot.
    pub async fn scan_raw<S>(
        &self,
        scanner: S,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<S::Output>, BodyOperationError>
    where
        S: RawBodyScanner,
    {
        self.0.scan_raw(scanner, max_scan_bytes).await
    }

    /// Run one of the shared String/Body logical-text operations.
    pub async fn text_operation(
        &self,
        operation: BodyTextOperation,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<BodyTextResult>, BodyOperationError> {
        self.0.text_operation(operation, max_scan_bytes).await
    }

    /// Read an independent raw range without changing replay position.
    pub async fn read_range(
        &self,
        offset: u64,
        max_bytes: u64,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<Bytes>, BodyOperationError> {
        self.0.read_range(offset, max_bytes, max_scan_bytes).await
    }

    /// Hash the logical body and report raw scan accounting.
    pub async fn sha256_scan(&self, max_scan_bytes: u64) -> Result<BodyScanResult<[u8; 32]>, BodyOperationError> {
        self.0.sha256_scan(max_scan_bytes).await
    }

    pub fn clear(&self) -> Result<(), BodyOperationError> {
        self.0.clear()
    }

    pub fn append(&self, bytes: Bytes) -> Result<(), BodyOperationError> {
        self.0.append(bytes)
    }

    pub fn prepend(&self, bytes: Bytes) -> Result<(), BodyOperationError> {
        self.0.prepend(bytes)
    }

    pub fn remove(&self, range: std::ops::Range<u64>) -> Result<(), BodyOperationError> {
        self.0.remove(range)
    }

    pub async fn replace_literal(
        &self,
        from: Bytes,
        to: Bytes,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<()>, BodyOperationError> {
        self.0.replace_literal(from, to, max_scan_bytes).await
    }

    /// Return the final mutated length that must be reflected in upstream
    /// framing. The value is retained after the read-only view is released so
    /// retries rebuild the same headers.
    pub fn modified_len_for_framing(&mut self) -> Result<Option<u64>, BodyReadError> {
        self.0.modified_len_for_framing()
    }

    /// Current logical snapshot generation, when a view is installed.
    pub fn generation(&self) -> Option<u64> {
        self.0.generation()
    }

    /// Apply several disjoint replacements as one atomic mutation.
    pub fn replace_spans(
        &self,
        expected_generation: u64,
        edits: &[(std::ops::Range<u64>, Bytes)],
    ) -> Result<u64, BodyOperationError> {
        self.0.replace_spans(expected_generation, edits)
    }

    /// Release the plugin-readable cache after the final response-header
    /// chain. Retained mutation framing remains available for retries.
    pub fn release_view(&mut self) {
        self.0.release_view();
    }

    pub(crate) fn install(&mut self, view: Arc<BodyStorageView>) {
        self.0.install(view);
    }

    #[cfg(test)]
    pub(crate) fn cached_body_is_materialized(&self) -> Option<bool> {
        self.0.cached_body_is_materialized()
    }
}

pub(super) fn empty_snapshot_reader() -> Box<dyn RequestBodySnapshotReader> {
    Box::new(EmptySnapshotReader)
}

struct EmptySnapshotReader;

#[async_trait]
impl RequestBodySnapshotReader for EmptySnapshotReader {
    fn len(&self) -> u64 {
        0
    }

    fn memory_bytes_to_isolate(&self) -> Option<usize> {
        Some(0)
    }

    fn isolate_memory(&mut self) -> Result<(), BodyReadError> {
        Ok(())
    }

    async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
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
}
