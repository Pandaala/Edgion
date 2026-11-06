//! OTLP LinkSys client placeholder for builds without the `otlp` feature.
//!
//! The type is uninhabited: construction always fails, so an OTLP LinkSys never
//! publishes a runtime client and every consumer observes it as unavailable.

use std::convert::Infallible;
use std::sync::Arc;

use edgion_common::dns::ResolverSpec;
use edgion_common::link_sys_core::PublishedHealth;
use edgion_resources::resources::link_sys::LinkSys;

use super::apply::OtlpCandidate;
use crate::link_sys::runtime::apply::{ConnectionCoverage, RuntimeFingerprint};
use crate::link_sys::LogMeta;

pub(crate) const OTLP_NOT_COMPILED: &str =
    "OTLP support is not compiled into this gateway build (cargo feature `otlp`)";

type CleanupResult = Result<(), &'static str>;

pub struct OtlpLinkClient {
    never: Infallible,
}

impl OtlpLinkClient {
    pub fn from_link_sys(resource: Arc<LinkSys>) -> anyhow::Result<Arc<Self>> {
        OtlpCandidate::new(resource)?;
        anyhow::bail!(OTLP_NOT_COMPILED)
    }

    #[cfg(feature = "test-util")]
    pub fn from_link_sys_with_resolver(resource: Arc<LinkSys>, _resolver: ResolverSpec) -> anyhow::Result<Arc<Self>> {
        Self::from_link_sys(resource)
    }

    pub(super) fn from_candidate(
        _key: &str,
        candidate: &OtlpCandidate,
        _resolver: ResolverSpec,
    ) -> anyhow::Result<Arc<Self>> {
        // The candidate was fully prepared and fingerprinted; only the client is missing.
        let _ = &candidate.tls;
        anyhow::bail!(OTLP_NOT_COMPILED)
    }

    pub(crate) fn runtime_fingerprint(&self) -> &RuntimeFingerprint {
        match self.never {}
    }

    pub(crate) fn connection_coverage(&self) -> ConnectionCoverage {
        match self.never {}
    }

    pub(crate) fn mark_retired(&self) {
        match self.never {}
    }

    pub async fn init(&self) -> anyhow::Result<()> {
        match self.never {}
    }

    pub fn healthy(&self) -> bool {
        match self.never {}
    }

    pub(crate) fn published_health(&self) -> &PublishedHealth {
        match self.never {}
    }

    pub fn name(&self) -> &str {
        match self.never {}
    }

    pub fn send_with_meta(&self, _data: String, _meta: &LogMeta<'_>) -> anyhow::Result<()> {
        match self.never {}
    }

    pub async fn shutdown(&self) -> CleanupResult {
        match self.never {}
    }
}
