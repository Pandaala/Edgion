//! Etcd fingerprint and [`ApplyOwner`] for the persistent runner.

use std::sync::Arc;

use async_trait::async_trait;
use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use crate::link_sys::credential::{self, CredentialState};
use crate::link_sys::runtime::apply::{
    hash_tls_material, hash_tls_policy, ConnectionCoverage, FingerprintHasher, RuntimeFingerprint,
};
use crate::link_sys::runtime::health_probe::health_transition_reporter;
use crate::link_sys::runtime::runner::ApplyOwner;
use edgion_common::link_sys_core::providers::etcd::config_mapping::{MAX_DIAL_TIMEOUT_MS, MAX_REQUEST_TIMEOUT_MS};

use super::EtcdLinkClient;

pub struct EtcdApply;

#[async_trait]
impl ApplyOwner for EtcdApply {
    type Client = EtcdLinkClient;

    fn fingerprint(incoming: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
        fingerprint_etcd(incoming)
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

    fn build(key: &str, incoming: &LinkSys) -> anyhow::Result<Arc<Self::Client>> {
        let SystemConfig::Etcd(config) = &incoming.spec.config else {
            anyhow::bail!("Etcd build called with a non-Etcd LinkSys");
        };
        let auth = credential::basic_state(incoming);
        let fingerprint = fingerprint_etcd(incoming)?;
        let (rebuild, secret, rv) = fingerprint.into_parts();
        Ok(Arc::new(
            EtcdLinkClient::from_config(key, config, &auth)?
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

pub fn fingerprint_etcd(ls: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
    let SystemConfig::Etcd(cfg) = &ls.spec.config else {
        anyhow::bail!("fingerprint_etcd called with a non-Etcd LinkSys");
    };

    let mut rebuild = FingerprintHasher::new();
    rebuild.tag(2); // etcd kind tag
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

fn hash_rebuild_fields(h: &mut FingerprintHasher, cfg: &EtcdClientConfig) {
    let mut endpoints = cfg.endpoints.clone();
    endpoints.sort();
    h.u64(endpoints.len() as u64);
    for endpoint in &endpoints {
        h.bytes(endpoint.as_bytes());
    }

    h.opt_str(cfg.namespace.as_deref());

    // Keep-alive is applied only when both time and timeout are set
    // (`build_connect_options`); a partial object is a no-op at runtime.
    match cfg.keep_alive.as_ref() {
        Some(ka) => {
            if let (Some(time), Some(timeout)) = (ka.time, ka.timeout) {
                h.tag(1);
                h.u64(time);
                h.u64(timeout);
                h.opt_bool(ka.permit_without_stream);
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
            // timeout.keep_alive is Ignore (D16).
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
mod tests {
    use super::*;
    use edgion_resources::resources::link_sys::etcd::{EtcdKeepAlive, EtcdObservability, EtcdTimeout};
    use edgion_resources::resources::link_sys::LinkSysSpec;

    fn etcd_ls(endpoints: Vec<&str>, rv: Option<&str>) -> LinkSys {
        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some("fp".to_string()),
                name: Some("etcd".to_string()),
                resource_version: rv.map(str::to_string),
                ..Default::default()
            },
            spec: LinkSysSpec {
                current_status: None,
                unknown_fields: Vec::new(),
                config: SystemConfig::Etcd(EtcdClientConfig {
                    endpoints: endpoints.into_iter().map(str::to_string).collect(),
                    auth: None,
                    tls: None,
                    timeout: None,
                    keep_alive: None,
                    namespace: None,
                    auto_sync_interval: None,
                    max_call_send_size: None,
                    max_call_recv_size: None,
                    user_agent: None,
                    reject_old_cluster: None,
                    observability: None,
                }),
            },
            status: None,
        }
    }

    #[test]
    fn endpoint_order_does_not_change_fingerprint() {
        let left = etcd_ls(vec!["http://b.example:2379", "http://a.example:2379"], None);
        let right = etcd_ls(vec!["http://a.example:2379", "http://b.example:2379"], None);
        assert!(fingerprint_etcd(&left)
            .unwrap()
            .equal_runtime(&fingerprint_etcd(&right).unwrap()));
    }

    #[test]
    fn unmapped_crd_fields_are_ignored() {
        let base = etcd_ls(vec!["http://a.example:2379"], Some("1"));
        let mut ignored = base.clone();
        if let SystemConfig::Etcd(cfg) = &mut ignored.spec.config {
            cfg.auto_sync_interval = Some(30);
            cfg.max_call_send_size = Some(1024);
            cfg.max_call_recv_size = Some(2048);
            cfg.user_agent = Some("other".to_string());
            cfg.reject_old_cluster = Some(true);
            cfg.observability = Some(EtcdObservability {
                metrics: None,
                logging: None,
            });
            cfg.timeout = Some(EtcdTimeout {
                dial: None,
                request: None,
                keep_alive: Some(9),
            });
        }
        assert!(fingerprint_etcd(&base)
            .unwrap()
            .equal_runtime(&fingerprint_etcd(&ignored).unwrap()));
    }

    #[test]
    fn namespace_and_keepalive_are_rebuild() {
        let base = etcd_ls(vec!["http://a.example:2379"], None);
        let mut ns = base.clone();
        let mut ka = base.clone();
        if let SystemConfig::Etcd(cfg) = &mut ns.spec.config {
            cfg.namespace = Some("/edgion".to_string());
        }
        if let SystemConfig::Etcd(cfg) = &mut ka.spec.config {
            cfg.keep_alive = Some(EtcdKeepAlive {
                time: Some(10),
                timeout: Some(5),
                permit_without_stream: Some(true),
            });
        }
        assert!(!fingerprint_etcd(&base)
            .unwrap()
            .equal_runtime(&fingerprint_etcd(&ns).unwrap()));
        assert!(!fingerprint_etcd(&base)
            .unwrap()
            .equal_runtime(&fingerprint_etcd(&ka).unwrap()));
    }

    #[test]
    fn dial_and_request_timeouts_are_rebuild() {
        let base = etcd_ls(vec!["http://a.example:2379"], None);
        let mut timeout = base.clone();
        if let SystemConfig::Etcd(cfg) = &mut timeout.spec.config {
            cfg.timeout = Some(EtcdTimeout {
                dial: Some(3000),
                request: Some(5000),
                keep_alive: None,
            });
        }
        assert!(!fingerprint_etcd(&base)
            .unwrap()
            .equal_runtime(&fingerprint_etcd(&timeout).unwrap()));
    }

    #[test]
    fn incomplete_keepalive_is_a_runtime_noop() {
        let base = etcd_ls(vec!["http://a.example:2379"], None);
        let mut partial = base.clone();
        if let SystemConfig::Etcd(cfg) = &mut partial.spec.config {
            cfg.keep_alive = Some(EtcdKeepAlive {
                time: Some(10),
                timeout: None,
                permit_without_stream: Some(true),
            });
        }
        assert!(fingerprint_etcd(&base)
            .unwrap()
            .equal_runtime(&fingerprint_etcd(&partial).unwrap()));
    }

    #[test]
    fn timeout_fingerprint_uses_clamped_values() {
        let mut a = etcd_ls(vec!["http://a.example:2379"], None);
        let mut b = a.clone();
        if let SystemConfig::Etcd(cfg) = &mut a.spec.config {
            cfg.timeout = Some(EtcdTimeout {
                dial: Some(MAX_DIAL_TIMEOUT_MS),
                request: Some(MAX_REQUEST_TIMEOUT_MS),
                keep_alive: None,
            });
        }
        if let SystemConfig::Etcd(cfg) = &mut b.spec.config {
            cfg.timeout = Some(EtcdTimeout {
                dial: Some(MAX_DIAL_TIMEOUT_MS + 1),
                request: Some(MAX_REQUEST_TIMEOUT_MS + 1),
                keep_alive: None,
            });
        }
        assert!(fingerprint_etcd(&a)
            .unwrap()
            .equal_runtime(&fingerprint_etcd(&b).unwrap()));
    }
}
