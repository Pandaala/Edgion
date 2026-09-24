use super::*;

#[test]
fn leader_election_defaults_are_valid() {
    let c = EtcdLeaderElectionConfig {
        identity: None,
        grpc_addr: "10.0.0.11:9090".into(),
        admin_addr: "10.0.0.11:8080".into(),
        ..Default::default()
    };
    assert_eq!(c.lease_duration_secs, 15);
    assert_eq!(c.renew_deadline_secs, 10);
    assert_eq!(c.retry_period_secs, 2);
    c.validate().expect("defaults must validate");
}

#[test]
fn leader_election_rejects_bad_timing() {
    // renew_deadline >= lease_duration
    let c = EtcdLeaderElectionConfig {
        grpc_addr: "10.0.0.11:9090".into(),
        admin_addr: "10.0.0.11:8080".into(),
        lease_duration_secs: 10,
        renew_deadline_secs: 10,
        retry_period_secs: 2,
        identity: None,
    };
    assert!(c.validate().is_err());
    // fewer than 3 keepalive attempts: floor(4/2)=2
    let c = EtcdLeaderElectionConfig {
        grpc_addr: "10.0.0.11:9090".into(),
        admin_addr: "10.0.0.11:8080".into(),
        lease_duration_secs: 6,
        renew_deadline_secs: 4,
        retry_period_secs: 2,
        identity: None,
    };
    assert!(c.validate().is_err());
}

#[test]
fn leader_election_rejects_loopback_and_missing_addr() {
    let c = EtcdLeaderElectionConfig {
        grpc_addr: "127.0.0.1:9090".into(),
        admin_addr: "10.0.0.11:8080".into(),
        identity: None,
        ..Default::default()
    };
    assert!(c.validate().is_err(), "loopback grpc_addr must be rejected");
    let c = EtcdLeaderElectionConfig {
        grpc_addr: "".into(),
        admin_addr: "10.0.0.11:8080".into(),
        identity: None,
        ..Default::default()
    };
    assert!(c.validate().is_err(), "empty grpc_addr must be rejected");
}

#[test]
fn test_deserialize_minimal() {
    let yaml = r#"
endpoints: ["http://127.0.0.1:2379"]
"#;
    let config: EtcdConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.endpoints, vec!["http://127.0.0.1:2379"]);
    assert_eq!(config.key_prefix, "/edgion");
    assert!(config.auth.is_none());
    assert!(config.tls.is_none());
}

#[test]
fn test_deserialize_full() {
    let yaml = r#"
endpoints: ["http://etcd-1:2379", "http://etcd-2:2379"]
key_prefix: /my-app
connect_timeout_ms: 3000
request_timeout_ms: 8000
keep_alive_time_secs: 30
keep_alive_timeout_secs: 10
auth:
  username: root
  password: changeme
"#;
    let config: EtcdConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.endpoints.len(), 2);
    assert_eq!(config.key_prefix, "/my-app");
    assert_eq!(config.connect_timeout_ms, Some(3000));

    let auth = config.auth.as_ref().unwrap();
    assert_eq!(auth.username, "root");
    assert_eq!(auth.password, "changeme");
}

#[test]
fn test_default_key_prefix() {
    assert_eq!(default_key_prefix(), "/edgion");
}

#[test]
fn test_to_client_config_minimal() {
    let config = EtcdConfig {
        controller_name: Some("edgion.io/gateway-controller".into()),
        endpoints: vec!["http://127.0.0.1:2379".to_string()],
        key_prefix: "/edgion".to_string(),
        auth: None,
        tls: None,
        connect_timeout_ms: None,
        request_timeout_ms: None,
        keep_alive_time_secs: None,
        keep_alive_timeout_secs: None,
        leader_election: EtcdLeaderElectionConfig::default(),
    };

    let (client_config, auth_state) = config.to_client_config().unwrap();
    assert_eq!(client_config.endpoints, vec!["http://127.0.0.1:2379"]);
    assert!(client_config.auth.is_none());
    assert!(client_config.tls.is_none());
    // Conf_center-scoped defaults are injected at THIS layer (never in the
    // shared builder — spec §10.5): a minimal config still gets a bounded
    // dial timeout and always-on h2 keepalive (spec §4.2 fact 2), with no
    // channel-wide request timeout (it would kill the blocking campaign()).
    let timeout = client_config.timeout.as_ref().expect("dial default expected");
    assert_eq!(timeout.dial, Some(5_000));
    assert!(timeout.request.is_none());
    let ka = client_config.keep_alive.as_ref().expect("keepalive default expected");
    assert_eq!((ka.time, ka.timeout), (Some(5), Some(3)));
    assert_eq!(ka.permit_without_stream, Some(true));
    assert!(matches!(auth_state, CredentialState::NotRequired));
}

#[test]
fn test_to_client_config_with_auth_wires_yaml_credentials_into_state() {
    // YAML `conf_center.auth` must flow through to `CredentialState::Resolved`.
    let config = EtcdConfig {
        controller_name: Some("edgion.io/gateway-controller".into()),
        endpoints: vec!["http://127.0.0.1:2379".to_string()],
        key_prefix: "/edgion".to_string(),
        auth: Some(EtcdAuthConfig {
            username: "root".to_string(),
            password: "changeme".to_string(),
        }),
        tls: None,
        connect_timeout_ms: Some(5000),
        request_timeout_ms: Some(10000),
        keep_alive_time_secs: None,
        keep_alive_timeout_secs: None,
        leader_election: EtcdLeaderElectionConfig::default(),
    };

    let (client_config, auth_state) = config.to_client_config().unwrap();

    assert!(
        client_config.auth.is_some(),
        "placeholder EtcdAuth must be present so the provider's `crd.auth.is_some()` gate opens"
    );
    match auth_state {
        CredentialState::Resolved(mat) => {
            assert_eq!(mat.username.as_deref(), Some("root"));
            assert_eq!(mat.password, "changeme");
        }
        other => panic!("expected Resolved, got {:?}", other),
    }

    let timeout = client_config.timeout.unwrap();
    assert_eq!(timeout.dial, Some(5000));
    assert_eq!(timeout.request, Some(10000));
}

#[test]
fn test_to_client_config_without_auth_is_not_required() {
    let config = EtcdConfig {
        controller_name: Some("edgion.io/gateway-controller".into()),
        endpoints: vec!["http://127.0.0.1:2379".to_string()],
        key_prefix: "/edgion".to_string(),
        auth: None,
        tls: None,
        connect_timeout_ms: None,
        request_timeout_ms: None,
        keep_alive_time_secs: None,
        keep_alive_timeout_secs: None,
        leader_election: EtcdLeaderElectionConfig::default(),
    };

    let (client_config, auth_state) = config.to_client_config().unwrap();
    assert!(client_config.auth.is_none());
    assert!(matches!(auth_state, CredentialState::NotRequired));
}

/// A CA-only TLS config (only ca_cert_file set, no client cert/key) must:
/// - produce a TlsConfig with validation.caCertificateRefs set so the resolver
///   reads the CA from resolved_ca_certificates (not system roots).
/// - have resolved_ca_certificates set (the CA material).
/// - have resolved_client_certificate = None (no mTLS identity).
///
/// Regression guard for the bug where the synthetic Secret was assigned to
/// BOTH resolved_ca_certificates and resolved_client_certificate, causing the
/// resolver to fail with OutboundTlsClientCertInvalid when no tls.crt/tls.key
/// were present.
#[test]
fn test_ca_only_tls_config_does_not_set_client_secret() {
    use std::io::Write;
    use tempfile::NamedTempFile;

    // Write a dummy CA PEM to a temp file.
    let mut ca_file = NamedTempFile::new().expect("failed to create temp file");
    write!(
        ca_file,
        "-----BEGIN CERTIFICATE-----\nMIIBxxx\n-----END CERTIFICATE-----\n"
    )
    .expect("failed to write temp CA");
    let ca_path = ca_file.path().to_str().unwrap().to_string();

    let config = EtcdConfig {
        controller_name: Some("edgion.io/gateway-controller".into()),
        endpoints: vec!["https://etcd:2379".to_string()],
        key_prefix: "/edgion".to_string(),
        auth: None,
        tls: Some(EtcdTlsConfig {
            ca_cert_file: Some(ca_path),
            client_cert_file: None,
            client_key_file: None,
        }),
        connect_timeout_ms: None,
        request_timeout_ms: None,
        keep_alive_time_secs: None,
        keep_alive_timeout_secs: None,
        leader_election: EtcdLeaderElectionConfig::default(),
    };

    let (client_config, _) = config.to_client_config().unwrap();
    let tls = client_config.tls.expect("TLS config must be present");

    // When a CA file is configured, the outbound validation block must contain a
    // non-empty ca_certificate_refs so that resolve_outbound_tls picks Custom CA.
    assert!(
        tls.outbound
            .validation
            .as_ref()
            .and_then(|v| v.ca_certificate_refs.as_ref())
            .is_some_and(|r| !r.is_empty()),
        "outbound.validation.ca_certificate_refs must be set when ca_cert_file is provided"
    );
    assert!(
        tls.resolved_ca_certificates.is_some(),
        "resolved_ca_certificates must carry the CA material"
    );
    assert!(
        tls.resolved_client_certificate.is_none(),
        "resolved_client_certificate must be None when no client cert/key is configured (CA-only)"
    );
}
