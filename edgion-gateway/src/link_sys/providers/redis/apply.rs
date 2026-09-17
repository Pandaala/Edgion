//! Redis fingerprint and [`ApplyOwner`] for the persistent runner.

use std::sync::Arc;

use async_trait::async_trait;
use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType, WellKnownCACertificates};
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::redis::{
    RedisClientConfig, RedisTopologyMode, RedisTransport, REDIS_DEFAULT_POOL_SIZE,
};
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};
use k8s_openapi::api::core::v1::Secret;

use crate::link_sys::credential::{self, CredentialState};
use crate::link_sys::runtime::apply::{ConnectionCoverage, FingerprintHasher, RuntimeFingerprint};
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

fn hash_tls_policy(h: &mut FingerprintHasher, tls: Option<&TlsConfig>) {
    let Some(tls) = tls else {
        h.tag(0);
        return;
    };
    h.tag(1);
    h.tag(u8::from(tls.enabled));
    if !tls.enabled {
        return;
    }
    h.opt_bool(tls.outbound.verify);
    match tls.outbound.validation.as_ref() {
        None => h.tag(0),
        Some(validation) => {
            h.tag(1);
            h.opt_str(validation.hostname.as_deref());
            match validation.well_known_ca_certificates {
                None => h.tag(0),
                Some(WellKnownCACertificates::System) => h.tag(1),
            }
            match validation.subject_alt_names.as_deref() {
                None => h.tag(0),
                Some(sans) => {
                    h.tag(1);
                    h.u64(sans.len() as u64);
                    for san in sans {
                        hash_san(h, san);
                    }
                }
            }
            match validation.ca_certificate_refs.as_deref() {
                None => h.tag(0),
                Some(refs) => {
                    h.tag(1);
                    h.u64(refs.len() as u64);
                    for r in refs {
                        h.bytes(r.group.as_bytes());
                        h.bytes(r.kind.as_bytes());
                        h.bytes(r.name.as_bytes());
                        h.opt_str(r.namespace.as_deref());
                    }
                }
            }
        }
    }
    match tls.outbound.client_certificate_ref.as_ref() {
        None => h.tag(0),
        Some(r) => {
            h.tag(1);
            h.opt_str(r.group.as_deref());
            h.opt_str(r.kind.as_deref());
            h.bytes(r.name.as_bytes());
            h.opt_str(r.namespace.as_deref());
        }
    }
}

fn hash_san(h: &mut FingerprintHasher, san: &SubjectAltName) {
    h.tag(match san.san_type {
        SubjectAltNameType::Hostname => 0,
        SubjectAltNameType::URI => 1,
    });
    h.opt_str(san.hostname.as_deref());
    h.opt_str(san.uri.as_deref());
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

fn hash_tls_material(h: &mut FingerprintHasher, tls: Option<&TlsConfig>) {
    let Some(tls) = tls else {
        h.tag(0);
        return;
    };
    h.tag(1);
    if !tls.enabled {
        return;
    }
    hash_secrets(h, tls.resolved_ca_certificates.as_deref());
    hash_secret(h, tls.resolved_client_certificate.as_ref());
}

fn hash_secrets(h: &mut FingerprintHasher, secrets: Option<&[Secret]>) {
    match secrets {
        None => h.tag(0),
        Some(secrets) => {
            h.tag(1);
            h.u64(secrets.len() as u64);
            for secret in secrets {
                hash_secret_data(h, secret);
            }
        }
    }
}

fn hash_secret(h: &mut FingerprintHasher, secret: Option<&Secret>) {
    match secret {
        None => h.tag(0),
        Some(secret) => {
            h.tag(1);
            hash_secret_data(h, secret);
        }
    }
}

fn hash_secret_data(h: &mut FingerprintHasher, secret: &Secret) {
    let mut entries: Vec<(&String, &k8s_openapi::ByteString)> = secret
        .data
        .as_ref()
        .map(|data| data.iter().collect())
        .unwrap_or_default();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    h.u64(entries.len() as u64);
    for (key, value) in entries {
        h.bytes(key.as_bytes());
        h.bytes(&value.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::outbound_tls::OutboundTlsLocal;
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot, TlsConfig};
    use edgion_resources::resources::link_sys::redis::{
        RedisAuth, RedisCluster, RedisPool, RedisSentinel, RedisTimeout, RedisTopology,
    };
    use edgion_resources::resources::link_sys::LinkSysSpec;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    fn redis_ls(endpoints: Vec<&str>, rv: Option<&str>) -> LinkSys {
        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some("fp".to_string()),
                name: Some("redis".to_string()),
                resource_version: rv.map(str::to_string),
                ..Default::default()
            },
            spec: LinkSysSpec {
                unknown_fields: Vec::new(),
                config: SystemConfig::Redis(RedisClientConfig {
                    endpoints: endpoints.into_iter().map(str::to_string).collect(),
                    auth: None,
                    db: None,
                    timeout: None,
                    pool: None,
                    topology: None,
                    tls: None,
                }),
            },
            status: None,
        }
    }

    fn with_auth(mut ls: LinkSys, username: Option<&str>, password: &[u8]) -> LinkSys {
        let SystemConfig::Redis(cfg) = &mut ls.spec.config else {
            panic!("redis");
        };
        let mut data = BTreeMap::new();
        if let Some(username) = username {
            data.insert("username".to_string(), ByteString(username.as_bytes().to_vec()));
        }
        data.insert("password".to_string(), ByteString(password.to_vec()));
        cfg.auth = Some(RedisAuth {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "creds".to_string(),
                    namespace: None,
                },
                secret: Some(k8s_openapi::api::core::v1::Secret {
                    data: Some(data),
                    ..Default::default()
                }),
            },
        });
        ls
    }

    fn with_password(ls: LinkSys, password: &[u8]) -> LinkSys {
        with_auth(ls, None, password)
    }

    fn with_tls(mut ls: LinkSys, enabled: bool, verify: Option<bool>, ca: Option<&[u8]>) -> LinkSys {
        let SystemConfig::Redis(cfg) = &mut ls.spec.config else {
            panic!("redis");
        };
        cfg.tls = Some(TlsConfig {
            enabled,
            outbound: OutboundTlsLocal {
                verify,
                validation: None,
                client_certificate_ref: None,
            },
            resolved_ca_certificates: ca.map(|bytes| {
                let mut data = BTreeMap::new();
                data.insert("ca.crt".to_string(), ByteString(bytes.to_vec()));
                vec![k8s_openapi::api::core::v1::Secret {
                    data: Some(data),
                    ..Default::default()
                }]
            }),
            resolved_client_certificate: None,
        });
        ls
    }

    #[test]
    fn endpoint_order_does_not_change_cluster_fingerprint() {
        let mut left = redis_ls(vec!["redis://b.example:6379", "redis://a.example:6379"], None);
        let mut right = redis_ls(vec!["redis://a.example:6379", "redis://b.example:6379"], None);
        for ls in [&mut left, &mut right] {
            if let SystemConfig::Redis(cfg) = &mut ls.spec.config {
                cfg.topology = Some(RedisTopology {
                    mode: RedisTopologyMode::Cluster,
                    sentinel: None,
                    cluster: Some(RedisCluster { max_redirects: None }),
                });
            }
        }
        assert!(fingerprint_redis(&left)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&right).unwrap()));
    }

    #[test]
    fn rediss_transport_differs_from_redis() {
        let plain = redis_ls(vec!["redis://a.example:6379"], None);
        let tls = redis_ls(vec!["rediss://a.example:6379"], None);
        assert!(!fingerprint_redis(&plain)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&tls).unwrap()));
    }

    #[test]
    fn pool_size_is_rebuild() {
        let mut small = redis_ls(vec!["redis://a.example:6379"], None);
        let mut large = small.clone();
        if let SystemConfig::Redis(cfg) = &mut small.spec.config {
            cfg.pool = Some(RedisPool { size: Some(2) });
        }
        if let SystemConfig::Redis(cfg) = &mut large.spec.config {
            cfg.pool = Some(RedisPool { size: Some(8) });
        }
        assert!(!fingerprint_redis(&small)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&large).unwrap()));
    }

    #[test]
    fn default_pool_matches_explicit_eight() {
        let implicit = redis_ls(vec!["redis://a.example:6379"], None);
        let mut explicit = implicit.clone();
        if let SystemConfig::Redis(cfg) = &mut explicit.spec.config {
            cfg.pool = Some(RedisPool {
                size: Some(REDIS_DEFAULT_POOL_SIZE),
            });
        }
        assert!(fingerprint_redis(&implicit)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&explicit).unwrap()));
    }

    #[test]
    fn secret_bytes_change_secret_hash() {
        let a = with_password(redis_ls(vec!["redis://a.example:6379"], Some("10")), b"pw-a");
        let b = with_password(redis_ls(vec!["redis://a.example:6379"], Some("10")), b"pw-b");
        let fa = fingerprint_redis(&a).unwrap();
        let fb = fingerprint_redis(&b).unwrap();
        assert!(!fa.equal_runtime(&fb));
        assert!(!fa.rv_skip_allows_reuse(&fb));
    }

    #[test]
    fn resource_version_is_not_a_rebuild_field() {
        let a = redis_ls(vec!["redis://a.example:6379"], Some("1"));
        let b = redis_ls(vec!["redis://a.example:6379"], Some("2"));
        assert!(fingerprint_redis(&a)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&b).unwrap()));
    }

    #[test]
    fn sync_version_annotation_is_ignored() {
        let mut a = redis_ls(vec!["redis://a.example:6379"], Some("1"));
        let mut b = a.clone();
        a.metadata.annotations = Some(
            [("edgion.io/sync-version".to_string(), "aaa".to_string())]
                .into_iter()
                .collect(),
        );
        b.metadata.annotations = Some(
            [("edgion.io/sync-version".to_string(), "bbb".to_string())]
                .into_iter()
                .collect(),
        );
        assert!(fingerprint_redis(&a)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&b).unwrap()));
    }

    #[test]
    fn db_and_timeouts_are_rebuild() {
        let base = redis_ls(vec!["redis://a.example:6379"], None);
        let mut db = base.clone();
        let mut timeout = base.clone();
        if let SystemConfig::Redis(cfg) = &mut db.spec.config {
            cfg.db = Some(1);
        }
        if let SystemConfig::Redis(cfg) = &mut timeout.spec.config {
            cfg.timeout = Some(RedisTimeout {
                connect: Some("3s".to_string()),
                command: Some("5s".to_string()),
            });
        }
        assert!(!fingerprint_redis(&base)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&db).unwrap()));
        assert!(!fingerprint_redis(&base)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&timeout).unwrap()));
    }

    #[test]
    fn username_is_part_of_secret_fingerprint() {
        let a = with_auth(
            redis_ls(vec!["redis://a.example:6379"], Some("10")),
            Some("alice"),
            b"pw",
        );
        let b = with_auth(redis_ls(vec!["redis://a.example:6379"], Some("10")), Some("bob"), b"pw");
        assert!(!fingerprint_redis(&a)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&b).unwrap()));
    }

    #[test]
    fn tls_verify_is_rebuild_when_enabled() {
        let base = redis_ls(vec!["rediss://a.example:6379"], None);
        let verify = with_tls(base.clone(), true, Some(true), None);
        let skip = with_tls(base, true, Some(false), None);
        assert!(!fingerprint_redis(&verify)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&skip).unwrap()));
    }

    #[test]
    fn disabled_tls_ignores_unused_ca_and_verify() {
        let base = redis_ls(vec!["redis://a.example:6379"], None);
        let left = with_tls(base.clone(), false, Some(true), Some(b"ca-a"));
        let right = with_tls(base, false, Some(false), Some(b"ca-b"));
        assert!(fingerprint_redis(&left)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&right).unwrap()));
    }

    #[test]
    fn sentinel_master_name_is_rebuild() {
        fn sentinel_ls(master: &str) -> LinkSys {
            let mut ls = redis_ls(vec![], None);
            if let SystemConfig::Redis(cfg) = &mut ls.spec.config {
                cfg.topology = Some(RedisTopology {
                    mode: RedisTopologyMode::Sentinel,
                    sentinel: Some(RedisSentinel {
                        master_name: master.to_string(),
                        sentinels: vec!["redis://sentinel.example:26379".to_string()],
                    }),
                    cluster: None,
                });
            }
            ls
        }
        assert!(!fingerprint_redis(&sentinel_ls("mymaster"))
            .unwrap()
            .equal_runtime(&fingerprint_redis(&sentinel_ls("other")).unwrap()));
    }

    #[test]
    fn cluster_max_redirects_is_rebuild() {
        let mut left = redis_ls(vec!["redis://a.example:6379"], None);
        let mut right = left.clone();
        for (ls, redirects) in [(&mut left, Some(5)), (&mut right, Some(8))] {
            if let SystemConfig::Redis(cfg) = &mut ls.spec.config {
                cfg.topology = Some(RedisTopology {
                    mode: RedisTopologyMode::Cluster,
                    sentinel: None,
                    cluster: Some(RedisCluster {
                        max_redirects: redirects,
                    }),
                });
            }
        }
        assert!(!fingerprint_redis(&left)
            .unwrap()
            .equal_runtime(&fingerprint_redis(&right).unwrap()));
    }
}
