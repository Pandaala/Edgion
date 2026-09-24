//! Kafka fingerprint and [`ApplyOwner`] for the persistent runner.

use std::sync::Arc;

use async_trait::async_trait;
use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType, WellKnownCACertificates};
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::kafka::{
    KafkaClientConfig, DEFAULT_KAFKA_CHANNEL_SIZE, DEFAULT_KAFKA_LINGER_MS, DEFAULT_KAFKA_MAX_PENDING_BYTES,
    DEFAULT_KAFKA_MAX_PENDING_RECORDS, DEFAULT_KAFKA_MAX_TOPICS,
};
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};
use k8s_openapi::api::core::v1::Secret;

use crate::link_sys::credential::{self, CredentialState, ResolvedCredential};
use crate::link_sys::runtime::apply::{ConnectionCoverage, FingerprintHasher, RuntimeFingerprint};
use crate::link_sys::runtime::runner::ApplyOwner;

use super::client::KafkaLinkClient;

pub struct KafkaApply;

#[async_trait]
impl ApplyOwner for KafkaApply {
    type Client = KafkaLinkClient;

    fn fingerprint(incoming: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
        fingerprint_kafka(incoming)
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
        KafkaLinkClient::from_linksys(key, incoming)
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

pub fn fingerprint_kafka(ls: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
    let SystemConfig::Kafka(cfg) = &ls.spec.config else {
        anyhow::bail!("fingerprint_kafka called with a non-Kafka LinkSys");
    };

    let mut rebuild = FingerprintHasher::new();
    rebuild.tag(4); // kafka kind tag
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
                Some(WellKnownCACertificates::Other(ref name)) => {
                    h.tag(2);
                    h.bytes(name.as_bytes());
                }
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
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    use edgion_resources::resources::link_sys::kafka::KafkaSaslConfig;
    use edgion_resources::resources::link_sys::LinkSysSpec;

    fn kafka_ls(brokers: Vec<&str>, linger_ms: Option<u64>, rv: Option<&str>) -> LinkSys {
        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some("fp".to_string()),
                name: Some("kafka".to_string()),
                resource_version: rv.map(str::to_string),
                ..Default::default()
            },
            spec: LinkSysSpec {
                current_status: None,
                unknown_fields: Vec::new(),
                config: SystemConfig::Kafka(KafkaClientConfig {
                    brokers: brokers.into_iter().map(str::to_string).collect(),
                    sasl: None,
                    tls: None,
                    channel_size: None,
                    max_topics: None,
                    max_pending_records: None,
                    max_pending_bytes: None,
                    linger_ms,
                }),
            },
            status: None,
        }
    }

    #[test]
    fn broker_order_does_not_change_fingerprint() {
        let left = kafka_ls(vec!["b.example:9092", "a.example:9092"], None, None);
        let right = kafka_ls(vec!["a.example:9092", "b.example:9092"], None, None);
        assert!(fingerprint_kafka(&left)
            .unwrap()
            .equal_runtime(&fingerprint_kafka(&right).unwrap()));
    }

    #[test]
    fn defaulted_linger_matches_explicit_default() {
        let base = kafka_ls(vec!["a.example:9092"], None, None);
        let explicit = kafka_ls(vec!["a.example:9092"], Some(DEFAULT_KAFKA_LINGER_MS), None);
        assert!(fingerprint_kafka(&base)
            .unwrap()
            .equal_runtime(&fingerprint_kafka(&explicit).unwrap()));
    }

    #[test]
    fn linger_change_rebuilds() {
        let a = kafka_ls(vec!["a.example:9092"], Some(5), None);
        let b = kafka_ls(vec!["a.example:9092"], Some(10), None);
        assert!(!fingerprint_kafka(&a)
            .unwrap()
            .equal_runtime(&fingerprint_kafka(&b).unwrap()));
    }

    #[test]
    fn sasl_username_is_in_rebuild_hash() {
        let mut a = kafka_ls(vec!["a.example:9092"], None, None);
        let mut b = a.clone();
        if let SystemConfig::Kafka(cfg) = &mut a.spec.config {
            cfg.sasl = Some(KafkaSaslConfig {
                username: "alice".to_string(),
                password: SecretSlot {
                    secret_ref: SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "creds".to_string(),
                        namespace: None,
                    },
                    secret: None,
                },
            });
        }
        if let SystemConfig::Kafka(cfg) = &mut b.spec.config {
            cfg.sasl = Some(KafkaSaslConfig {
                username: "bob".to_string(),
                password: SecretSlot {
                    secret_ref: SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "creds".to_string(),
                        namespace: None,
                    },
                    secret: None,
                },
            });
        }
        assert!(!fingerprint_kafka(&a)
            .unwrap()
            .equal_runtime(&fingerprint_kafka(&b).unwrap()));
    }
}
