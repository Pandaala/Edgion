use super::*;
use crate::resources::edgion_tls::GatewayAttachmentProof;

#[test]
fn test_http01_yaml_deserialization() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: my-acme
  namespace: default
spec:
  email: admin@example.com
  privateKeySecretRef:
    name: my-acme-account
  domains:
    - example.com
    - www.example.com
  challenge:
    type: http-01
    gatewayRef:
      name: my-gateway
      namespace: default
  storage:
    secretName: acme-cert-example
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: my-gateway
        namespace: default
"#;

    let acme: Result<EdgionAcme, _> = serde_yaml::from_str(yaml);
    assert!(acme.is_ok(), "Failed to deserialize YAML: {:?}", acme.err());

    let acme = acme.unwrap();
    assert_eq!(acme.metadata.name, Some("my-acme".to_string()));
    assert_eq!(acme.spec.email, "admin@example.com");
    assert_eq!(acme.spec.private_key_secret_ref.name, "my-acme-account");
    assert_eq!(acme.spec.domains.len(), 2);
    assert!(!acme.has_wildcard_domains());
    assert_eq!(acme.spec.server, "https://acme-v02.api.letsencrypt.org/directory");
    assert_eq!(acme.spec.renewal.renew_before, "720h");
}

#[test]
fn challenge_rejects_unsupported_types_and_dns_fields() {
    for yaml in [
        "type: dns-01\nprovider: cloudflare\ncredentialRef:\n  name: dns-token",
        "type: tls-alpn-01\ngatewayRef:\n  name: gw",
        "type: http-01\ngatewayRef:\n  name: gw\nprovider: cloudflare",
        "type: http-01\ngatewayRef:\n  name: gw\ncredentialRef:\n  name: dns-token",
        "type: http-01\ngatewayRef:\n  name: gw\npropagationTimeout: 120s",
    ] {
        assert!(serde_yaml::from_str::<AcmeChallenge>(yaml).is_err(), "accepted: {yaml}");
    }
}

#[test]
fn test_active_challenges() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: test-acme
  namespace: default
spec:
  email: admin@example.com
  privateKeySecretRef:
    name: test-acme-account
  domains:
    - example.com
  challenge:
    type: http-01
    gatewayRef:
      name: my-gateway
  storage:
    secretName: test-cert
status:
  activeChallenges:
    - domain: example.com
      token: "abc123token"
      keyAuthorization: "abc123token.thumbprint"
      expireAt: 1700000000
"#;

    let mut acme: EdgionAcme = serde_yaml::from_str(yaml).unwrap();
    assert!(
        acme.active_challenges().is_empty(),
        "full status is not a serving input"
    );
    acme.spec.current_status = acme.status.clone();
    let challenges = acme.active_challenges();
    assert_eq!(challenges.len(), 1);
    assert_eq!(challenges[0].domain, "example.com");
    assert_eq!(challenges[0].token, "abc123token");
    assert_eq!(challenges[0].key_authorization, "abc123token.thumbprint");
}

#[test]
fn test_default_values() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: minimal
  namespace: default
spec:
  email: a@b.com
  privateKeySecretRef:
    name: minimal-account
  domains:
    - a.com
  challenge:
    type: http-01
    gatewayRef:
      name: gw
  storage:
    secretName: cert
"#;

    let acme: EdgionAcme = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(acme.spec.server, "https://acme-v02.api.letsencrypt.org/directory");
    assert_eq!(acme.spec.key_type, AcmeKeyType::EcdsaP256);
    assert_eq!(acme.spec.renewal.renew_before, "720h");
    assert_eq!(acme.spec.renewal.check_interval, "24h");
    assert_eq!(acme.spec.renewal.fail_backoff, "5m");
    assert!(acme.spec.auto_edgion_tls.enabled);
    assert!(acme.status.is_none());
    assert!(acme.spec.external_account_binding.is_none());
}

#[test]
fn test_get_edgion_tls_name() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: my-cert
  namespace: default
spec:
  email: a@b.com
  privateKeySecretRef:
    name: my-cert-account
  domains: [a.com]
  challenge:
    type: http-01
    gatewayRef:
      name: gw
  storage:
    secretName: cert
"#;

    let acme: EdgionAcme = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(acme.get_edgion_tls_name(), "acme-my-cert");
    assert_eq!(acme.get_secret_namespace(), "default");
}

#[test]
fn test_private_key_secret_ref_required() {
    // Missing privateKeySecretRef should be rejected
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: test
  namespace: default
spec:
  email: admin@example.com
  domains: [example.com]
  challenge:
    type: http-01
    gatewayRef:
      name: gw
  storage:
    secretName: cert
"#;
    let result: Result<EdgionAcme, _> = serde_yaml::from_str(yaml);
    assert!(result.is_err(), "privateKeySecretRef should be required");
}

#[test]
fn eab_deserializes_key_secret_ref() {
    let yaml = "keyId: kid-123\nkeySecretRef:\n  name: eab-secret";
    let eab: AcmeExternalAccountBinding = serde_yaml::from_str(yaml).expect("should deserialize");
    assert_eq!(eab.key_id, "kid-123");
    assert_eq!(eab.key_secret_ref.name, "eab-secret");
}

#[test]
fn eab_rejects_inline_hmac_key() {
    let yaml = "keyId: kid\nkeySecretRef:\n  name: s\nhmacKey: leaked";
    assert!(serde_yaml::from_str::<AcmeExternalAccountBinding>(yaml).is_err());
}

#[test]
fn test_private_key_secret_ref_with_namespace() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: test
  namespace: default
spec:
  email: admin@example.com
  privateKeySecretRef:
    name: my-acme-account
    namespace: secrets-ns
  domains: [example.com]
  challenge:
    type: http-01
    gatewayRef:
      name: gw
  storage:
    secretName: cert
"#;
    let acme: EdgionAcme = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(acme.spec.private_key_secret_ref.name, "my-acme-account");
    assert_eq!(
        acme.spec.private_key_secret_ref.namespace.as_deref(),
        Some("secrets-ns")
    );
}

#[test]
fn resolved_listener_attachments_survive_transport_round_trip() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: test
  namespace: default
spec:
  email: admin@example.com
  privateKeySecretRef:
    name: account
  domains: [example.com]
  challenge:
    type: http-01
    gatewayRef:
      name: gw
  storage:
    secretName: cert
"#;
    let mut acme: EdgionAcme = serde_yaml::from_str(yaml).expect("test resource must deserialize");
    acme.spec.notify_after_publish = true;
    acme.spec.resolved_listener_attachments = Some(vec![ResolvedListenerAttachment {
        gateway_namespace: "default".to_string(),
        gateway_name: "gw".to_string(),
        listener_name: "http".to_string(),
        listener_key: crate::ListenerKey::new("0.0.0.0:80".parse().expect("test listener address must parse"), "HTTP"),
        gateway_proof: GatewayAttachmentProof {
            instance: "instance".to_string(),
            generation: 1,
            epoch: "1".to_string(),
            attachment_digest: "a".repeat(64),
        },
    }]);
    let encoded = serde_json::to_vec(&acme).expect("transport resource must serialize");
    let decoded: EdgionAcme = serde_json::from_slice(&encoded).expect("transport resource must deserialize");
    assert!(!decoded.spec.notify_after_publish);
    let mut wire = serde_json::to_value(&acme).unwrap();
    assert!(wire["spec"].get("notifyAfterPublish").is_none());
    wire["spec"]["notifyAfterPublish"] = serde_json::json!(true);
    let injected: EdgionAcme = serde_json::from_value(wire).unwrap();
    assert!(
        !injected.spec.notify_after_publish,
        "input must not inject notification intent"
    );
    let schema = serde_json::to_value(schemars::schema_for!(EdgionAcmeSpec)).unwrap();
    assert!(schema["properties"].get("notifyAfterPublish").is_none());
    assert_eq!(
        decoded.spec.resolved_listener_attachments,
        acme.spec.resolved_listener_attachments
    );
}
