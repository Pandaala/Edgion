//! Redis fingerprint and [`ApplyOwner`] for the persistent runner.

use std::sync::Arc;

use async_trait::async_trait;
use edgion_resources::resources::link_sys::redis::{
    RedisClientConfig, RedisTopologyMode, RedisTransport, REDIS_DEFAULT_POOL_SIZE,
};
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use crate::link_sys::credential::{self, CredentialState};
use crate::link_sys::runtime::apply::{
    hash_tls_material, hash_tls_policy, ConnectionCoverage, FingerprintHasher, RuntimeFingerprint,
};
use crate::link_sys::runtime::runner::ApplyOwner;

use super::client::RedisLinkClient;

pub struct RedisApply;

#[async_trait]
impl ApplyOwner for RedisApply {
    type Client = RedisLinkClient;

    fn fingerprint(incoming: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
        fingerprint_redis(incoming)
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
        RedisLinkClient::from_linksys(key, incoming)
    }

    async fn init(client: &Self::Client) -> anyhow::Result<()> {
        RedisLinkClient::init(client).await
    }

    async fn shutdown(client: Arc<Self::Client>) {
        if let Err(e) = RedisLinkClient::shutdown(&client).await {
            tracing::warn!(redis = %client.name(), error = %e, "Redis shutdown failed");
        }
    }

    fn mark_retired(client: &Self::Client) {
        client.mark_retired();
    }

    fn label() -> &'static str {
        "Redis"
    }
}

pub fn fingerprint_redis(ls: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
    let SystemConfig::Redis(cfg) = &ls.spec.config else {
        anyhow::bail!("fingerprint_redis called with a non-Redis LinkSys");
    };

    let mut rebuild = FingerprintHasher::new();
    rebuild.tag(1); // redis kind tag
    hash_rebuild_fields(&mut rebuild, cfg)?;

    let mut secret = FingerprintHasher::new();
    hash_auth_material(&mut secret, ls);
    hash_tls_material(&mut secret, cfg.tls.as_ref());

    Ok(RuntimeFingerprint::from_parts(
        rebuild.finish(),
        secret.finish(),
        ls.metadata.resource_version.clone(),
    ))
}

fn hash_rebuild_fields(h: &mut FingerprintHasher, cfg: &RedisClientConfig) -> anyhow::Result<()> {
    let mut endpoints = cfg
        .parsed_active_endpoints()
        .map_err(|errors| anyhow::anyhow!(errors.join("; ")))?;
    endpoints.sort_by(|a, b| {
        (&a.host, a.port, transport_tag(a.transport)).cmp(&(&b.host, b.port, transport_tag(b.transport)))
    });
    h.u64(endpoints.len() as u64);
    for endpoint in &endpoints {
        h.bytes(endpoint.host.as_bytes());
        h.u64(u64::from(endpoint.port));
        h.tag(transport_tag(endpoint.transport));
    }

    match cfg.topology.as_ref() {
        None => h.tag(0),
        Some(topology) => {
            h.tag(1);
            h.tag(match topology.mode {
                RedisTopologyMode::Standalone => 0,
                RedisTopologyMode::Sentinel => 1,
                RedisTopologyMode::Cluster => 2,
            });
            match topology.sentinel.as_ref() {
                None => h.tag(0),
                Some(sentinel) => {
                    h.tag(1);
                    h.bytes(sentinel.master_name.as_bytes());
                }
            }
            match topology.cluster.as_ref().and_then(|cluster| cluster.max_redirects) {
                None => h.tag(0),
                Some(max_redirects) => {
                    h.tag(1);
                    h.u64(u64::from(max_redirects));
                }
            }
        }
    }

    h.u64(u64::from(cfg.db.unwrap_or(0).max(0) as u32));
    h.u64(u64::from(
        cfg.pool
            .as_ref()
            .and_then(|pool| pool.size)
            .unwrap_or(REDIS_DEFAULT_POOL_SIZE),
    ));

    let connect = cfg.connect_timeout().map_err(anyhow::Error::msg)?;
    let command = cfg.command_timeout().map_err(anyhow::Error::msg)?;
    h.u64(connect.as_millis() as u64);
    h.u64(command.as_millis() as u64);

    hash_tls_policy(h, cfg.tls.as_ref());
    Ok(())
}

fn transport_tag(transport: RedisTransport) -> u8 {
    match transport {
        RedisTransport::Plaintext => 0,
        RedisTransport::Tls => 1,
    }
}

fn hash_auth_material(h: &mut FingerprintHasher, ls: &LinkSys) {
    match credential::basic_state(ls) {
        CredentialState::NotRequired => h.tag(0),
        CredentialState::Pending => h.tag(1),
        CredentialState::Invalid(_) => h.tag(2),
        CredentialState::Resolved(material) => {
            h.tag(3);
            h.opt_str(material.username.as_deref());
            h.bytes(material.password.as_bytes());
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/redis/apply_test.rs"]
mod tests;
