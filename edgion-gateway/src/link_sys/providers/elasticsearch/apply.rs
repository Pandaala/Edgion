//! Elasticsearch fingerprint and [`ApplyOwner`] for the persistent runner.

use std::sync::Arc;

use async_trait::async_trait;
use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchClientConfig;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use crate::link_sys::credential::{self, CredentialState, ResolvedCredential};
use crate::link_sys::runtime::apply::{
    hash_tls_material, hash_tls_policy, ConnectionCoverage, FingerprintHasher, RuntimeFingerprint,
};
use crate::link_sys::runtime::runner::ApplyOwner;

use super::client::EsLinkClient;
use super::config_mapping::{es_pool, es_timeouts, EsBulkConfig};

pub struct EsApply;

#[async_trait]
impl ApplyOwner for EsApply {
    type Client = EsLinkClient;

    fn fingerprint(incoming: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
        fingerprint_es(incoming)
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

    fn build(key: &str, incoming: &LinkSys) -> anyhow::Result<Arc<Self::Client>> {
        EsLinkClient::from_linksys(key, incoming)
    }

    async fn init(client: &Self::Client) -> anyhow::Result<()> {
        EsLinkClient::init(client).await
    }

    async fn shutdown(client: Arc<Self::Client>) {
        if let Err(e) = EsLinkClient::shutdown(&client).await {
            tracing::warn!(es = %client.name(), error = %e, "Elasticsearch shutdown failed");
        }
    }

    fn mark_retired(client: &Self::Client) {
        client.mark_retired();
    }

    fn label() -> &'static str {
        "Elasticsearch"
    }
}

pub fn fingerprint_es(ls: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
    let SystemConfig::Elasticsearch(cfg) = &ls.spec.config else {
        anyhow::bail!("fingerprint_es called with a non-Elasticsearch LinkSys");
    };

    let mut rebuild = FingerprintHasher::new();
    rebuild.tag(3); // elasticsearch kind tag
    hash_rebuild_fields(&mut rebuild, cfg);

    let mut secret = FingerprintHasher::new();
    hash_auth_material(&mut secret, ls);
    hash_tls_material(&mut secret, cfg.tls.as_ref());

    Ok(RuntimeFingerprint::from_parts(
        rebuild.finish(),
        secret.finish(),
        ls.metadata.resource_version.clone(),
    ))
}

fn hash_rebuild_fields(h: &mut FingerprintHasher, cfg: &ElasticsearchClientConfig) {
    let mut endpoints = cfg.endpoints.clone();
    endpoints.sort();
    h.u64(endpoints.len() as u64);
    for endpoint in &endpoints {
        h.bytes(endpoint.as_bytes());
    }

    let (connect, request) = es_timeouts(cfg);
    h.u64(connect.as_millis() as u64);
    h.u64(request.as_millis() as u64);

    let (idle_per_host, idle_timeout) = es_pool(cfg);
    h.u64(idle_per_host as u64);
    h.u64(idle_timeout.as_millis() as u64);

    let bulk = EsBulkConfig::from_crd(cfg);
    h.u64(bulk.batch_size as u64);
    h.u64(bulk.flush_interval.as_millis() as u64);
    h.u64(u64::from(bulk.max_retries));
    h.u64(bulk.backoff.as_millis() as u64);
    h.u64(bulk.max_body_bytes as u64);
    h.u64(bulk.channel_size as u64);
    h.bytes(bulk.index_prefix.as_bytes());
    h.bytes(bulk.date_pattern.as_bytes());

    hash_tls_policy(h, cfg.tls.as_ref());
}

fn hash_auth_material(h: &mut FingerprintHasher, ls: &LinkSys) {
    match credential::resolved_state(ls) {
        CredentialState::NotRequired => h.tag(0),
        CredentialState::Pending => h.tag(1),
        CredentialState::Invalid(_) => h.tag(2),
        CredentialState::Resolved(material) => {
            h.tag(3);
            match material {
                ResolvedCredential::Basic(basic) => {
                    h.tag(0);
                    h.bytes(basic.username.as_deref().unwrap_or("").as_bytes());
                    h.bytes(basic.password.as_bytes());
                }
                ResolvedCredential::EsApiKey(key) => {
                    h.tag(1);
                    h.bytes(key.api_key_id.as_bytes());
                    h.bytes(key.api_key_secret.as_bytes());
                }
                ResolvedCredential::Bearer(bearer) => {
                    h.tag(2);
                    h.bytes(bearer.token.as_bytes());
                }
                ResolvedCredential::Hmac(hmac) => {
                    h.tag(3);
                    h.bytes(hmac.key.as_bytes());
                }
                ResolvedCredential::WebhookApiKey(key) => {
                    h.tag(4);
                    h.bytes(key.api_key.as_bytes());
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/elasticsearch/apply_test.rs"]
mod tests;
