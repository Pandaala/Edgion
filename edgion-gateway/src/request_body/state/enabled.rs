use bytes::Bytes;
use std::sync::Arc;

use super::super::BodyStorageView;
use crate::plugins::runtime::traits::session::{BodyReadError, BodyReadResult};

#[derive(Default)]
pub(super) struct Backend {
    view: Option<Arc<BodyStorageView>>,
    modified_len: Option<u64>,
}

impl Backend {
    pub(super) fn has_cached_body(&self) -> bool {
        self.view.is_some()
    }

    pub(super) fn cached_len(&self) -> Result<Option<u64>, BodyReadError> {
        self.view.as_ref().map(|view| view.len()).transpose()
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
            .ok_or(BodyReadError::Incomplete)?
            .replace_slice(range, replacement)
            .await
    }

    pub(super) fn overwrite(&self, body: Bytes) -> Result<(), BodyReadError> {
        self.view
            .as_ref()
            .ok_or(BodyReadError::Incomplete)?
            .overwrite_body(body)
    }

    pub(super) async fn sha256(&self) -> Result<Option<[u8; 32]>, BodyReadError> {
        match self.view.as_ref() {
            Some(view) => view.sha256().await.map(Some),
            None => Ok(None),
        }
    }

    pub(super) fn modified_len_for_framing(&mut self) -> Result<Option<u64>, BodyReadError> {
        if let Some(len) = self.modified_len {
            return Ok(Some(len));
        }
        let Some(view) = self.view.as_ref() else {
            return Ok(None);
        };
        if !view.is_modified() {
            return Ok(None);
        }
        let len = view.len()?;
        self.modified_len = Some(len);
        Ok(Some(len))
    }

    pub(super) fn release_view(&mut self) {
        self.view = None;
    }

    pub(super) fn install(&mut self, view: Arc<BodyStorageView>) {
        self.view = Some(view);
    }

    #[cfg(test)]
    pub(super) fn cached_body_is_materialized(&self) -> Option<bool> {
        self.view.as_ref().map(|view| view.is_materialized())
    }
}
