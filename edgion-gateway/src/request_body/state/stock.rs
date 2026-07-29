use bytes::Bytes;
use sha2::Digest;

use super::super::operation::{
    empty_text_operation, BodyOperationError, BodyScanResult, BodyTextOperation, BodyTextResult, RawBodyScanner,
};
use crate::plugins::runtime::traits::session::{BodyReadError, BodyReadResult, RequestBodySnapshotReader};

#[derive(Default)]
pub(super) struct Backend {
    known_empty: bool,
}

impl Backend {
    pub(super) fn has_cached_body(&self) -> bool {
        false
    }

    pub(super) fn cached_len(&self) -> Result<Option<u64>, BodyReadError> {
        Ok(None)
    }

    pub(super) fn available_len(&self, known_bodyless: bool) -> Result<u64, BodyReadError> {
        if known_bodyless || self.known_empty {
            Ok(0)
        } else {
            Err(BodyReadError::Unimplemented)
        }
    }

    pub(super) fn snapshot_reader(
        &self,
        known_bodyless: bool,
    ) -> Result<Box<dyn RequestBodySnapshotReader>, BodyReadError> {
        if known_bodyless || self.known_empty {
            Ok(super::empty_snapshot_reader())
        } else {
            Err(BodyReadError::Unimplemented)
        }
    }

    pub(super) fn mark_known_empty(&mut self) {
        self.known_empty = true;
    }

    pub(super) async fn read_all(&self) -> Result<Option<Bytes>, BodyReadError> {
        Ok(None)
    }

    pub(super) async fn replace_slice(&self, range: std::ops::Range<usize>, replacement: Bytes) -> BodyReadResult {
        let _ = (range, replacement);
        Err(BodyReadError::Unimplemented)
    }

    pub(super) fn overwrite(&self, body: Bytes) -> Result<(), BodyReadError> {
        let _ = body;
        Err(BodyReadError::Unimplemented)
    }

    pub(super) async fn sha256(&self) -> Result<Option<[u8; 32]>, BodyReadError> {
        Ok(None)
    }

    pub(super) async fn scan_raw<S>(
        &self,
        scanner: S,
        _max_scan_bytes: u64,
    ) -> Result<BodyScanResult<S::Output>, BodyOperationError>
    where
        S: RawBodyScanner,
    {
        if self.known_empty {
            Ok(BodyScanResult {
                value: scanner.finish()?,
                bytes_scanned: 0,
            })
        } else {
            Err(BodyOperationError::Body(BodyReadError::Unimplemented))
        }
    }

    pub(super) async fn text_operation(
        &self,
        operation: BodyTextOperation,
        _max_scan_bytes: u64,
    ) -> Result<BodyScanResult<BodyTextResult>, BodyOperationError> {
        if self.known_empty {
            empty_text_operation(operation)
        } else {
            Err(BodyOperationError::Body(BodyReadError::Unimplemented))
        }
    }

    pub(super) async fn read_range(
        &self,
        offset: u64,
        _max_bytes: u64,
        _max_scan_bytes: u64,
    ) -> Result<BodyScanResult<Bytes>, BodyOperationError> {
        if self.known_empty && offset == 0 {
            Ok(BodyScanResult {
                value: Bytes::new(),
                bytes_scanned: 0,
            })
        } else if self.known_empty {
            Err(BodyOperationError::InvalidRange)
        } else {
            Err(BodyOperationError::Body(BodyReadError::Unimplemented))
        }
    }

    pub(super) async fn sha256_scan(
        &self,
        _max_scan_bytes: u64,
    ) -> Result<BodyScanResult<[u8; 32]>, BodyOperationError> {
        if self.known_empty {
            Ok(BodyScanResult {
                value: sha2::Sha256::digest([]).into(),
                bytes_scanned: 0,
            })
        } else {
            Err(BodyOperationError::Body(BodyReadError::Unimplemented))
        }
    }

    pub(super) fn clear(&self) -> Result<(), BodyOperationError> {
        Err(BodyOperationError::Body(BodyReadError::Unimplemented))
    }

    pub(super) fn append(&self, _bytes: Bytes) -> Result<(), BodyOperationError> {
        Err(BodyOperationError::Body(BodyReadError::Unimplemented))
    }

    pub(super) fn prepend(&self, _bytes: Bytes) -> Result<(), BodyOperationError> {
        Err(BodyOperationError::Body(BodyReadError::Unimplemented))
    }

    pub(super) fn remove(&self, _range: std::ops::Range<u64>) -> Result<(), BodyOperationError> {
        Err(BodyOperationError::Body(BodyReadError::Unimplemented))
    }

    pub(super) async fn replace_literal(
        &self,
        _from: Bytes,
        _to: Bytes,
        _max_scan_bytes: u64,
    ) -> Result<BodyScanResult<()>, BodyOperationError> {
        Err(BodyOperationError::Body(BodyReadError::Unimplemented))
    }

    pub(super) fn modified_len_for_framing(&mut self) -> Result<Option<u64>, BodyReadError> {
        Ok(None)
    }

    pub(super) fn release_view(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn known_empty_body_supports_read_only_operations() {
        let mut backend = Backend::default();
        backend.mark_known_empty();

        assert_eq!(backend.available_len(false), Ok(0));
        assert_eq!(
            backend
                .text_operation(BodyTextOperation::IsEmpty, 0)
                .await
                .unwrap()
                .value,
            BodyTextResult::Bool(true)
        );
        assert_eq!(backend.read_range(0, 64, 0).await.unwrap().value, Bytes::new());
        let expected: [u8; 32] = sha2::Sha256::digest([]).into();
        assert_eq!(backend.sha256_scan(0).await.unwrap().value, expected);
    }

    #[tokio::test]
    async fn bodyful_operations_remain_unimplemented() {
        let backend = Backend::default();

        assert_eq!(backend.available_len(false), Err(BodyReadError::Unimplemented));
        assert_eq!(
            backend
                .text_operation(BodyTextOperation::Contains("x".to_string()), 64)
                .await,
            Err(BodyOperationError::Body(BodyReadError::Unimplemented))
        );
        assert_eq!(
            backend.read_range(0, 64, 64).await,
            Err(BodyOperationError::Body(BodyReadError::Unimplemented))
        );
        assert_eq!(
            backend.clear(),
            Err(BodyOperationError::Body(BodyReadError::Unimplemented))
        );
    }
}
