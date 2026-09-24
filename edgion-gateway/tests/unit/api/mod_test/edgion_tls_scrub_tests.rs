use super::*;
use edgion_resources::resources::common::SecretObjectReference;
use edgion_resources::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode, EdgionTls, EdgionTlsSpec};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::api::ObjectMeta;
use std::collections::BTreeMap;

/// Build a `Secret` carrying a fake `tls.key` private key, mirroring what the
/// Controller resolves into the gateway cache.
fn tls_secret(key_bytes: &[u8]) -> Secret {
    let mut data = BTreeMap::new();
    data.insert("tls.crt".to_string(), ByteString(b"cert".to_vec()));
    data.insert("tls.key".to_string(), ByteString(key_bytes.to_vec()));
    Secret {
        data: Some(data),
        ..Default::default()
    }
}

fn edgion_tls_with_secrets() -> EdgionTls {
    EdgionTls {
        metadata: ObjectMeta {
            name: Some("tls-cert".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: EdgionTlsSpec {
            current_status: None,
            parent_refs: None,
            hosts: vec!["example.com".to_string()],
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "tls-cert".to_string(),
                namespace: None,
            },
            client_auth: Some(ClientAuthConfig {
                mode: ClientAuthMode::Mutual,
                ca_secret_ref: None,
                verify_depth: 1,
                ca_secret: Some(tls_secret(b"CA-PRIVATE-KEY")),
                allowed_sans: None,
                allowed_cns: None,
            }),
            min_tls_version: None,
            ciphers: None,
            secret: Some(tls_secret(b"LEAF-PRIVATE-KEY")),
            resolved_listeners: None,
            resolved_listener_attachments: None,
            resolved_listener_attachment_index: Default::default(),
            resolved_log_labels: None,
        },
        status: None,
    }
}

/// The admin API must redact resolved private-key material, whether listed or
/// fetched by name. Both `spec.secret` (leaf key) and `spec.client_auth.ca_secret`
/// (mTLS CA key) must serialize as `"[redacted]"` through the guarded path.
#[test]
fn admin_serialization_redacts_private_keys() {
    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
    let _enter = rt.enter();
    let client = ConfigClient::new("cid".to_string(), "cname".to_string());
    client.edgion_tls().reset(vec![edgion_tls_with_secrets()], 1);

    // List path: guard lives inside list_client_resources.
    let list = list_client_resources(&client, ResourceKind::EdgionTls);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["spec"]["secret"], "[redacted]", "leaf key not redacted");
    assert_eq!(
        list[0]["spec"]["clientAuth"]["caSecret"], "[redacted]",
        "CA key not redacted"
    );
    let serialized = serde_json::to_string(&list[0]).unwrap();
    assert!(!serialized.contains("tls.key"), "tls.key leaked: {serialized}");

    // Get-by-name path reuses list_client_resources, so the same guard applies.
    let got = get_client_resource(&client, ResourceKind::EdgionTls, Some("default"), "tls-cert")
        .expect("resource should be found");
    assert_eq!(got["spec"]["secret"], "[redacted]", "leaf key not redacted via get");
    assert_eq!(
        got["spec"]["clientAuth"]["caSecret"], "[redacted]",
        "CA key not redacted via get"
    );

    // Non-secret fields must survive so the endpoint stays useful.
    assert_eq!(got["spec"]["hosts"][0], "example.com");
}
