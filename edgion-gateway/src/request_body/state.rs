//! Request-scoped body state exposed to the rest of the Gateway.
//!
//! The transport-specific capture/replay backend exists only with the Edgion
//! Pingora fork. This facade remains available in every Gateway flavor so
//! plugins, routing hooks, and context lifecycle code never branch on the
//! selected Pingora source.

use bytes::Bytes;

use crate::plugins::runtime::traits::session::{BodyReadError, BodyReadResult};

#[cfg(feature = "body-buffering")]
mod enabled;
#[cfg(all(feature = "upstream-pingora", not(feature = "body-buffering")))]
mod stock;

#[cfg(feature = "body-buffering")]
use enabled::Backend;
#[cfg(all(feature = "upstream-pingora", not(feature = "body-buffering")))]
use stock::Backend;

#[cfg(feature = "body-buffering")]
use super::BodyStorageView;
#[cfg(feature = "body-buffering")]
use std::sync::Arc;

/// Request-scoped handle to a completely captured proxied request body.
///
/// A stock-Pingora build uses the same API with an always-empty state. The
/// capability failure is reported by the session adapter when capture is
/// requested; downstream consumers do not need compile-time feature branches.
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

    /// Return the final mutated length that must be reflected in upstream
    /// framing. The value is retained after the read-only view is released so
    /// retries rebuild the same headers.
    pub fn modified_len_for_framing(&mut self) -> Result<Option<u64>, BodyReadError> {
        self.0.modified_len_for_framing()
    }

    /// Release the read-only cache after the final response-header plugin
    /// chain. Retained mutation framing remains available for retries.
    pub fn release_view(&mut self) {
        self.0.release_view();
    }

    #[cfg(feature = "body-buffering")]
    pub(crate) fn install(&mut self, view: Arc<BodyStorageView>) {
        self.0.install(view);
    }

    #[cfg(all(test, feature = "body-buffering"))]
    pub(crate) fn cached_body_is_materialized(&self) -> Option<bool> {
        self.0.cached_body_is_materialized()
    }
}
