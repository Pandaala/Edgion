//! Etcd fingerprint and [`ApplyOwner`] for the persistent runner.

use std::sync::Arc;

use async_trait::async_trait;
use edgion_common::link_sys_core::outbound_tls::PreparedLinkSysTls;
use edgion_resources::resources::link_sys::etcd::{normalize_etcd_endpoint, EtcdClientConfig};
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use crate::link_sys::credential::{self, CredentialState};
use crate::link_sys::runtime::apply::{
    hash_tls_material, hash_tls_policy, ConnectionCoverage, FingerprintHasher, RuntimeFingerprint,
};
use crate::link_sys::runtime::health_probe::health_transition_reporter;
use crate::link_sys::runtime::runner::ApplyOwner;
use edgion_common::link_sys_core::providers::etcd::config_mapping::{MAX_DIAL_TIMEOUT_MS, MAX_REQUEST_TIMEOUT_MS};

use super::EtcdLinkClient;

pub(crate) struct EtcdApply;

pub(crate) struct EtcdCandidate {
    pub(super) tls: Arc<PreparedLinkSysTls>,
    fingerprint: RuntimeFingerprint,
}

impl EtcdCandidate {
    pub(super) fn new(owner: Arc<LinkSys>) -> anyhow::Result<Self> {
        Self::from_prepared(PreparedLinkSysTls::from_link_sys(owner)?)
    }
    pub(super) fn from_prepared(tls: PreparedLinkSysTls) -> anyhow::Result<Self> {
        anyhow::ensure!(
            matches!(&tls.owner().spec.config, SystemConfig::Etcd(_)),
            "Etcd candidate requires an actual Etcd LinkSys"
        );
        tls.validate_active_inheritance()?;
        let fingerprint = fingerprint_etcd_fields(tls.owner(), |hasher| tls.hash_active_inheritance(hasher))?;
        Ok(Self {
            tls: Arc::new(tls),
            fingerprint,
        })
    }
}

#[async_trait]
impl ApplyOwner for EtcdApply {
    type Client = EtcdLinkClient;
    type Candidate = EtcdCandidate;

    fn prepare(incoming: Arc<LinkSys>) -> anyhow::Result<Self::Candidate> {
        EtcdCandidate::new(incoming)
    }

    fn fingerprint(incoming: &Self::Candidate) -> anyhow::Result<RuntimeFingerprint> {
        Ok(incoming.fingerprint.clone())
    }

    fn fingerprint_live(client: &Self::Client) -> RuntimeFingerprint {
        let (rebuild, secret, rv) = client.apply_fingerprint_parts();
        RuntimeFingerprint::from_parts(rebuild, secret, rv)
    }

    fn coverage(client: &Self::Client) -> ConnectionCoverage {
        let (live, configured) = client.endpoint_coverage();
        ConnectionCoverage { live, configured }
    }

    fn published_health(client: &Self::Client) -> Option<&edgion_common::link_sys_core::PublishedHealth> {
        Some(client.published_health())
    }

    fn build(key: &str, incoming: &Self::Candidate) -> anyhow::Result<Arc<Self::Client>> {
        let (rebuild, secret, rv) = incoming.fingerprint.clone().into_parts();
        anyhow::ensure!(incoming.tls.owner().key_name_str() == key, "Etcd LinkSys key mismatch");
        Ok(Arc::new(
            EtcdLinkClient::from_prepared_link_sys(Arc::clone(&incoming.tls))?
                .with_gateway_apply_meta(rebuild, secret, rv)
                .with_health_transition_reporter(health_transition_reporter(key)),
        ))
    }

    async fn init(client: &Self::Client) -> anyhow::Result<()> {
        EtcdLinkClient::init(client).await
    }

    async fn shutdown(client: Arc<Self::Client>) {
        if let Err(e) = EtcdLinkClient::shutdown(&client).await {
            tracing::warn!(etcd = %client.name(), error = %e, "Etcd shutdown failed");
        }
    }

    fn mark_retired(client: &Self::Client) {
        client.mark_retired();
    }

    fn label() -> &'static str {
        "Etcd"
    }
}

fn fingerprint_etcd_fields(
    ls: &LinkSys,
    inherited: impl FnOnce(&mut FingerprintHasher) -> anyhow::Result<()>,
) -> anyhow::Result<RuntimeFingerprint> {
    let SystemConfig::Etcd(cfg) = &ls.spec.config else {
        anyhow::bail!("fingerprint_etcd called with a non-Etcd LinkSys");
    };

    let errors = cfg.validation_errors();
    anyhow::ensure!(errors.is_empty(), "Invalid Etcd configuration: {}", errors.join("; "));
    let mut rebuild = FingerprintHasher::new();
    rebuild.tag(2); // etcd kind tag
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

fn hash_rebuild_fields(h: &mut FingerprintHasher, cfg: &EtcdClientConfig) {
    let mut endpoints: Vec<_> = cfg
        .endpoints
        .iter()
        .map(|endpoint| normalize_etcd_endpoint(endpoint).expect("validated Etcd endpoint"))
        .collect();
    endpoints.sort();
    endpoints.dedup();
    h.u64(endpoints.len() as u64);
    for endpoint in &endpoints {
        h.bytes(endpoint.as_bytes());
    }

    h.opt_str(cfg.namespace.as_deref().filter(|namespace| !namespace.is_empty()));

    // Keep-alive is applied only when both time and timeout are set
    // in the shared options core; invalid partial objects are rejected before hashing.
    match cfg.keep_alive.as_ref() {
        Some(ka) => {
            if let (Some(time), Some(timeout)) = (ka.time, ka.timeout) {
                h.tag(1);
                h.u64(time);
                h.u64(timeout);
                h.opt_bool(Some(ka.permit_without_stream.unwrap_or(false)));
            } else {
                h.tag(0);
            }
        }
        None => h.tag(0),
    }

    match cfg
        .timeout
        .as_ref()
        .filter(|timeout| timeout.dial.is_some() || timeout.request.is_some())
    {
        None => h.tag(0),
        Some(timeout) => {
            h.tag(1);
            match timeout.dial {
                None => h.tag(0),
                Some(dial) => {
                    h.tag(1);
                    h.u64(dial.min(MAX_DIAL_TIMEOUT_MS));
                }
            }
            match timeout.request {
                None => h.tag(0),
                Some(request) => {
                    h.tag(1);
                    h.u64(request.min(MAX_REQUEST_TIMEOUT_MS));
                }
            }
        }
    }

    hash_tls_policy(h, cfg.tls.as_ref());
}

fn hash_auth_material(h: &mut FingerprintHasher, ls: &LinkSys) {
    match credential::basic_state(ls) {
        CredentialState::NotRequired => h.tag(0),
        CredentialState::Pending => h.tag(1),
        CredentialState::Invalid(_) => h.tag(2),
        CredentialState::Resolved(material) => {
            h.tag(3);
            h.bytes(material.username.as_deref().unwrap_or("").as_bytes());
            h.bytes(material.password.as_bytes());
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/etcd/apply_test.rs"]
mod tests;
