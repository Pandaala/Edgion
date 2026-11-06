//! Kafka fingerprint and [`ApplyOwner`] for the persistent runner.

use std::sync::Arc;

use async_trait::async_trait;
use edgion_common::link_sys_core::outbound_tls::PreparedLinkSysTls;
use edgion_resources::resources::link_sys::kafka::{
    KafkaClientConfig, DEFAULT_KAFKA_CHANNEL_SIZE, DEFAULT_KAFKA_LINGER_MS, DEFAULT_KAFKA_MAX_PENDING_BYTES,
    DEFAULT_KAFKA_MAX_PENDING_RECORDS, DEFAULT_KAFKA_MAX_TOPICS,
};
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use crate::link_sys::credential::{self, CredentialState, ResolvedCredential};
use crate::link_sys::runtime::apply::{
    hash_tls_material, hash_tls_policy, ConnectionCoverage, FingerprintHasher, RuntimeFingerprint,
};
use crate::link_sys::runtime::runner::ApplyOwner;

use super::client::KafkaLinkClient;

pub(crate) struct KafkaApply;

pub(crate) struct KafkaCandidate {
    pub(super) tls: Arc<PreparedLinkSysTls>,
    fingerprint: RuntimeFingerprint,
}

impl KafkaCandidate {
    pub(super) fn new(owner: Arc<LinkSys>) -> anyhow::Result<Self> {
        Self::from_prepared(PreparedLinkSysTls::from_link_sys(owner)?)
    }

    pub(super) fn from_prepared(tls: PreparedLinkSysTls) -> anyhow::Result<Self> {
        anyhow::ensure!(
            matches!(&tls.owner().spec.config, SystemConfig::Kafka(_)),
            "Kafka candidate requires an actual Kafka LinkSys"
        );
        tls.validate_active_inheritance()?;
        let fingerprint = fingerprint_kafka_fields(tls.owner(), |hasher| tls.hash_active_inheritance(hasher))?;
        Ok(Self {
            tls: Arc::new(tls),
            fingerprint,
        })
    }

    pub(super) fn fingerprint(&self) -> &RuntimeFingerprint {
        &self.fingerprint
    }
}

#[async_trait]
impl ApplyOwner for KafkaApply {
    type Client = KafkaLinkClient;
    type Candidate = KafkaCandidate;

    fn prepare(incoming: Arc<LinkSys>) -> anyhow::Result<Self::Candidate> {
        KafkaCandidate::new(incoming)
    }

    fn fingerprint(incoming: &Self::Candidate) -> anyhow::Result<RuntimeFingerprint> {
        Ok(incoming.fingerprint.clone())
    }

    fn fingerprint_live(client: &Self::Client) -> RuntimeFingerprint {
        client.runtime_fingerprint().clone()
    }

    fn coverage(client: &Self::Client) -> ConnectionCoverage {
        client.connection_coverage()
    }

    fn published_health(client: &Self::Client) -> Option<&edgion_common::link_sys_core::PublishedHealth> {
        Some(client.published_health())
    }

    fn build(key: &str, incoming: &Self::Candidate) -> anyhow::Result<Arc<Self::Client>> {
        KafkaLinkClient::from_candidate(key, incoming).map(Arc::new)
    }

    async fn init(client: &Self::Client) -> anyhow::Result<()> {
        KafkaLinkClient::init(client).await
    }

    async fn shutdown(client: Arc<Self::Client>) {
        if let Err(e) = KafkaLinkClient::shutdown(&client).await {
            tracing::warn!(kafka = %client.name(), error = %e, "Kafka shutdown failed");
        }
    }

    fn mark_retired(client: &Self::Client) {
        client.mark_retired();
    }

    fn label() -> &'static str {
        "Kafka"
    }
}

fn fingerprint_kafka_fields(
    ls: &LinkSys,
    inherited: impl FnOnce(&mut FingerprintHasher) -> anyhow::Result<()>,
) -> anyhow::Result<RuntimeFingerprint> {
    let SystemConfig::Kafka(cfg) = &ls.spec.config else {
        anyhow::bail!("fingerprint_kafka called with a non-Kafka LinkSys");
    };

    let mut rebuild = FingerprintHasher::new();
    rebuild.tag(4); // kafka kind tag
    hash_rebuild_fields(&mut rebuild, cfg);

    let mut secret = FingerprintHasher::new();
    hash_auth_material(&mut secret, ls);
    hash_tls_material(&mut secret, cfg.tls.as_ref());
    inherited(&mut secret)?;

    Ok(RuntimeFingerprint::from_parts(
        rebuild.finish(),
        secret.finish(),
        ls.metadata.resource_version.clone(),
    ))
}

fn hash_rebuild_fields(h: &mut FingerprintHasher, cfg: &KafkaClientConfig) {
    let mut brokers = cfg.brokers.clone();
    brokers.sort();
    h.u64(brokers.len() as u64);
    for broker in &brokers {
        h.bytes(broker.as_bytes());
    }

    match cfg.sasl.as_ref() {
        None => h.tag(0),
        Some(sasl) => {
            h.tag(1);
            h.bytes(sasl.username.as_bytes());
        }
    }

    h.u64(cfg.channel_size.unwrap_or(DEFAULT_KAFKA_CHANNEL_SIZE) as u64);
    h.u64(cfg.max_topics.unwrap_or(DEFAULT_KAFKA_MAX_TOPICS) as u64);
    h.u64(cfg.max_pending_records.unwrap_or(DEFAULT_KAFKA_MAX_PENDING_RECORDS) as u64);
    h.u64(cfg.max_pending_bytes.unwrap_or(DEFAULT_KAFKA_MAX_PENDING_BYTES) as u64);
    h.u64(cfg.linger_ms.unwrap_or(DEFAULT_KAFKA_LINGER_MS));

    hash_tls_policy(h, cfg.tls.as_ref());
}

fn hash_auth_material(h: &mut FingerprintHasher, ls: &LinkSys) {
    match credential::resolved_state(ls) {
        CredentialState::NotRequired => h.tag(0),
        CredentialState::Pending => h.tag(1),
        CredentialState::Invalid(_) => h.tag(2),
        CredentialState::Resolved(material) => {
            h.tag(3);
            // Kafka SASL/PLAIN consumes the CRD username (rebuild hash) and the
            // Secret password. Do not hash an unused Secret username.
            match material {
                ResolvedCredential::Basic(basic) => {
                    h.tag(0);
                    h.bytes(basic.password.as_bytes());
                }
                _ => h.tag(1),
            }
        }
    }
}

#[cfg(all(test, feature = "kafka"))]
#[path = "../../../../tests/unit/link_sys/providers/kafka/apply_test.rs"]
mod tests;
