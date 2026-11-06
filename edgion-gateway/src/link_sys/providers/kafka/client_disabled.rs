//! Kafka LinkSys client placeholder for builds without the `kafka` feature.
//!
//! The type is uninhabited: construction always fails, so a Kafka LinkSys never
//! publishes a runtime client and every consumer observes it as unavailable.

use std::convert::Infallible;
use std::sync::Arc;

use anyhow::Result;
use edgion_common::link_sys_core::PublishedHealth;
use edgion_resources::resources::link_sys::LinkSys;

use crate::link_sys::runtime::apply::{ConnectionCoverage, RuntimeFingerprint};

pub(crate) const KAFKA_NOT_COMPILED: &str =
    "Kafka support is not compiled into this gateway build (cargo feature `kafka`)";

pub struct KafkaLinkClient {
    never: Infallible,
}

impl KafkaLinkClient {
    pub fn from_link_sys(owner: Arc<LinkSys>) -> Result<Self> {
        super::apply::KafkaCandidate::new(owner)?;
        anyhow::bail!(KAFKA_NOT_COMPILED)
    }

    pub(super) fn from_candidate(_name: &str, candidate: &super::apply::KafkaCandidate) -> Result<Self> {
        // The candidate was fully prepared and fingerprinted; only the client is missing.
        let _ = (&candidate.tls, candidate.fingerprint());
        anyhow::bail!(KAFKA_NOT_COMPILED)
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

    pub fn name(&self) -> &str {
        match self.never {}
    }

    pub(crate) fn published_health(&self) -> &PublishedHealth {
        match self.never {}
    }

    pub fn healthy(&self) -> bool {
        match self.never {}
    }

    pub fn try_produce(&self, _topic: &str, _data: String) -> Result<()> {
        match self.never {}
    }

    pub async fn init(&self) -> Result<()> {
        match self.never {}
    }

    pub async fn shutdown(&self) -> Result<()> {
        match self.never {}
    }
}
