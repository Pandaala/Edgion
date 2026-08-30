use bytes::Bytes;
use std::sync::Arc;

use super::super::operation::{
    empty_text_operation, BodyOperationError, BodyScanResult, BodyTextOperation, BodyTextResult, RawBodyScanner,
};
use super::super::BodyStorageView;
use crate::plugins::runtime::traits::session::{BodyReadError, BodyReadResult, RequestBodySnapshotReader};

#[derive(Default)]
pub(super) struct Backend {
    view: Option<Arc<BodyStorageView>>,
    modified_len: Option<u64>,
    known_empty: bool,
}

impl Backend {
    pub(super) fn has_cached_body(&self) -> bool {
        self.view.is_some()
    }

    pub(super) fn cached_len(&self) -> Result<Option<u64>, BodyReadError> {
        self.view.as_ref().map(|view| view.len()).transpose()
    }

    pub(super) fn available_len(&self, known_bodyless: bool) -> Result<u64, BodyReadError> {
        match self.view.as_ref() {
            Some(view) => view.len(),
            None if known_bodyless || self.known_empty => Ok(0),
            None => Err(BodyReadError::NotBuffered),
        }
    }

    pub(super) fn snapshot_reader(
        &self,
        known_bodyless: bool,
    ) -> Result<Box<dyn RequestBodySnapshotReader>, BodyReadError> {
        match self.view.as_ref() {
            Some(view) => view.snapshot_reader(),
            None if known_bodyless || self.known_empty => Ok(super::empty_snapshot_reader()),
            None => Err(BodyReadError::NotBuffered),
        }
    }

    pub(super) fn mark_known_empty(&mut self) {
        self.known_empty = true;
    }

    pub(super) async fn read_all(&self) -> Result<Option<Bytes>, BodyReadError> {
        match self.view.as_ref() {
            Some(view) => view.read_all().await.map(Some),
            None => Ok(None),
        }
    }

    pub(super) async fn replace_slice(&self, range: std::ops::Range<usize>, replacement: Bytes) -> BodyReadResult {
        self.view
            .as_ref()
            .ok_or(BodyReadError::NotBuffered)?
            .replace_slice(range, replacement)
            .await
    }

    pub(super) fn overwrite(&self, body: Bytes) -> Result<(), BodyReadError> {
        self.view
            .as_ref()
            .ok_or(BodyReadError::NotBuffered)?
            .overwrite_body(body)
    }

    pub(super) async fn sha256(&self) -> Result<Option<[u8; 32]>, BodyReadError> {
        match self.view.as_ref() {
            Some(view) => view.sha256().await.map(Some),
            None => Ok(None),
        }
    }

    pub(super) async fn scan_raw<S>(
        &self,
        scanner: S,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<S::Output>, BodyOperationError>
    where
        S: RawBodyScanner,
    {
        match self.view.as_ref() {
            Some(view) => view.scan_raw(scanner, max_scan_bytes).await,
            None if self.known_empty => {
                let value = if scanner.requires_blocking_context() {
                    super::super::storage::run_blocking_scan(move || scanner.finish()).await?
                } else {
                    scanner.finish()?
                };
                Ok(BodyScanResult {
                    value,
                    bytes_scanned: 0,
                })
            }
            None => Err(BodyOperationError::Body(BodyReadError::NotBuffered)),
        }
    }

    pub(super) async fn text_operation(
        &self,
        operation: BodyTextOperation,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<BodyTextResult>, BodyOperationError> {
        match self.view.as_ref() {
            Some(view) => view.text_operation(operation, max_scan_bytes).await,
            None if self.known_empty => empty_text_operation(operation),
            None => Err(BodyOperationError::Body(BodyReadError::NotBuffered)),
        }
    }

    pub(super) async fn read_range(
        &self,
        offset: u64,
        max_bytes: u64,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<Bytes>, BodyOperationError> {
        match self.view.as_ref() {
            Some(view) => view.read_range(offset, max_bytes, max_scan_bytes).await,
            None if self.known_empty && offset == 0 => Ok(BodyScanResult {
                value: Bytes::new(),
                bytes_scanned: 0,
            }),
            None if self.known_empty => Err(BodyOperationError::InvalidRange),
            None => Err(BodyOperationError::Body(BodyReadError::NotBuffered)),
        }
    }

    pub(super) async fn sha256_scan(
        &self,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<[u8; 32]>, BodyOperationError> {
        match self.view.as_ref() {
            Some(view) => view.sha256_scan(max_scan_bytes).await,
            None if self.known_empty => Ok(BodyScanResult {
                value: super::super::EMPTY_SHA256,
                bytes_scanned: 0,
            }),
            None => Err(BodyOperationError::Body(BodyReadError::NotBuffered)),
        }
    }

    pub(super) fn clear(&self) -> Result<(), BodyOperationError> {
        self.view
            .as_ref()
            .ok_or(BodyOperationError::Body(BodyReadError::NotBuffered))?
            .clear()
    }

    pub(super) fn append(&self, bytes: Bytes) -> Result<(), BodyOperationError> {
        self.view
            .as_ref()
            .ok_or(BodyOperationError::Body(BodyReadError::NotBuffered))?
            .append(bytes)
    }

    pub(super) fn prepend(&self, bytes: Bytes) -> Result<(), BodyOperationError> {
        self.view
            .as_ref()
            .ok_or(BodyOperationError::Body(BodyReadError::NotBuffered))?
            .prepend(bytes)
    }

    pub(super) fn remove(&self, range: std::ops::Range<u64>) -> Result<(), BodyOperationError> {
        self.view
            .as_ref()
            .ok_or(BodyOperationError::Body(BodyReadError::NotBuffered))?
            .remove(range)
    }

    pub(super) async fn replace_literal(
        &self,
        from: Bytes,
        to: Bytes,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<()>, BodyOperationError> {
        self.view
            .as_ref()
            .ok_or(BodyOperationError::Body(BodyReadError::NotBuffered))?
            .replace_literal(from, to, max_scan_bytes)
            .await
    }

    pub(super) fn modified_len_for_framing(&mut self) -> Result<Option<u64>, BodyReadError> {
        if let Some(view) = self.view.as_ref() {
            if let Some(len) = view.modified_len() {
                self.modified_len = Some(len);
                return Ok(Some(len));
            }
            return Ok(None);
        }
        Ok(self.modified_len)
    }

    pub(super) fn generation(&self) -> Option<u64> {
        self.view.as_ref().map(|view| view.generation())
    }

    pub(super) fn replace_spans(
        &self,
        expected_generation: u64,
        edits: &[(std::ops::Range<u64>, Bytes)],
    ) -> Result<u64, BodyOperationError> {
        self.view
            .as_ref()
            .ok_or(BodyOperationError::Body(BodyReadError::NotBuffered))?
            .replace_spans(expected_generation, edits)
    }

    pub(super) fn release_view(&mut self) {
        if let Some(len) = self.view.as_ref().and_then(|view| view.modified_len()) {
            self.modified_len = Some(len);
        }
        self.view = None;
    }

    pub(super) fn install(&mut self, view: Arc<BodyStorageView>) {
        self.view = Some(view);
        self.known_empty = false;
    }

    #[cfg(test)]
    pub(super) fn cached_body_is_materialized(&self) -> Option<bool> {
        self.view.as_ref().map(|view| view.is_materialized())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_body::ScanControl;

    struct BlockingFinishScanner {
        runtime_thread: std::thread::ThreadId,
    }

    impl RawBodyScanner for BlockingFinishScanner {
        type Output = bool;

        fn requires_blocking_context(&self) -> bool {
            true
        }

        fn push(&mut self, _: &[u8]) -> Result<ScanControl, BodyOperationError> {
            unreachable!("a known-empty body must not push bytes")
        }

        fn finish(self) -> Result<Self::Output, BodyOperationError> {
            Ok(std::thread::current().id() != self.runtime_thread)
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn known_empty_blocking_scanner_finishes_off_the_runtime_thread() {
        let mut backend = Backend::default();
        backend.mark_known_empty();
        let result = backend
            .scan_raw(
                BlockingFinishScanner {
                    runtime_thread: std::thread::current().id(),
                },
                0,
            )
            .await
            .unwrap();

        assert!(result.value);
        assert_eq!(result.bytes_scanned, 0);
    }
}
