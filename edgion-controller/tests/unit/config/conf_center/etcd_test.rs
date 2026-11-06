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
    let config: EtcdConfig = yaml_serde::from_str(yaml).unwrap();
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
    let config: EtcdConfig = yaml_serde::from_str(yaml).unwrap();
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
        cache_ready_deadline_secs: 180,
    };

    let client_config = config.to_bootstrap_config().unwrap();
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
}

#[test]
fn test_to_client_config_with_auth_wires_yaml_credentials_into_state() {
    // YAML credentials remain direct bootstrap material.
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
        cache_ready_deadline_secs: 180,
    };

    let client_config = config.to_bootstrap_config().unwrap();

    let material = client_config.auth.as_ref().expect("direct YAML credentials");
    assert_eq!(material.username.as_deref(), Some("root"));
    assert_eq!(material.password, "changeme");

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
        cache_ready_deadline_secs: 180,
    };

    let client_config = config.to_bootstrap_config().unwrap();
    assert!(client_config.auth.is_none());
}

/// A disk CA selects direct custom CA material without a client identity.
/// This retains the CA-only regression coverage without synthetic Secrets.
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
        cache_ready_deadline_secs: 180,
    };

    let client_config = config.to_bootstrap_config().unwrap();
    let tls = client_config.tls.expect("TLS config must be present");

    assert_eq!(
        tls.ca_pem.as_deref(),
        Some(b"-----BEGIN CERTIFICATE-----\nMIIBxxx\n-----END CERTIFICATE-----\n".as_slice())
    );
    assert!(tls.client_cert_pem.is_none());
    assert!(tls.client_key_pem.is_none());
}

fn file_tls_config() -> EtcdConfig {
    serde_json::from_value(serde_json::json!({"endpoints":["https://etcd.example:2379"],"tls":{}})).unwrap()
}

#[test]
fn bootstrap_keeps_tls_presence_and_direct_complete_identity_redacted() {
    let mut config = file_tls_config();
    let empty = config.to_bootstrap_config().unwrap();
    let tls = empty.tls.expect("an explicit empty TLS block still selects TLS");
    assert!(tls.ca_pem.is_none() && tls.client_cert_pem.is_none() && tls.client_key_pem.is_none());
    let cert = tempfile::NamedTempFile::new().unwrap();
    let key = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(cert.path(), "disk-certificate").unwrap();
    std::fs::write(key.path(), "disk-private-key").unwrap();
    let tls = config.tls.as_mut().unwrap();
    tls.client_cert_file = Some(cert.path().to_str().unwrap().into());
    tls.client_key_file = Some(key.path().to_str().unwrap().into());
    config.auth = Some(EtcdAuthConfig {
        username: "root".into(),
        password: "bootstrap-password".into(),
    });
    let direct = config.to_bootstrap_config().unwrap();
    let debug = format!("{direct:?}");
    assert!(!debug.contains("disk-private-key") && !debug.contains("bootstrap-password"));
    let tls = direct.tls.unwrap();
    assert_eq!(tls.client_cert_pem.as_deref(), Some(b"disk-certificate".as_slice()));
    assert_eq!(tls.client_key_pem.unwrap().as_slice(), b"disk-private-key");
}

#[test]
fn bootstrap_partial_identity_is_inert_but_every_declared_file_is_read() {
    let cert = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(cert.path(), "disk-certificate").unwrap();
    let mut config = file_tls_config();
    config.tls.as_mut().unwrap().client_cert_file = Some(cert.path().to_str().unwrap().into());
    let tls = config.to_bootstrap_config().unwrap().tls.unwrap();
    assert!(tls.client_cert_pem.is_none() && tls.client_key_pem.is_none());
    let key = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(key.path(), [0xff]).unwrap();
    config.tls.as_mut().unwrap().client_key_file = Some(key.path().to_str().unwrap().into());
    let error = config.to_bootstrap_config().unwrap_err().to_string();
    assert!(
        error.contains("Failed to read TLS file"),
        "invalid UTF-8 remains a file-read failure"
    );
}

#[test]
fn bootstrap_half_keepalive_uses_controller_defaults_only_for_missing_half() {
    let mut config = file_tls_config();
    config.keep_alive_time_secs = Some(17);
    let direct = config.to_bootstrap_config().unwrap();
    let keepalive = direct.keep_alive.unwrap();
    assert_eq!((keepalive.time, keepalive.timeout), (Some(17), Some(3)));
    config.keep_alive_time_secs = None;
    config.keep_alive_timeout_secs = Some(11);
    let direct = config.to_bootstrap_config().unwrap();
    let keepalive = direct.keep_alive.unwrap();
    assert_eq!((keepalive.time, keepalive.timeout), (Some(5), Some(11)));
}

#[test]
fn advertised_leader_addresses_classify_literals_without_broadening_egress_policy() {
    let cases = [
        ("", false),
        ("localhost:9090", false),
        ("127.0.0.0:9090", false),
        ("127.0.0.2:9090", false),
        ("127.255.255.255:9090", false),
        ("0.0.0.0:9090", false),
        ("[::1]:9090", false),
        ("[0:0:0:0:0:0:0:1]:9090", false),
        ("[0:0:0:0:0:0:0:0]:9090", false),
        ("[::ffff:127.0.0.2]:9090", false),
        ("[::ffff:7fff:ffff]:9090", false),
        ("[::ffff:0.0.0.0]:9090", false),
        ("126.255.255.255:9090", true),
        ("128.0.0.0:9090", true),
        ("10.0.0.11:9090", true),
        ("169.254.1.1:9090", true),
        ("192.0.2.1:9090", true),
        ("[fe80::1]:9090", true),
        ("[fd00::1]:9090", true),
        ("[2001:db8::1]:9090", true),
        ("[::ffff:10.0.0.11]:9090", true),
        ("[::ffff:192.0.2.1]:9090", true),
        ("controller.example.test:9090", true),
    ];
    for (address, accepted) in cases {
        for grpc in [true, false] {
            let mut config = EtcdLeaderElectionConfig {
                grpc_addr: "10.0.0.11:9090".into(),
                admin_addr: "10.0.0.11:8080".into(),
                ..Default::default()
            };
            if grpc {
                config.grpc_addr = address.into();
            } else {
                config.admin_addr = address.into();
            }
            let result = config.validate();
            assert_eq!(result.is_ok(), accepted, "grpc={grpc}, address={address}: {result:?}");
            if let Err(error) = result {
                let field = if grpc { "grpc_addr" } else { "admin_addr" };
                assert!(error
                    .to_string()
                    .starts_with(&format!("etcd leader_election: {field} must be")));
            }
            assert_eq!(if grpc { &config.grpc_addr } else { &config.admin_addr }, address);
        }
    }
}

#[test]
fn etcd_cache_ready_deadline_default_and_floor() {
    let mut config = file_tls_config();
    assert_eq!(config.cache_ready_deadline_secs, 180);
    assert!(config.validate().is_ok());
    config.cache_ready_deadline_secs = 30;
    assert!(config.validate().is_err());
    config.cache_ready_deadline_secs = 600;
    assert!(config.validate().is_ok());
}
