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
