//! OTLP fingerprint and [`ApplyOwner`] for the persistent runner.

use std::sync::Arc;

use async_trait::async_trait;
use edgion_common::link_sys_core::outbound_tls::PreparedOtlpTls;
use edgion_resources::credentials::SecretMaterialResolver;
use edgion_resources::resources::link_sys::otlp::OtlpBearerResolver;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use crate::link_sys::runtime::apply::{
    hash_tls_material, hash_tls_policy, ConnectionCoverage, FingerprintHasher, RuntimeFingerprint,
};
use crate::link_sys::runtime::runner::ApplyOwner;

use super::client::OtlpLinkClient;

pub(crate) struct OtlpApply;

pub(crate) struct OtlpCandidate {
    pub(super) tls: Arc<PreparedOtlpTls>,
    fingerprint: RuntimeFingerprint,
}

impl OtlpCandidate {
    pub(super) fn new(owner: Arc<LinkSys>) -> anyhow::Result<Self> {
        Self::from_prepared(PreparedOtlpTls::from_link_sys(owner)?)
    }

    pub(super) fn from_prepared(tls: PreparedOtlpTls) -> anyhow::Result<Self> {
        let fingerprint = fingerprint_otlp_fields(tls.owner(), |hasher| tls.hash_inheritance(hasher))?;
        Ok(Self {
            tls: Arc::new(tls),
            fingerprint,
        })
    }
}

#[async_trait]
impl ApplyOwner for OtlpApply {
    type Client = OtlpLinkClient;
    type Candidate = OtlpCandidate;

    fn prepare(incoming: Arc<LinkSys>) -> anyhow::Result<Self::Candidate> {
        OtlpCandidate::new(incoming)
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
        OtlpLinkClient::from_candidate(key, incoming, edgion_common::dns::ResolverSpec::System)
    }

    async fn init(client: &Self::Client) -> anyhow::Result<()> {
        OtlpLinkClient::init(client).await
    }

    async fn shutdown(client: Arc<Self::Client>) {
        if let Err(error) = OtlpLinkClient::shutdown(&client).await {
            tracing::warn!(
                link = %client.name(),
                reason = error,
                "OTLP shutdown failed"
            );
        }
    }

    fn mark_retired(client: &Self::Client) {
        client.mark_retired();
    }

    fn label() -> &'static str {
        "OTLP"
    }
}

fn fingerprint_otlp_fields(
    ls: &LinkSys,
    inherited: impl FnOnce(&mut FingerprintHasher),
) -> anyhow::Result<RuntimeFingerprint> {
    let SystemConfig::Otlp(config) = &ls.spec.config else {
        anyhow::bail!("fingerprint_otlp called with a non-OTLP LinkSys");
    };

    let mut rebuild = FingerprintHasher::new();
    rebuild.tag(6); // OTLP kind tag
    rebuild.bytes(config.endpoint.as_bytes());
    rebuild.u64(config.timeout_ms);
    hash_tls_policy(&mut rebuild, config.tls.as_ref());

    let mut secret = FingerprintHasher::new();
    match config.auth.as_ref() {
        None => secret.tag(0),
        Some(_) => {
            secret.tag(1);
            let resolved = edgion_common::link_sys_core::credential::checked_auth_secret(ls)
                .map_err(|_| anyhow::anyhow!("OTLP bearer credential is invalid"))?
                .and_then(|value| value.data.as_ref())
                .ok_or_else(|| anyhow::anyhow!("OTLP bearer credential is unresolved"))?;
            let material = OtlpBearerResolver
                .parse(resolved)
                .map_err(|_| anyhow::anyhow!("OTLP bearer credential is invalid"))?;
            secret.bytes(material.token.as_bytes());
        }
    }
    hash_tls_material(&mut secret, config.tls.as_ref());
    inherited(&mut secret);

    Ok(RuntimeFingerprint::from_parts(
        rebuild.finish(),
        secret.finish(),
        ls.metadata.resource_version.clone(),
    ))
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/otlp/apply_test.rs"]
mod tests;
