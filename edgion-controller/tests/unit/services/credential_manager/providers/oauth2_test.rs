use super::*;

fn bootstrap(client_id: &[u8], client_secret: &[u8]) -> BTreeMap<String, ByteString> {
    BTreeMap::from([
        (CLIENT_ID_KEY.to_string(), ByteString(client_id.to_vec())),
        (CLIENT_SECRET_KEY.to_string(), ByteString(client_secret.to_vec())),
    ])
}

#[test]
fn bootstrap_parser_enforces_fixed_keys_utf8_and_bounds_without_leaking_values() {
    let credentials = BootstrapCredentials::parse(&bootstrap(b"client-id-value", b"private-value")).expect("valid");
    let debug = format!("{credentials:?}");
    assert!(!debug.contains("client-id-value"));
    assert!(!debug.contains("private-value"));

    assert_eq!(
        BootstrapCredentials::parse(&BTreeMap::new()).unwrap_err(),
        BootstrapCredentialError::MissingKey(CLIENT_ID_KEY)
    );
    assert_eq!(
        BootstrapCredentials::parse(&bootstrap(&[0xff], b"secret")).unwrap_err(),
        BootstrapCredentialError::InvalidUtf8(CLIENT_ID_KEY)
    );
    assert!(matches!(
        BootstrapCredentials::parse(&bootstrap(b"client", &vec![b'x'; CLIENT_SECRET_MAX_LEN + 1])),
        Err(BootstrapCredentialError::TooLong {
            key: CLIENT_SECRET_KEY,
            ..
        })
    ));
}

#[test]
fn basic_auth_independently_form_encodes_id_and_secret() {
    let credentials = BootstrapCredentials::parse(&bootstrap(b"client id", b"s:e+cret")).expect("valid");
    let header = build_basic_authorization(&credentials).expect("header");
    let encoded = header.to_str().expect("ASCII").strip_prefix("Basic ").expect("scheme");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("base64");
    assert_eq!(decoded, b"client+id:s%3Ae%2Bcret");
    assert!(header.is_sensitive());
}

#[test]
fn request_form_is_fixed_and_scope_is_encoded() {
    assert_eq!(build_request_form(&[]).unwrap(), "grant_type=client_credentials");
    assert_eq!(
        build_request_form(&["read:data".to_string(), "write/data".to_string()]).unwrap(),
        "grant_type=client_credentials&scope=read%3Adata+write%2Fdata"
    );
}

#[test]
fn success_response_requires_bearer_token_and_integral_bounded_lifetime() {
    let token = parse_success_response(br#"{"access_token":"abc-._~+/=","token_type":"bearer","expires_in":40}"#)
        .expect("valid token");
    assert_eq!(token.access_token, "abc-._~+/=");
    assert_eq!(token.expires_in, Duration::from_secs(40));

    for invalid in [
        br#"{"access_token":"abc","token_type":"MAC","expires_in":40}"#.as_slice(),
        br#"{"access_token":"abc","token_type":"Bearer","expires_in":39}"#.as_slice(),
        br#"{"access_token":"abc","token_type":"Bearer","expires_in":604801}"#.as_slice(),
        br#"{"access_token":"abc","token_type":"Bearer","expires_in":40.5}"#.as_slice(),
        br#"{"access_token":"bad token","token_type":"Bearer","expires_in":40}"#.as_slice(),
    ] {
        assert_eq!(
            parse_success_response(invalid).unwrap_err().kind,
            OAuth2FailureKind::Permanent
        );
    }
}

#[test]
fn response_classification_only_allows_typed_invalid_client_fallback() {
    let invalid_client = br#"{"error":"invalid_client","error_description":"ignored"}"#;
    assert_eq!(
        classify_response(StatusCode::UNAUTHORIZED, invalid_client, None)
            .unwrap_err()
            .kind,
        OAuth2FailureKind::InvalidClient
    );
    for (status, body, expected) in [
        (
            StatusCode::UNAUTHORIZED,
            br#"{}"#.as_slice(),
            OAuth2FailureKind::Permanent,
        ),
        (
            StatusCode::BAD_REQUEST,
            br#"{"error":"invalid_scope"}"#.as_slice(),
            OAuth2FailureKind::Permanent,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            br#"{"error":"slow_down"}"#.as_slice(),
            OAuth2FailureKind::Transient,
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            b"not-json".as_slice(),
            OAuth2FailureKind::Transient,
        ),
    ] {
        assert_eq!(classify_response(status, body, None).unwrap_err().kind, expected);
    }
}

#[test]
fn retry_after_is_limited_to_429_and_503_and_capped() {
    let cap = Duration::from_secs(60);
    let value = HeaderValue::from_static("120");
    assert_eq!(
        retry_after_for(StatusCode::TOO_MANY_REQUESTS, Some(&value), cap),
        Some(cap)
    );
    assert_eq!(
        retry_after_for(StatusCode::SERVICE_UNAVAILABLE, Some(&value), cap),
        Some(cap)
    );
    assert_eq!(retry_after_for(StatusCode::BAD_GATEWAY, Some(&value), cap), None);
    assert_eq!(retry_after_for(StatusCode::UNAUTHORIZED, Some(&value), cap), None);
    assert_eq!(
        retry_after_for(StatusCode::TOO_MANY_REQUESTS, Some(&HeaderValue::from_static("0")), cap),
        None
    );
}

fn oauth_actual_owner(tls: serde_json::Value) -> edgion_resources::resources::link_sys::LinkSys {
    serde_json::from_value(serde_json::json!({
        "metadata":{"name":"issuer","namespace":"apps","uid":"issuer-uid","generation":7},
        "spec":{"type":"credentialSource","config":{"provider":{
            "type":"oauth2ClientCredentials","tokenEndpoint":"https://issuer.example/token",
            "clientAuthentication":{"activeSecretRef":{"name":"bootstrap"}},"tls":tls
        }}}
    }))
    .unwrap()
}

fn oauth_defaults(
    config: serde_json::Value,
) -> Arc<edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot> {
    Arc::new(
        edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot::from_configs([Arc::new(
            serde_json::from_value::<edgion_resources::EdgionGatewayConfig>(config).unwrap(),
        )]),
    )
}

fn oauth_build(prepared: &PreparedLinkSysTls) -> Result<OAuth2Provider, OAuth2ProviderError> {
    OAuth2Provider::new(
        prepared,
        Duration::from_secs(3),
        Duration::from_secs(8),
        ResolverSpec::System,
    )
}

#[test]
fn literal_issuer_egress_uses_source_policy_and_shared_testing_exception() {
    use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
    const CHILD: &str = "EDGION_ARCH104_OAUTH_EGRESS_CHILD";
    const TEST: &str = "services::credential_manager::providers::oauth2::tests::literal_issuer_egress_uses_source_policy_and_shared_testing_exception";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    enum Expected {
        Public,
        Private,
        Loopback,
        Blocked,
    }
    use Expected::*;
    let cases = [
        ("https://8.8.8.8/token", Public),
        ("https://[2606:4700:4700::1111]/token", Public),
        ("https://[::ffff:8.8.8.8]/token", Public),
        ("https://10.0.0.1/token", Private),
        ("https://[fd00::1]/token", Private),
        ("https://[::ffff:10.0.0.1]/token", Private),
        ("https://127.0.0.1/token", Loopback),
        ("https://127.1/token", Loopback),
        ("https://0x7f000001/token", Loopback),
        ("https://[::1]/token", Loopback),
        ("https://[::ffff:127.0.0.1]/token", Loopback),
        ("https://169.254.169.254/token", Blocked),
        ("https://[fe80::1]/token", Blocked),
        ("https://[::ffff:169.254.1.1]/token", Blocked),
        ("https://0.0.0.0/token", Blocked),
        ("https://[::]/token", Blocked),
        ("https://[::ffff:0.0.0.0]/token", Blocked),
        ("https://issuer.example/token", Public),
    ];
    for testing in [false, true] {
        edgion_common::test_mode::init_integration_testing_mode(testing);
        for block_private in [true, false] {
            for (endpoint, expected) in &cases {
                let mut owner = oauth_actual_owner(serde_json::Value::Null);
                let SystemConfig::CredentialSource(source) = &mut owner.spec.config else {
                    unreachable!()
                };
                source.egress.block_private = block_private;
                let CredentialSourceProvider::OAuth2ClientCredentials(config) = &mut source.provider;
                config.token_endpoint = endpoint.to_string();
                let prepared = PreparedLinkSysTls::with_snapshot(
                    Arc::new(owner),
                    Arc::new(GlobalOutboundTlsSnapshot::from_configs([])),
                )
                .unwrap();
                let result = OAuth2Provider::new(
                    &prepared,
                    Duration::from_secs(3),
                    Duration::from_secs(8),
                    ResolverSpec::StandardDns(edgion_common::dns::StandardDnsServers::new(
                        vec!["192.0.2.53:53".into()],
                    )),
                );
                let allowed = match expected {
                    Public => true,
                    Private => !block_private,
                    Loopback => testing,
                    Blocked => false,
                };
                assert_eq!(
                    result.is_ok(),
                    allowed,
                    "{endpoint} private={block_private} testing={testing}"
                );
                match result {
                    Ok(provider) => assert_eq!(provider.token_endpoint, Url::parse(endpoint).unwrap()),
                    Err(error) => {
                        assert_eq!(error.kind, OAuth2FailureKind::Permanent);
                        assert_eq!(error.retry_after, None);
                        assert!(!error.to_string().contains(*endpoint));
                    }
                }
            }
        }
    }
}

#[test]
fn owning_oauth_rejects_invalid_source_and_whole_material_mismatch() {
    use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
    use serde_json::json;
    let empty = || Arc::new(GlobalOutboundTlsSnapshot::from_configs([]));
    let system = PreparedLinkSysTls::with_snapshot(
        Arc::new(oauth_actual_owner(json!({"enabled":true,"verify":true,
            "validation":{"wellKnownCACertificates":"System"}
        }))),
        empty(),
    )
    .unwrap();
    let system_client = oauth_build(&system).unwrap();
    assert_eq!(system_client.token_endpoint.scheme(), "https");
    let mut invalid_owner = oauth_actual_owner(serde_json::Value::Null);
    invalid_owner.metadata.namespace = Some("Invalid".into());
    assert!(PreparedLinkSysTls::with_snapshot(Arc::new(invalid_owner), empty()).is_err());
    let other = serde_json::from_value(json!({"metadata":{"name":"other","namespace":"apps"},"spec":{"type":"elasticsearch","config":{"endpoints":["https://example.com"]}}})).unwrap();
    let other = PreparedLinkSysTls::with_snapshot(Arc::new(other), empty()).unwrap();
    assert_eq!(oauth_build(&other).err().unwrap().kind, OAuth2FailureKind::Permanent);
    let paired = json!({"enabled":true,"verify":false,
        "validation":{"caCertificateRefs":[{"kind":"Secret","name":"first"},{"kind":"Secret","name":"second"}]},
        "resolvedCaCertificates":[{"metadata":{"name":"first","namespace":"apps"}},{"metadata":{"name":"second","namespace":"apps"}}],
        "clientCertificateRef":{"name":"client"},"resolvedClientCertificate":{"metadata":{"name":"client","namespace":"apps"}}
    });
    assert!(PreparedLinkSysTls::with_snapshot(Arc::new(oauth_actual_owner(paired.clone())), empty()).is_ok());
    for mutation in ["missing", "wrong-name", "wrong-namespace", "ca-order"] {
        let mut tls = paired.clone();
        match mutation {
            "missing" => tls["resolvedClientCertificate"] = serde_json::Value::Null,
            "wrong-name" => tls["resolvedClientCertificate"]["metadata"]["name"] = "decoy".into(),
            "wrong-namespace" => tls["resolvedClientCertificate"]["metadata"]["namespace"] = "decoy".into(),
            _ => tls["resolvedCaCertificates"].as_array_mut().unwrap().swap(0, 1),
        }
        let error = PreparedLinkSysTls::with_snapshot(Arc::new(oauth_actual_owner(tls)), empty())
            .err()
            .unwrap();
        if mutation != "missing" {
            assert_eq!(error.to_string(), "outbound TLS reference/material identity mismatch");
        }
    }
}

#[tokio::test]
async fn owning_oauth_captures_actual_owner_and_global_generation_across_await() {
    use edgion_common::link_sys_core::outbound_tls::resolve_validated_outbound_tls;
    use serde_json::json;
    let owner = Arc::new(oauth_actual_owner(json!({"enabled":true})));
    let defaults = oauth_defaults(json!({"metadata":{"name":"global"},"spec":{"outboundTls":{"verify":false}}}));
    let prepared = PreparedLinkSysTls::with_snapshot(Arc::clone(&owner), Arc::clone(&defaults)).unwrap();
    assert!(std::ptr::eq(prepared.owner(), owner.as_ref()));
    let weak = Arc::downgrade(&owner);
    drop(owner);
    tokio::task::yield_now().await;
    assert!(weak.upgrade().is_some());
    assert_eq!(prepared.owner().metadata.uid.as_deref(), Some("issuer-uid"));
    assert_eq!(prepared.owner().metadata.namespace.as_deref(), Some("apps"));
    assert_eq!(prepared.owner().metadata.generation, Some(7));
    prepared
        .with_tls_selection(|proof, captured| {
            assert!(std::ptr::eq(captured, defaults.as_ref()));
            assert!(!resolve_validated_outbound_tls(proof, true, captured).unwrap().verify);
            Ok(())
        })
        .unwrap();
    let next = oauth_defaults(
        json!({"metadata":{"name":"next"},"spec":{"outboundTls":{"clientCertificateRef":{"name":"missing","namespace":"apps"}}}}),
    );
    let new_generation =
        PreparedLinkSysTls::with_snapshot(Arc::new(oauth_actual_owner(json!({"enabled":true}))), next).unwrap();
    assert!(new_generation.validate_active_inheritance().is_err());
    assert_eq!(
        oauth_build(&new_generation).err().unwrap().kind,
        OAuth2FailureKind::Permanent
    );
    let client = oauth_build(&prepared).unwrap();
    assert_eq!(client.token_endpoint.scheme(), "https");
    assert_eq!(client.retry_after_cap, Duration::from_secs(8));
    drop(prepared);
    assert!(
        weak.upgrade().is_none(),
        "built client does not retain the owner payload"
    );
}

#[test]
fn owning_oauth_inactive_tls_keeps_https_without_consuming_invalid_globals() {
    use serde_json::json;
    let invalid = oauth_defaults(
        json!({"metadata":{"name":"global"},"spec":{"outboundTls":{"clientCertificateRef":{"name":"missing","namespace":"apps"}}}}),
    );
    for tls in [
        serde_json::Value::Null,
        json!({"enabled":false,"clientCertificateRef":{"name":"Missing Invalid"},"resolvedClientCertificate":{"metadata":{"name":"decoy","namespace":"wrong"}}}),
    ] {
        let prepared =
            PreparedLinkSysTls::with_snapshot(Arc::new(oauth_actual_owner(tls)), Arc::clone(&invalid)).unwrap();
        prepared.validate_active_inheritance().unwrap();
        let client = oauth_build(&prepared).unwrap();
        assert_eq!(client.token_endpoint.scheme(), "https");
    }
}
