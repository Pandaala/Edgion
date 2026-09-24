//! OTLP fingerprint and [`ApplyOwner`] for the persistent runner.

use std::sync::Arc;

use async_trait::async_trait;
use edgion_resources::credentials::SecretMaterialResolver;
use edgion_resources::resources::link_sys::otlp::OtlpBearerResolver;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use crate::link_sys::runtime::apply::{
    hash_tls_material, hash_tls_policy, ConnectionCoverage, FingerprintHasher, RuntimeFingerprint,
};
use crate::link_sys::runtime::runner::ApplyOwner;

use super::client::OtlpLinkClient;

pub struct OtlpApply;

#[async_trait]
impl ApplyOwner for OtlpApply {
    type Client = OtlpLinkClient;

    fn fingerprint(incoming: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
        fingerprint_otlp(incoming)
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
        OtlpLinkClient::from_linksys(key, incoming)
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

pub fn fingerprint_otlp(ls: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
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
        Some(auth) => {
            secret.tag(1);
            let resolved = auth
                .slot
                .secret
                .as_ref()
                .and_then(|value| value.data.as_ref())
                .ok_or_else(|| anyhow::anyhow!("OTLP bearer credential is unresolved"))?;
            let material = OtlpBearerResolver
                .parse(resolved)
                .map_err(|_| anyhow::anyhow!("OTLP bearer credential is invalid"))?;
            secret.bytes(material.token.as_bytes());
        }
    }
    hash_tls_material(&mut secret, config.tls.as_ref());

    Ok(RuntimeFingerprint::from_parts(
        rebuild.finish(),
        secret.finish(),
        ls.metadata.resource_version.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use edgion_resources::resources::common::outbound_tls::OutboundTlsLocal;
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot, TlsConfig};
    use edgion_resources::resources::link_sys::otlp::{OtlpAuth, OtlpClientConfig};
    use edgion_resources::resources::link_sys::{LinkSysLocalStatus, LinkSysSpec};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;

    use super::*;

    fn otlp_ls(endpoint: &str, timeout_ms: u64, rv: Option<&str>) -> LinkSys {
        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some("fp".to_string()),
                name: Some("otlp".to_string()),
                resource_version: rv.map(str::to_string),
                ..Default::default()
            },
            spec: LinkSysSpec {
                current_status: None,
                unknown_fields: Vec::new(),
                config: SystemConfig::Otlp(OtlpClientConfig {
                    endpoint: endpoint.to_string(),
                    timeout_ms,
                    auth: None,
                    tls: None,
                }),
            },
            status: None,
        }
    }

    fn with_auth(mut ls: LinkSys, token: &[u8]) -> LinkSys {
        let SystemConfig::Otlp(config) = &mut ls.spec.config else {
            panic!("OTLP fixture");
        };
        let data = BTreeMap::from([("bearerToken".to_string(), ByteString(token.to_vec()))]);
        config.auth = Some(OtlpAuth {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "auth".to_string(),
                    namespace: None,
                },
                secret: Some(Secret {
                    data: Some(data),
                    ..Default::default()
                }),
            },
        });
        ls
    }

    #[test]
    fn endpoint_and_timeout_are_rebuild_fields() {
        let base = otlp_ls("https://a.example:4317", 10_000, None);
        let endpoint = otlp_ls("https://b.example:4317", 10_000, None);
        let timeout = otlp_ls("https://a.example:4317", 20_000, None);
        assert!(!fingerprint_otlp(&base)
            .unwrap()
            .equal_runtime(&fingerprint_otlp(&endpoint).unwrap()));
        assert!(!fingerprint_otlp(&base)
            .unwrap()
            .equal_runtime(&fingerprint_otlp(&timeout).unwrap()));
    }

    #[test]
    fn bearer_and_tls_material_rebuild() {
        let base = otlp_ls("https://a.example:4317", 10_000, Some("1"));
        let token_a = with_auth(base.clone(), b"token-a");
        let token_b = with_auth(base.clone(), b"token-b");
        assert!(!fingerprint_otlp(&token_a)
            .unwrap()
            .equal_runtime(&fingerprint_otlp(&token_b).unwrap()));

        let mut ca_a = base.clone();
        let mut ca_b = base;
        for (ls, ca) in [(&mut ca_a, b"ca-a".as_slice()), (&mut ca_b, b"ca-b".as_slice())] {
            let SystemConfig::Otlp(config) = &mut ls.spec.config else {
                panic!("OTLP fixture");
            };
            config.tls = Some(TlsConfig {
                enabled: true,
                outbound: OutboundTlsLocal::default(),
                resolved_ca_certificates: Some(vec![Secret {
                    data: Some(BTreeMap::from([("ca.crt".to_string(), ByteString(ca.to_vec()))])),
                    ..Default::default()
                }]),
                resolved_client_certificate: None,
            });
        }
        assert!(!fingerprint_otlp(&ca_a)
            .unwrap()
            .equal_runtime(&fingerprint_otlp(&ca_b).unwrap()));
    }

    #[test]
    fn resource_and_sync_versions_are_ignored() {
        let mut first = otlp_ls("https://a.example:4317", 10_000, Some("1"));
        let mut second = otlp_ls("https://a.example:4317", 10_000, Some("2"));
        first.metadata.annotations = Some(BTreeMap::from([(
            "edgion.io/sync-version".to_string(),
            "first".to_string(),
        )]));
        second.metadata.annotations = Some(BTreeMap::from([(
            "edgion.io/sync-version".to_string(),
            "second".to_string(),
        )]));
        second.status = Some(
            LinkSysLocalStatus {
                observed_generation: Some(7),
                ..Default::default()
            }
            .into(),
        );
        assert!(fingerprint_otlp(&first)
            .unwrap()
            .equal_runtime(&fingerprint_otlp(&second).unwrap()));
    }
}
