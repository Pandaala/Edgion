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

fn expect_default_identity_cleanup(session: &mut MockPluginSession) {
    for name in ["X-Consumer-Username", "X-Client-Cert-Fingerprint"] {
        session
            .expect_remove_request_header()
            .withf(move |actual| actual == name)
            .times(1)
            .returning(|_| Ok(()));
    }
}

fn assert_header_values(session: &pingora_proxy::Session, name: &str, expected: &[&str]) {
    let actual: Vec<_> = session
        .req_header()
        .headers
        .get_all(name)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    assert_eq!(actual, expected, "unexpected values for {name}");
}

#[tokio::test]
async fn certificate_projection_replaces_duplicates_and_clears_missing_or_unsafe_fields() {
    use crate::plugins::runtime::PingoraSessionAdapter;
    use crate::EdgionHttpContext;

    for state in ["present", "no-san", "unsafe-san", "unsafe-dn", "anonymous"] {
        let mut config = HeaderCertAuthConfig {
            mode: CertSourceMode::Connection,
            skip_consumer_lookup: true,
            allow_anonymous: true,
            ..Default::default()
        };
        config.upstream_headers.san_header = "X-Authenticated-San".into();
        let plugin = actual_plugin(&config);
        let mut info = sample_cert_info();
        match state {
            "no-san" => info.sans.clear(),
            "unsafe-san" => info.sans = vec!["bad\r\nvalue".into()],
            "unsafe-dn" => info.subject = "bad\nvalue".into(),
            _ => {}
        }
        let mut session = crate::plugins::runtime::session_adapter::record_outcome_tests::test_session().await;
        for name in ["X-Authenticated-San", "X-Client-Cert-Dn", "X-Client-Cert-Fingerprint"] {
            session.req_header_mut().append_header(name, "forged").unwrap();
            session
                .req_header_mut()
                .append_header(name.to_ascii_lowercase(), "duplicate")
                .unwrap();
        }
        session.req_header_mut().insert_header("x-unrelated", "keep").unwrap();
        session
            .req_header_mut()
            .insert_header("x-consumer-username", "unowned")
            .unwrap();
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info
            .install_tls_info(None, None, (state != "anonymous").then_some(info), None, None);
        let result = plugin
            .run_request(
                &mut PingoraSessionAdapter::new(&mut session, &mut ctx),
                &mut PluginLog::new("HeaderCertAuth"),
            )
            .await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_header_values(
            &session,
            "x-authenticated-san",
            if matches!(state, "present" | "unsafe-dn") {
                &["alice@example.com"]
            } else {
                &[]
            },
        );
        assert_header_values(
            &session,
            "x-client-cert-dn",
            if matches!(state, "unsafe-dn" | "anonymous") {
                &[]
            } else {
                &["CN=alice, O=edgion"]
            },
        );
        assert_header_values(
            &session,
            "x-client-cert-fingerprint",
            if state == "anonymous" { &[] } else { &["aa:bb:cc"] },
        );
        assert_header_values(&session, "x-unrelated", &["keep"]);
        assert_header_values(
            &session,
            "x-consumer-username",
            if state == "anonymous" {
                &["anonymous"]
            } else {
                &["unowned"]
            },
        );
        for name in ["x-authenticated-san", "x-client-cert-dn", "x-client-cert-fingerprint"] {
            assert!(session
                .req_header()
                .headers
                .get_all(name)
                .iter()
                .all(http::HeaderValue::is_sensitive));
        }
    }
}

#[tokio::test]
async fn consumer_mapping_does_not_reserve_inactive_dn_or_san_names() {
    use crate::plugins::runtime::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    for info in [Some(sample_cert_info()), Some(subject_only_cert_info()), None] {
        let expected = if info.as_ref().is_some_and(|cert| !cert.sans.is_empty()) {
            "alice@example.com"
        } else {
            "anonymous"
        };
        let plugin = connection_plugin(ConsumerBy::SanOrCn, true);
        let mut session = crate::plugins::runtime::session_adapter::record_outcome_tests::test_session().await;
        for name in [
            "x-consumer-username",
            "x-client-cert-fingerprint",
            "x-client-cert-san",
            "x-client-cert-dn",
        ] {
            session.req_header_mut().append_header(name, "forged").unwrap();
            session.req_header_mut().append_header(name, "duplicate").unwrap();
        }
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.install_tls_info(None, None, info, None, None);
        assert_eq!(
            plugin
                .run_request(
                    &mut PingoraSessionAdapter::new(&mut session, &mut ctx),
                    &mut PluginLog::new("HeaderCertAuth")
                )
                .await,
            PluginRunningResult::GoodNext
        );
        assert_header_values(&session, "x-consumer-username", &[expected]);
        assert_header_values(
            &session,
            "x-client-cert-fingerprint",
            if expected == "anonymous" { &[] } else { &["aa:bb:cc"] },
        );
        for name in ["x-client-cert-san", "x-client-cert-dn"] {
            assert_header_values(&session, name, &["forged", "duplicate"]);
        }
    }
}

#[tokio::test]
async fn header_source_is_consumed_before_cleanup_and_output_aliases_keep_write_order() {
    use crate::plugins::runtime::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    let (ca_pem, leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
    for hide in [false, true] {
        let mut plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&ca_pem)]);
        plugin.config.skip_consumer_lookup = true;
        plugin.config.hide_credentials = hide;
        plugin.config.upstream_headers.san_header = plugin.config.certificate_header_name.clone();
        // DN and fingerprint intentionally alias, with fingerprint retaining the final write.
        plugin.config.upstream_headers.dn_header = "X-Shared-Identity".into();
        plugin.config.upstream_headers.fingerprint_header = "x-shared-identity".into();
        let expected = extract_client_cert_info_from_cert(&X509::from_pem(leaf_pem.as_bytes()).unwrap());
        let mut session = crate::plugins::runtime::session_adapter::record_outcome_tests::test_session().await;
        session
            .req_header_mut()
            .insert_header("X-Client-Cert", base64_leaf(&leaf_pem))
            .unwrap();
        session
            .req_header_mut()
            .append_header("X-Shared-Identity", "forged")
            .unwrap();
        let mut ctx = EdgionHttpContext::default();
        assert_eq!(
            plugin
                .run_request(
                    &mut PingoraSessionAdapter::new(&mut session, &mut ctx),
                    &mut PluginLog::new("HeaderCertAuth")
                )
                .await,
            PluginRunningResult::GoodNext
        );
        assert_header_values(
            &session,
            "x-client-cert",
            if hide { &[] } else { &["leaf.example.com"] },
        );
        assert_header_values(&session, "x-shared-identity", &[&expected.fingerprint]);
    }
}

#[tokio::test]
async fn identity_header_mutation_failures_never_downgrade_to_anonymous() {
    for anonymous in [false, true] {
        for failure_kind in ["remove", "write"] {
            for fail_at in 0..3 {
                let plugin = actual_plugin(&HeaderCertAuthConfig {
                    mode: CertSourceMode::Connection,
                    skip_consumer_lookup: true,
                    allow_anonymous: anonymous,
                    ..Default::default()
                });
                let mut session = MockPluginSession::new();
                session
                    .expect_client_cert_info()
                    .times(1)
                    .return_const(Some(sample_cert_fixture()));
                let mut sequence = Sequence::new();
                let names = ["X-Client-Cert-Dn", "X-Client-Cert-San", "X-Client-Cert-Fingerprint"];
                for (index, name) in names.into_iter().enumerate() {
                    if failure_kind == "remove" && index > fail_at {
                        break;
                    }
                    session
                        .expect_remove_request_header()
                        .withf(move |actual| actual == name)
                        .times(1)
                        .in_sequence(&mut sequence)
                        .returning(move |_| {
                            if failure_kind == "remove" && index == fail_at {
                                Err("injected secret detail".into())
                            } else {
                                Ok(())
                            }
                        });
                }
                if failure_kind == "write" {
                    for (index, name) in names.into_iter().enumerate().take(fail_at + 1) {
                        session
                            .expect_set_request_header_value()
                            .withf(move |actual, value| actual == name && value.is_sensitive())
                            .times(1)
                            .in_sequence(&mut sequence)
                            .returning(move |_, _| {
                                if index == fail_at {
                                    Err("injected secret detail".into())
                                } else {
                                    Ok(())
                                }
                            });
                    }
                }
                let mut log = PluginLog::new("HeaderCertAuth");
                assert_eq!(
                    plugin.run_request(&mut session, &mut log).await,
                    PluginRunningResult::ErrResponse {
                        status: 500,
                        body: None
                    }
                );
                assert_eq!(log.render_for_test(), "err: certificate header mutation failed");
            }
        }
    }
}

#[tokio::test]
async fn anonymous_identity_writes_fail_closed_after_certificate_outputs_are_cleared() {
    for has_cert in [false, true] {
        for failed_name in ["X-Anonymous-Consumer", "X-Consumer-Username"] {
            let plugin = connection_plugin(ConsumerBy::SanOrCn, true);
            let mut session = MockPluginSession::new();
            session
                .expect_client_cert_info()
                .times(1)
                .return_const(has_cert.then_some(subject_only_cert_fixture()));
            expect_default_identity_cleanup(&mut session);
            let mut sequence = Sequence::new();
            for name in ["X-Anonymous-Consumer", "X-Consumer-Username"] {
                session
                    .expect_set_request_header_value()
                    .withf(move |actual, _| actual == name)
                    .times(1)
                    .in_sequence(&mut sequence)
                    .returning(move |_, _| {
                        if name == failed_name {
                            Err("injected failure".into())
                        } else {
                            Ok(())
                        }
                    });
                if name == failed_name {
                    break;
                }
            }
            let mut log = PluginLog::new("HeaderCertAuth");
            assert_eq!(
                plugin.run_request(&mut session, &mut log).await,
                PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None
                }
            );
            assert!(!log.contains("Allow anonymous"));
        }
    }
}

#[tokio::test]
async fn header_mode_cleanup_and_credential_hiding_failures_cannot_forward() {
    let (ca_pem, leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
    for has_cert in [false, true] {
        for fail_cleanup in [false, true] {
            let mut plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&ca_pem)]);
            plugin.config.allow_anonymous = true;
            let mut session = MockPluginSession::new();
            let mut headers = http::HeaderMap::new();
            if has_cert {
                headers.insert(
                    "x-client-cert",
                    http::HeaderValue::from_str(&base64_leaf(&leaf_pem)).unwrap(),
                );
            }
            session.expect_request_header_map().times(1).return_const(headers);
            if fail_cleanup {
                session
                    .expect_remove_request_header()
                    .withf(|name| name == "X-Consumer-Username")
                    .times(1)
                    .returning(|_| Err("injected cleanup failure".into()));
            } else {
                expect_default_identity_cleanup(&mut session);
                session
                    .expect_set_request_header_value()
                    .times(2)
                    .returning(|_, _| Ok(()));
                session
                    .expect_remove_request_header()
                    .withf(|name| name == "X-Client-Cert")
                    .times(1)
                    .returning(|_| Err("injected credential hiding failure".into()));
            }
            let mut log = PluginLog::new("HeaderCertAuth");
            assert_eq!(
                plugin.run_request(&mut session, &mut log).await,
                PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None
                }
            );
            assert_eq!(log.render_for_test(), "err: certificate header mutation failed");
        }
    }
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
        expect_default_identity_cleanup(&mut session);
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
        expect_default_identity_cleanup(&mut session);
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

    expect_default_identity_cleanup(&mut session);

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

    expect_default_identity_cleanup(&mut session);

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
    make_named_ca_and_leaf(
        "Edgion Test CA",
        "leaf",
        ca_not_before_days,
        ca_not_after_days,
        leaf_not_before_days,
        leaf_not_after_days,
    )
}

fn make_named_ca_and_leaf(
    ca_name: &str,
    leaf_name: &str,
    ca_not_before_days: i64,
    ca_not_after_days: i64,
    leaf_not_before_days: i64,
    leaf_not_after_days: i64,
) -> (String, String) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};

    let now = ::time::OffsetDateTime::now_utc();

    let ca_key = KeyPair::generate().expect("CA key gen failed");
    let mut ca_params = CertificateParams::new(vec!["Edgion Test CA".to_string()]).expect("CA params failed");
    ca_params.distinguished_name.push(DnType::CommonName, ca_name);
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.not_before = now + ::time::Duration::days(ca_not_before_days);
    ca_params.not_after = now + ::time::Duration::days(ca_not_after_days);
    let ca_cert = ca_params.self_signed(&ca_key).expect("CA self-sign failed");
    let ca_issuer = Issuer::from_params(&ca_params, &ca_key);

    let leaf_key = KeyPair::generate().expect("leaf key gen failed");
    let mut leaf_params = CertificateParams::new(vec![
        "leaf.example.com".to_string(),
        "alternate.example.com".to_string(),
    ])
    .expect("leaf params failed");
    leaf_params.distinguished_name.push(DnType::CommonName, leaf_name);
    leaf_params
        .distinguished_name
        .push(DnType::OrganizationName, "Edgion Leaf Test");
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

fn extract_header_value(plugin: &HeaderCertAuth, value: &str) -> Result<ClientCertInfo, &'static str> {
    let mut session = MockPluginSession::new();
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::HeaderName::from_bytes(plugin.config.certificate_header_name.as_bytes()).unwrap(),
        http::HeaderValue::from_str(value).unwrap(),
    );
    session.expect_request_header_map().times(1).return_const(headers);
    plugin.extract_from_header(&session)
}

fn assert_same_identity(actual: &ClientCertInfo, expected: &ClientCertInfo) {
    assert_eq!(actual.subject, expected.subject);
    assert_eq!(actual.sans, expected.sans);
    assert_eq!(actual.cn, expected.cn);
    assert_eq!(actual.fingerprint, expected.fingerprint);
}

fn base64_leaf(pem: &str) -> String {
    STANDARD.encode(HeaderCertAuth::parse_first_cert_der(pem.as_bytes()).unwrap())
}

fn url_pem(pem: &str) -> String {
    percent_encoding::utf8_percent_encode(pem, percent_encoding::NON_ALPHANUMERIC).to_string()
}

#[test]
fn base64_header_extraction_preserves_canonical_identity_and_ascii_whitespace() {
    let (ca_pem, leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
    let mut plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&ca_pem)]);
    plugin.config.certificate_header_format = CertHeaderFormat::Base64Encoded;
    let expected = extract_client_cert_info_from_cert(&X509::from_pem(leaf_pem.as_bytes()).unwrap());
    assert_eq!(expected.cn.as_deref(), Some("leaf"));
    assert_eq!(expected.sans, ["leaf.example.com", "alternate.example.com"]);
    assert!(expected.subject.contains("O=Edgion Leaf Test"));
    assert!(!expected.fingerprint.is_empty());
    let encoded = base64_leaf(&leaf_pem);
    let spaced = format!(" \t{} \t{}\t ", &encoded[..64], &encoded[64..]);
    for value in [encoded, spaced] {
        assert_same_identity(&extract_header_value(&plugin, &value).unwrap(), &expected);
    }
}

#[test]
fn base64_header_extraction_preserves_error_precedence() {
    let (ca_pem, _) = make_ca_and_leaf(-1, 30, -1, 30);
    let mut trusted = header_mode_plugin_with_ca(vec![make_ca_secret(&ca_pem)]);
    let mut empty = header_mode_plugin_with_ca(Vec::new());
    for plugin in [&mut trusted, &mut empty] {
        plugin.config.certificate_header_format = CertHeaderFormat::Base64Encoded;
    }
    for (value, expected) in [
        ("!invalid!".to_string(), "invalid-base64-cert"),
        ("Zh==".to_string(), "invalid-base64-cert"),
        (" \t".to_string(), "invalid-cert-pem"),
        (STANDARD.encode(b"malformed DER"), "invalid-leaf-cert"),
    ] {
        assert_eq!(extract_header_value(&trusted, &value).unwrap_err(), expected);
        let empty_expected = if expected == "invalid-base64-cert" {
            expected
        } else {
            "no-ca-configured"
        };
        assert_eq!(extract_header_value(&empty, &value).unwrap_err(), empty_expected);
    }
}

#[test]
fn base64_header_extraction_preserves_trailing_der() {
    let (ca_pem, leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
    let mut plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&ca_pem)]);
    plugin.config.certificate_header_format = CertHeaderFormat::Base64Encoded;
    let mut der = HeaderCertAuth::parse_first_cert_der(leaf_pem.as_bytes()).unwrap();
    der.extend_from_slice(b"trailing bytes");
    // The previous PEM constructor and the DER constructor both accept a leaf prefix.
    let body = STANDARD.encode(&der);
    let previous_pem = format!("-----BEGIN CERTIFICATE-----\n{body}\n-----END CERTIFICATE-----\n");
    let expected = extract_client_cert_info_from_cert(&X509::from_pem(previous_pem.as_bytes()).unwrap());
    assert_same_identity(&extract_header_value(&plugin, &body).unwrap(), &expected);
}

#[test]
fn base64_header_extraction_preserves_expiry_and_distinct_issuer_filtering() {
    let (trusted_ca, expired_leaf) = make_named_ca_and_leaf("Trusted CA", "expired", -30, 30, -30, -1);
    let (_, foreign_leaf) = make_named_ca_and_leaf("Foreign CA", "foreign", -1, 30, -1, 30);
    let trusted_der = HeaderCertAuth::parse_first_cert_der(trusted_ca.as_bytes()).unwrap();
    let foreign_der = HeaderCertAuth::parse_first_cert_der(foreign_leaf.as_bytes()).unwrap();
    let (_, trusted) = parse_x509_certificate(&trusted_der).unwrap();
    let (_, foreign) = parse_x509_certificate(&foreign_der).unwrap();
    assert_ne!(trusted.subject().as_raw(), foreign.issuer().as_raw());
    let mut plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&trusted_ca)]);
    plugin.config.certificate_header_format = CertHeaderFormat::Base64Encoded;
    assert_eq!(
        extract_header_value(&plugin, &base64_leaf(&expired_leaf)).unwrap_err(),
        "expired-leaf-cert"
    );
    assert_eq!(
        extract_header_value(&plugin, &base64_leaf(&foreign_leaf)).unwrap_err(),
        "ca-verification-failed"
    );
}

#[test]
fn base64_header_extraction_checks_same_subject_keys_and_later_valid_ca() {
    let (wrong_key_ca, _) = make_ca_and_leaf(-1, 30, -1, 30);
    let (trusted_ca, leaf_pem) = make_ca_and_leaf(-1, 30, -1, 30);
    let wrong_der = HeaderCertAuth::parse_first_cert_der(wrong_key_ca.as_bytes()).unwrap();
    let trusted_der = HeaderCertAuth::parse_first_cert_der(trusted_ca.as_bytes()).unwrap();
    let (_, wrong) = parse_x509_certificate(&wrong_der).unwrap();
    let (_, trusted) = parse_x509_certificate(&trusted_der).unwrap();
    assert_eq!(wrong.subject().as_raw(), trusted.subject().as_raw());
    assert_ne!(wrong.public_key().raw, trusted.public_key().raw);
    let mut plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&wrong_key_ca)]);
    plugin.config.certificate_header_format = CertHeaderFormat::Base64Encoded;
    let encoded = base64_leaf(&leaf_pem);
    assert_eq!(
        extract_header_value(&plugin, &encoded).unwrap_err(),
        "ca-verification-failed"
    );
    let mut plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&wrong_key_ca), make_ca_secret(&trusted_ca)]);
    plugin.config.certificate_header_format = CertHeaderFormat::Base64Encoded;
    let expected = extract_client_cert_info_from_cert(&X509::from_pem(leaf_pem.as_bytes()).unwrap());
    assert_same_identity(&extract_header_value(&plugin, &encoded).unwrap(), &expected);
}

#[test]
fn url_header_extraction_keeps_first_pem_identity_and_envelope_behavior() {
    let (trusted_ca, verified_leaf) = make_named_ca_and_leaf("Trusted CA", "verified", -1, 30, -1, 30);
    let (_, older_leaf) = make_named_ca_and_leaf("Other CA", "older-identity", -1, 30, -1, 30);
    let mut plugin = header_mode_plugin_with_ca(vec![make_ca_secret(&trusted_ca)]);
    plugin.config.certificate_header_format = CertHeaderFormat::UrlEncoded;
    let verified_info = extract_client_cert_info_from_cert(&X509::from_pem(verified_leaf.as_bytes()).unwrap());
    let older_info = extract_client_cert_info_from_cert(&X509::from_pem(older_leaf.as_bytes()).unwrap());
    assert_ne!(verified_info.subject, older_info.subject);
    assert_ne!(verified_info.fingerprint, older_info.fingerprint);
    assert_same_identity(
        &extract_header_value(&plugin, &url_pem(&verified_leaf)).unwrap(),
        &verified_info,
    );
    let multiple = format!("{verified_leaf}{older_leaf}");
    assert_same_identity(
        &extract_header_value(&plugin, &url_pem(&multiple)).unwrap(),
        &verified_info,
    );
    let older_label = older_leaf.replace("CERTIFICATE", "X509 CERTIFICATE");
    let mixed = format!("{older_label}{verified_leaf}");
    // Verification skips the older label; BoringSSL identity extraction accepts it.
    assert_same_identity(&extract_header_value(&plugin, &url_pem(&mixed)).unwrap(), &older_info);
    let mismatched = verified_leaf.replace("END CERTIFICATE", "END OTHER");
    assert!(plugin.verify_against_ca(mismatched.as_bytes()).is_ok());
    assert_eq!(
        extract_header_value(&plugin, &url_pem(&mismatched)).unwrap_err(),
        "invalid-cert-pem"
    );
    let mut empty = header_mode_plugin_with_ca(Vec::new());
    empty.config.certificate_header_format = CertHeaderFormat::UrlEncoded;
    assert_eq!(
        extract_header_value(&empty, "%FF").unwrap_err(),
        "invalid-url-encoded-cert"
    );
    assert_eq!(
        extract_header_value(&empty, &url_pem(&mismatched)).unwrap_err(),
        "no-ca-configured"
    );
}
