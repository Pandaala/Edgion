use bytes::Bytes;

use crate::plugins::runtime::traits::session::{BodyReadError, BodyReadResult};

#[derive(Default)]
pub(super) struct Backend;

impl Backend {
    pub(super) fn has_cached_body(&self) -> bool {
        false
    }

    pub(super) fn cached_len(&self) -> Result<Option<u64>, BodyReadError> {
        Ok(None)
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

    pub(super) fn modified_len_for_framing(&mut self) -> Result<Option<u64>, BodyReadError> {
        Ok(None)
    }

    pub(super) fn release_view(&mut self) {}
}
