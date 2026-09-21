//! Elasticsearch fingerprint and [`ApplyOwner`] for the persistent runner.

use std::sync::Arc;

use async_trait::async_trait;
use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType, WellKnownCACertificates};
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchClientConfig;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};
use k8s_openapi::api::core::v1::Secret;

use crate::link_sys::credential::{self, CredentialState, ResolvedCredential};
use crate::link_sys::runtime::apply::{ConnectionCoverage, FingerprintHasher, RuntimeFingerprint};
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
    use edgion_resources::resources::link_sys::elasticsearch::{ElasticsearchClientConfig, EsTimeout};
    use edgion_resources::resources::link_sys::LinkSysSpec;

    fn es_ls(endpoints: Vec<&str>, rv: Option<&str>) -> LinkSys {
        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some("fp".to_string()),
                name: Some("es".to_string()),
                resource_version: rv.map(str::to_string),
                ..Default::default()
            },
            spec: LinkSysSpec {
                current_status: None,
                unknown_fields: Vec::new(),
                config: SystemConfig::Elasticsearch(ElasticsearchClientConfig {
                    endpoints: endpoints.into_iter().map(str::to_string).collect(),
                    auth: None,
                    tls: None,
                    timeout: None,
                    pool: None,
                    bulk: None,
                    index: None,
                }),
            },
            status: None,
        }
    }

    #[test]
    fn endpoint_order_does_not_change_fingerprint() {
        let left = es_ls(vec!["http://b.example:9200", "http://a.example:9200"], None);
        let right = es_ls(vec!["http://a.example:9200", "http://b.example:9200"], None);
        assert!(fingerprint_es(&left)
            .unwrap()
            .equal_runtime(&fingerprint_es(&right).unwrap()));
    }

    #[test]
    fn defaulted_timeouts_match_explicit_defaults() {
        let base = es_ls(vec!["http://a.example:9200"], None);
        let mut explicit = base.clone();
        if let SystemConfig::Elasticsearch(cfg) = &mut explicit.spec.config {
            cfg.timeout = Some(EsTimeout {
                connect: Some(5_000),
                request: Some(30_000),
            });
        }
        assert!(fingerprint_es(&base)
            .unwrap()
            .equal_runtime(&fingerprint_es(&explicit).unwrap()));
    }

    #[test]
    fn timeout_clamp_does_not_change_fingerprint() {
        let mut a = es_ls(vec!["http://a.example:9200"], None);
        let mut b = a.clone();
        if let SystemConfig::Elasticsearch(cfg) = &mut a.spec.config {
            cfg.timeout = Some(EsTimeout {
                connect: Some(30_000),
                request: Some(120_000),
            });
        }
        if let SystemConfig::Elasticsearch(cfg) = &mut b.spec.config {
            cfg.timeout = Some(EsTimeout {
                connect: Some(30_001),
                request: Some(120_001),
            });
        }
        assert!(fingerprint_es(&a).unwrap().equal_runtime(&fingerprint_es(&b).unwrap()));
    }
}
