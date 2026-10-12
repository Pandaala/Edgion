use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use bytes::Bytes;
use mockall::Sequence;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};

fn actual_plugin(config: &HeaderCertAuthConfig) -> HeaderCertAuth {
    use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"header-auth","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"HeaderCertAuth","config":config}]}
    }))
    .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    HeaderCertAuth::new(&entry.header_cert_auth().unwrap().unwrap())
}

fn sample_cert_info() -> ClientCertInfo {
    ClientCertInfo {
        subject: "CN=alice, O=edgion".to_string(),
        sans: vec!["alice@example.com".to_string()],
        cn: Some("alice".to_string()),
        fingerprint: "aa:bb:cc".to_string(),
    }
}

fn subject_only_cert_info() -> ClientCertInfo {
    let key = KeyPair::generate().expect("test key generation must succeed");
    let mut params = CertificateParams::new(Vec::<String>::new()).expect("empty SAN list must be valid");
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::OrganizationName, "subject-only-org");
    params.distinguished_name = distinguished_name;
    let pem = params
        .self_signed(&key)
        .expect("test certificate signing must succeed")
        .pem();
    let cert = X509::from_pem(pem.as_bytes()).expect("generated certificate must parse");
    let info = extract_client_cert_info_from_cert(&cert);
    assert!(!info.subject.is_empty());
    assert!(info.sans.is_empty());
    assert!(info.cn.is_none());
    info
}

fn connection_plugin(consumer_by: ConsumerBy, allow_anonymous: bool) -> HeaderCertAuth {
    let config = HeaderCertAuthConfig {
        mode: CertSourceMode::Connection,
        consumer_by,
        allow_anonymous,
        auth_failure_delay_ms: 0,
        ..Default::default()
    };
    actual_plugin(&config)
}

#[test]
fn identity_selection_borrows_exact_configured_certificate_field() {
    let mut info = sample_cert_info();
    let plugin = connection_plugin(ConsumerBy::SanOrCn, false);
    let identity = plugin.get_identity(&info).expect("SAN identity must exist");
    assert_eq!(identity, "alice@example.com");
    assert_eq!(identity.as_ptr(), info.sans[0].as_ptr());

    info.sans.clear();
    let identity = plugin.get_identity(&info).expect("CN fallback must exist");
    assert_eq!(identity, "alice");
    assert_eq!(
        identity.as_ptr(),
        info.cn.as_deref().expect("test CN must exist").as_ptr()
    );

    let plugin = connection_plugin(ConsumerBy::Cn, false);
    let identity = plugin.get_identity(&info).expect("CN identity must exist");
    assert_eq!(
        identity.as_ptr(),
        info.cn.as_deref().expect("test CN must exist").as_ptr()
    );

    let plugin = connection_plugin(ConsumerBy::Fingerprint, false);
    let identity = plugin.get_identity(&info).expect("fingerprint identity must exist");
    assert_eq!(identity, "aa:bb:cc");
    assert_eq!(identity.as_ptr(), info.fingerprint.as_ptr());
}

#[test]
fn subject_is_not_an_identity_fallback() {
    let info = subject_only_cert_info();

    assert!(connection_plugin(ConsumerBy::SanOrCn, false)
        .get_identity(&info)
        .is_none());
    assert!(connection_plugin(ConsumerBy::Cn, false).get_identity(&info).is_none());
}

#[tokio::test]
async fn subject_only_identity_strategies_fail_closed_with_exact_response() {
    for consumer_by in [ConsumerBy::SanOrCn, ConsumerBy::Cn] {
        let plugin = connection_plugin(consumer_by, false);
        let mut session = MockPluginSession::new();
        session
            .expect_client_cert_info()
            .times(1)
            .return_const(Some(subject_only_cert_fixture()));

        let mut sequence = Sequence::new();
        session
            .expect_write_response_header()
            .withf(|response, end_of_stream| {
                !end_of_stream
                    && response.status.as_u16() == 401
                    && response
                        .headers
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        == Some("text/plain")
                    && response
                        .headers
                        .get("www-authenticate")
                        .and_then(|value| value.to_str().ok())
                        == Some("TLSCert realm=\"edgion\"")
            })
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_write_response_body()
            .withf(|body, end_of_stream| {
                *end_of_stream
                    && body.as_deref() == Some(Bytes::from_static(b"401 TLS certificate failed verification").as_ref())
            })
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_shutdown()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|| {});

        let mut log = PluginLog::new("HeaderCertAuth");
        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert_eq!(log.render_for_test(), "err: identity-not-found");
        assert!(!log.contains("cert verify:ok"));
    }
}

#[tokio::test]
async fn subject_only_identity_strategies_allow_exact_anonymous_result() {
    for consumer_by in [ConsumerBy::SanOrCn, ConsumerBy::Cn] {
        let plugin = connection_plugin(consumer_by, true);
        let mut session = MockPluginSession::new();
        session
            .expect_client_cert_info()
            .times(1)
            .return_const(Some(subject_only_cert_fixture()));

        let mut sequence = Sequence::new();
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "X-Anonymous-Consumer" && value == "true" && value.is_sensitive())
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "X-Consumer-Username" && value == "anonymous" && value.is_sensitive())
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));

        let mut log = PluginLog::new("HeaderCertAuth");
        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "Allow anonymous");
    }
}

#[tokio::test]
async fn test_connection_mode_success() {
    let config = HeaderCertAuthConfig {
        mode: CertSourceMode::Connection,
        ..Default::default()
    };
    let plugin = actual_plugin(&config);

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("HeaderCertAuth");

    session
        .expect_client_cert_info()
        .times(1)
        .return_const(Some(sample_cert_fixture()));
    session
        .expect_set_request_header_value()
        .times(2)
        .returning(|_, value| {
            assert!(value.is_sensitive());
            Ok(())
        });

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(log.contains("cert verify:ok"));
}

#[tokio::test]
async fn test_connection_mode_anonymous_on_missing_cert() {
    let config = HeaderCertAuthConfig {
        mode: CertSourceMode::Connection,
        allow_anonymous: true,
        ..Default::default()
    };
    let plugin = actual_plugin(&config);

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("HeaderCertAuth");

    session.expect_client_cert_info().times(1).return_const(None);
    session
        .expect_set_request_header_value()
        .times(2)
        .returning(|_, value| {
            assert!(value.is_sensitive());
            Ok(())
        });

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(log.contains("Allow anonymous"));
}

// Generate a CA cert + a leaf cert signed by that CA, both PEM-encoded.
fn make_ca_and_leaf(
    ca_not_before_days: i64,
    ca_not_after_days: i64,
    leaf_not_before_days: i64,
    leaf_not_after_days: i64,
) -> (String, String) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};

    let now = ::time::OffsetDateTime::now_utc();

    let ca_key = KeyPair::generate().expect("CA key gen failed");
    let mut ca_params = CertificateParams::new(vec!["Edgion Test CA".to_string()]).expect("CA params failed");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.not_before = now + ::time::Duration::days(ca_not_before_days);
    ca_params.not_after = now + ::time::Duration::days(ca_not_after_days);
    let ca_cert = ca_params.self_signed(&ca_key).expect("CA self-sign failed");
    let ca_issuer = Issuer::from_params(&ca_params, &ca_key);

    let leaf_key = KeyPair::generate().expect("leaf key gen failed");
    let mut leaf_params = CertificateParams::new(vec!["leaf.example.com".to_string()]).expect("leaf params failed");
    leaf_params.not_before = now + ::time::Duration::days(leaf_not_before_days);
    leaf_params.not_after = now + ::time::Duration::days(leaf_not_after_days);
    let leaf_cert = leaf_params.signed_by(&leaf_key, &ca_issuer).expect("leaf sign failed");

    (ca_cert.pem(), leaf_cert.pem())
}

fn make_ca_secret(ca_pem: &str) -> k8s_openapi::api::core::v1::Secret {
    let mut data = std::collections::BTreeMap::new();
    data.insert(
        "ca.crt".to_string(),
        k8s_openapi::ByteString(ca_pem.as_bytes().to_vec()),
    );
    k8s_openapi::api::core::v1::Secret {
        metadata: kube::core::ObjectMeta {
            name: Some("ca".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    }
}

fn header_mode_plugin_with_ca(mut secrets: Vec<k8s_openapi::api::core::v1::Secret>) -> HeaderCertAuth {
    let refs = secrets
        .iter_mut()
        .enumerate()
        .map(|(index, secret)| {
            let name = format!("ca-{index}");
            secret.metadata.name = Some(name.clone());
            secret.metadata.namespace = Some("default".into());
            serde_json::from_value(serde_json::json!({"name":name})).unwrap()
        })
        .collect();
    let config = HeaderCertAuthConfig {
        mode: CertSourceMode::Header,
        ca_secret_refs: refs,
        resolved_ca_secrets: Some(secrets),
        ..Default::default()
    };
    actual_plugin(&config)
}

#[test]
fn prepared_ca_pool_drops_runtime_secrets_and_borrows_pem() {
    let (ca_pem, _) = make_ca_and_leaf(-1, 30, -1, 30);
    let secret = make_ca_secret(&ca_pem);
    let borrowed = HeaderCertAuth::read_ca_pem_from_secret(&secret).unwrap();
    assert_eq!(borrowed.as_ptr(), secret.data.as_ref().unwrap()["ca.crt"].0.as_ptr());
    let string_secret = k8s_openapi::api::core::v1::Secret {
        string_data: Some(std::collections::BTreeMap::from([("ca.crt".to_string(), ca_pem)])),
        ..Default::default()
    };
    assert_eq!(
        HeaderCertAuth::read_ca_pem_from_secret(&string_secret)
            .unwrap()
            .as_ptr(),
        string_secret.string_data.as_ref().unwrap()["ca.crt"].as_ptr()
    );
    let config = HeaderCertAuthConfig {
        mode: CertSourceMode::Header,
        ca_secret_refs: vec![serde_json::from_value(serde_json::json!({"name":"ca"})).unwrap()],
        resolved_ca_secrets: Some(vec![secret]),
        ..Default::default()
    };
    let plugin = actual_plugin(&config);
    assert_eq!(plugin.ca_pool.len(), 1);
    assert!(plugin.config.resolved_ca_secrets.is_none());
    assert_eq!(config.resolved_ca_secrets.as_ref().unwrap().len(), 1);
}

#[test]
fn test_verify_against_ca_succeeds_with_matching_ca() {
    let (ca_pem, leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
    let plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&ca_pem)]);
    assert!(plugin.verify_against_ca(leaf_pem.as_bytes()).is_ok());
}

#[test]
fn test_verify_against_ca_rejects_issuer_mismatch() {
    // Trusted CA in pool, but leaf is signed by a different (untrusted) CA.
    let (trusted_ca_pem, _) = make_ca_and_leaf(-1, 30, -1, 30);
    let (_, foreign_leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
    let plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&trusted_ca_pem)]);
    assert_eq!(
        plugin.verify_against_ca(foreign_leaf_pem.as_bytes()).unwrap_err(),
        "ca-verification-failed"
    );
}

#[test]
fn test_verify_against_ca_rejects_expired_leaf() {
    let (ca_pem, expired_leaf_pem) = make_ca_and_leaf(-30, 30, -30, -1);
    let plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&ca_pem)]);
    assert_eq!(
        plugin.verify_against_ca(expired_leaf_pem.as_bytes()).unwrap_err(),
        "expired-leaf-cert"
    );
}

#[test]
fn test_verify_against_ca_no_ca_when_pool_empty() {
    let (_, leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
    let config = HeaderCertAuthConfig {
        mode: CertSourceMode::Header,
        ..Default::default()
    };
    let plugin = actual_plugin(&config);
    assert_eq!(
        plugin.verify_against_ca(leaf_pem.as_bytes()).unwrap_err(),
        "no-ca-configured"
    );
}

#[test]
fn test_verify_against_ca_matches_non_first_ca_in_pool() {
    // Pool contains [foreign_ca, trusted_ca]; leaf is signed by trusted_ca.
    // Exercises the loop iterating past a non-matching CA before hitting the match.
    let (foreign_ca_pem, _) = make_ca_and_leaf(-1, 30, -1, 30);
    let (trusted_ca_pem, leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
    let plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&foreign_ca_pem), make_ca_secret(&trusted_ca_pem)]);
    assert!(plugin.verify_against_ca(leaf_pem.as_bytes()).is_ok());
}

#[test]
fn test_load_ca_certs_drops_expired_ca_at_init() {
    // Expired CA should be filtered out at init; pool becomes empty.
    let (expired_ca_pem, leaf_pem) = make_ca_and_leaf(-30, -1, -1, 30);
    let plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&expired_ca_pem)]);
    assert_eq!(
        plugin.verify_against_ca(leaf_pem.as_bytes()).unwrap_err(),
        "no-ca-configured"
    );
}

#[test]
fn actual_ca_identity_filters_before_crypto_and_preserves_partial_success() {
    let (pem, leaf) = make_ca_and_leaf(-1, 30, -1, 30);
    let mut secret = make_ca_secret(&pem);
    secret.metadata.resource_version = Some("7".into());
    let base = HeaderCertAuthConfig {
        mode: CertSourceMode::Header,
        ca_secret_refs: vec![
            serde_json::from_value(serde_json::json!({"name":"missing"})).unwrap(),
            serde_json::from_value(serde_json::json!({"name":"ca"})).unwrap(),
        ],
        resolved_ca_secrets: Some(vec![secret]),
        ..Default::default()
    };
    for case in [
        "matching",
        "invalid-first",
        "partial",
        "name",
        "no-name",
        "namespace",
        "no-namespace",
        "marker",
        "retarget",
        "no-ref",
        "connection",
    ] {
        let mut config = base.clone();
        let material = &mut config.resolved_ca_secrets.as_mut().unwrap()[0];
        match case {
            "name" => material.metadata.name = Some("other".into()),
            "no-name" => material.metadata.name = None,
            "namespace" => material.metadata.namespace = Some("other".into()),
            "no-namespace" => material.metadata.namespace = None,
            "marker" => {
                material.metadata.annotations = Some(std::collections::BTreeMap::from([(
                    edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE.into(),
                    "ConfigMap".into(),
                )]))
            }
            "retarget" => config.ca_secret_refs[1].name = "other".into(),
            "no-ref" => config.ca_secret_refs.clear(),
            "invalid-first" => config.ca_secret_refs[0].name = "bad/name".into(),
            "partial" => {
                let mut bad = material.clone();
                bad.metadata.name = None;
                config.resolved_ca_secrets.as_mut().unwrap().insert(0, bad);
            }
            "connection" => config.mode = CertSourceMode::Connection,
            _ => {}
        }
        let plugin = actual_plugin(&config);
        let good = matches!(case, "matching" | "invalid-first" | "partial");
        assert_eq!(plugin.ca_pool.len(), usize::from(good), "{case}");
        if good {
            assert!(plugin.verify_against_ca(leaf.as_bytes()).is_ok(), "{case}");
        } else {
            assert_eq!(
                plugin.verify_against_ca(leaf.as_bytes()).unwrap_err(),
                "no-ca-configured",
                "{case}"
            );
        }
        assert!(plugin.config.resolved_ca_secrets.is_none());
    }
}

fn subject_only_cert_fixture() -> &'static ClientCertInfo {
    static CERT: std::sync::OnceLock<ClientCertInfo> = std::sync::OnceLock::new();
    CERT.get_or_init(subject_only_cert_info)
}

fn sample_cert_fixture() -> &'static ClientCertInfo {
    static CERT: std::sync::OnceLock<ClientCertInfo> = std::sync::OnceLock::new();
    CERT.get_or_init(sample_cert_info)
}
