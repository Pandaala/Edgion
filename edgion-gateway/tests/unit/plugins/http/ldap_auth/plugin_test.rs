use super::*;
use edgion_resources::resources::edgion_plugins::LdapAuthConfig;
use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;

fn actual_ldap(config: &LdapAuthConfig) -> Result<LdapAuth, String> {
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"ldap-test","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"LdapAuth","config":config}]}
    }))
    .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    LdapAuth::new(
        &entry,
        &edgion_common::link_sys_core::outbound_tls::outbound_global_default(),
    )
}

use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use edgion_resources::resources::common::CustomCACertificateRef;
use edgion_resources::resources::common::OutboundTlsLocal;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;

fn test_config() -> LdapAuthConfig {
    LdapAuthConfig {
        ldap_host: "ldap.example.com".to_string(),
        base_dn: "dc=example,dc=com".to_string(),
        attribute: "uid".to_string(),
        ..Default::default()
    }
}

fn install_test_crypto_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Transport kind and, for TLS transports, the parsed server name.
fn transport_summary(auth: &LdapAuth) -> (&'static str, Option<String>) {
    let name = |name: &Option<ServerName<'static>>| name.as_ref().map(|name| name.to_str().into_owned());
    match &auth.transport {
        LdapTransport::Plain => ("plain", None),
        LdapTransport::Ldaps { server_name, .. } => ("ldaps", name(server_name)),
        LdapTransport::StartTls { server_name, .. } => ("starttls", name(server_name)),
    }
}

fn ldap_auth_default() -> LdapAuth {
    actual_ldap(&test_config()).expect("default plain LDAP must build")
}

#[test]
fn test_parse_authorization_value_invalid() {
    let auth = ldap_auth_default();
    assert!(auth.parse_authorization_value("Basic abc").is_none());
    assert!(auth.parse_authorization_value("ldap !!!").is_none());
    assert!(auth.parse_authorization_value("ldap").is_none());
}

#[test]
fn decoded_credentials_use_one_byte_safe_utf8_backing_buffer() {
    let auth = ldap_auth_default();
    for raw_username in ["José", "  José", "José  ", "\t José \n", "\u{00A0}José\u{2003}"] {
        let payload = format!("{raw_username}: pä:ss ");
        let header = format!("LdAp {}", general_purpose::STANDARD.encode(&payload));
        let credentials = auth.parse_authorization_value(&header).expect("credentials must parse");

        assert_eq!(credentials.username(), "José");
        assert_eq!(credentials.password(), " pä:ss ");
        assert_eq!(credentials.decoded, payload);
        let backing = credentials.decoded.as_bytes().as_ptr_range();
        assert!(backing.contains(&credentials.username().as_ptr()));
        assert!(backing.contains(&credentials.password().as_ptr()));
    }
    for payload in [":password", " \t\u{00A0}\u{2003}:password", "José:"] {
        let header = format!("ldap {}", general_purpose::STANDARD.encode(payload));
        assert!(auth.parse_authorization_value(&header).is_none());
    }
}

#[test]
fn parse_credentials_short_circuits_authorization_when_proxy_is_present() {
    use crate::plugins::runtime::traits::session::MockPluginSession;

    let auth = ldap_auth_default();
    let proxy = format!("ldap {}", general_purpose::STANDARD.encode("alice:password"));
    // A *different* valid credential in `authorization`: the assertion below
    // only holds if `proxy-authorization` short-circuits it. Asserting the
    // winner positively is stronger than asserting `authorization` is never
    // read, because it survives any change to how the header is fetched.
    let fallback = format!("ldap {}", general_purpose::STANDARD.encode("mallory:other"));
    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[
            ("proxy-authorization", proxy.as_str()),
            ("authorization", fallback.as_str()),
        ],
    );

    match auth.parse_credentials(&session) {
        ParseAuthResult::Valid(credentials) => {
            assert_eq!(credentials.username(), "alice");
            assert_eq!(credentials.password(), "password");
        }
        ParseAuthResult::Missing | ParseAuthResult::Invalid => {
            panic!("proxy credentials must be valid")
        }
    }
}

#[test]
fn invalid_proxy_does_not_fall_back_to_authorization() {
    use crate::plugins::runtime::traits::session::MockPluginSession;

    let auth = ldap_auth_default();
    // A malformed proxy header must be terminal. The valid `authorization`
    // below would parse cleanly, so a `Valid` result here would prove a
    // fallback happened.
    let fallback = format!("ldap {}", general_purpose::STANDARD.encode("mallory:other"));
    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[
            ("proxy-authorization", "ldap !!!"),
            ("authorization", fallback.as_str()),
        ],
    );

    assert!(matches!(auth.parse_credentials(&session), ParseAuthResult::Invalid));
}

#[test]
fn test_build_bind_dn_injection_escaped() {
    let auth = ldap_auth_default();

    // RFC 4514 metacharacters must be escaped so injection becomes a
    // literal value inside the RDN. edgion_ldap::dn::escape uses the hex form
    // (\HH); both forms are RFC-equivalent for any compliant server.
    let dn = auth.build_bind_dn("alice,admin").unwrap();
    assert_eq!(dn, r"uid=alice\2cadmin,dc=example,dc=com");

    let dn = auth.build_bind_dn("alice+admin").unwrap();
    assert_eq!(dn, r"uid=alice\2badmin,dc=example,dc=com");

    // Leading '#' and trailing ' ' are also escaped per RFC 4514.
    let dn = auth.build_bind_dn("#bob ").unwrap();
    assert_eq!(dn, r"uid=\23bob\20,dc=example,dc=com");

    // Sanity check: no bare injection-relevant metachar remains inside the
    // attribute value. (`\` is the escape leader produced by `escape` and
    // is expected to appear; the metachars below would terminate the RDN
    // or change its semantics if they leaked through unescaped.)
    let dn = auth.build_bind_dn(r#"x"y\z<a>b;c=d"#).unwrap();
    let value = dn
        .strip_prefix("uid=")
        .and_then(|s| s.strip_suffix(",dc=example,dc=com"))
        .unwrap();
    for ch in ['"', '<', '>', ';', '=', ',', '+'] {
        assert!(
            !value.contains(ch),
            "bare metachar {:?} leaked into RDN value {:?}",
            ch,
            value
        );
    }
}

#[test]
fn test_build_bind_dn_accepts_spaces() {
    // AD sAMAccountName may contain spaces. Interior spaces are not RFC 4514
    // metacharacters and must pass through unescaped.
    let auth = ldap_auth_default();
    let dn = auth.build_bind_dn("John Smith").unwrap();
    assert_eq!(dn, "uid=John Smith,dc=example,dc=com");
}

#[test]
fn test_build_bind_dn_accepts_unicode() {
    // Multi-byte UTF-8 in CN/uid values (e.g. accented Latin) is valid in
    // RFC 4514 attribute values and must not be rejected or escaped.
    let auth = ldap_auth_default();
    let dn = auth.build_bind_dn("José").unwrap();
    assert_eq!(dn, "uid=José,dc=example,dc=com");
}

#[test]
fn test_build_bind_dn_rejects_empty() {
    let auth = ldap_auth_default();
    assert!(auth.build_bind_dn("").is_err());
}

#[test]
fn test_build_bind_dn_template_escapes_username() {
    // The {username} placeholder must be escaped before substitution so a
    // template-based deployment is equally injection-safe.
    let config = LdapAuthConfig {
        ldap_host: "ldap.example.com".to_string(),
        base_dn: "dc=example,dc=com".to_string(),
        attribute: "uid".to_string(),
        bind_dn_template: Some("uid={username},ou=people,dc=example,dc=com".to_string()),
        ..Default::default()
    };
    let auth = actual_ldap(&config).expect("must build");
    let dn = auth.build_bind_dn("alice,admin").unwrap();
    assert_eq!(dn, r"uid=alice\2cadmin,ou=people,dc=example,dc=com");
}

fn secret_with(entries: &[(&str, &[u8])]) -> Secret {
    let data = entries
        .iter()
        .map(|(name, value)| ((*name).to_string(), ByteString(value.to_vec())))
        .collect();
    Secret {
        data: Some(data),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    }
}

#[test]
fn plain_ldap_preserves_caller_secrets_without_retaining_config() {
    let mut config = test_config();
    config.tls = OutboundTlsLocal {
        verify: Some(false),
        ..Default::default()
    };
    config.resolved_ca_certificates = Some(vec![secret_with(&[("ca.crt", b"caller-ca")])]);
    config.resolved_client_certificate = Some(secret_with(&[
        ("tls.crt", b"caller-client-cert"),
        ("tls.key", b"caller-client-key"),
    ]));
    let original_ca = config.resolved_ca_certificates.clone();
    let original_client = config.resolved_client_certificate.clone();

    let auth = actual_ldap(&config).expect("plain LDAP must build");
    assert_eq!(transport_summary(&auth), ("plain", None));
    assert_eq!(auth.ldap_host, "ldap.example.com");
    assert_eq!(auth.ldap_port, 389);
    assert_eq!(config.resolved_ca_certificates, original_ca);
    assert_eq!(config.resolved_client_certificate, original_client);
}

#[tokio::test]
async fn prepared_plain_generations_remain_independent() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LdapAuth>();

    let k1_config = LdapAuthConfig {
        ldap_host: "ldap-one.example.com".to_string(),
        ldap_port: 1389,
        base_dn: "dc=one,dc=example".to_string(),
        attribute: "uid".to_string(),
        header_type: "ldap-one".to_string(),
        realm: "Realm One".to_string(),
        timeout: 111,
        ..Default::default()
    };
    let k1 = actual_ldap(&k1_config).expect("K1 must build");

    let k2_config = LdapAuthConfig {
        ldap_host: "ldap-two.example.com".to_string(),
        ldap_port: 2389,
        base_dn: "dc=unused,dc=example".to_string(),
        attribute: "cn".to_string(),
        bind_dn_template: Some("cn={username},ou=people,dc=two,dc=example".to_string()),
        header_type: "ldap-two".to_string(),
        realm: "Realm Two".to_string(),
        anonymous: Some("guest-two".to_string()),
        hide_credentials: true,
        timeout: 222,
        ..Default::default()
    };
    let k2 = actual_ldap(&k2_config).expect("K2 must build");

    assert_eq!(transport_summary(&k1), ("plain", None));
    assert_eq!(k1.ldap_host, "ldap-one.example.com");
    assert_eq!(k1.ldap_port, 1389);
    assert_eq!(k1.timeout, Duration::from_millis(111));
    assert_eq!(k1.auth.header_type, "ldap-one");
    assert_prepared_challenge(&k1.auth.auth_challenge, "ldap-one realm=\"Realm One\"").await;
    assert!(matches!(k1.bind_dn_plan, BindDnPlan::AttributeBase { .. }));

    assert_eq!(transport_summary(&k2), ("plain", None));
    assert_eq!(k2.ldap_host, "ldap-two.example.com");
    assert_eq!(k2.ldap_port, 2389);
    assert_eq!(k2.timeout, Duration::from_millis(222));
    assert_eq!(k2.auth.header_type, "ldap-two");
    assert_prepared_challenge(&k2.auth.auth_challenge, "ldap-two realm=\"Realm Two\"").await;
    assert_eq!(k2.auth.anonymous.as_deref(), Some("guest-two"));
    assert!(k2.auth.hide_credentials);
    assert!(matches!(k2.bind_dn_plan, BindDnPlan::Template(_)));

    assert_eq!(k1.ldap_host, "ldap-one.example.com");
    assert_prepared_challenge(&k1.auth.auth_challenge, "ldap-one realm=\"Realm One\"").await;
    assert_eq!(k1.build_bind_dn("alice").unwrap(), "uid=alice,dc=one,dc=example");
}

#[test]
fn plugin_name_is_static_across_instances() {
    let first = ldap_auth_default();
    let second = ldap_auth_default();
    assert_eq!(first.name(), PLUGIN_NAME);
    assert_eq!(second.name(), PLUGIN_NAME);
    assert_eq!(first.name().as_ptr(), second.name().as_ptr());
}

#[tokio::test]
async fn anonymous_applies_only_when_both_credential_headers_are_missing() {
    use crate::plugins::runtime::traits::session::MockPluginSession;

    let config = LdapAuthConfig {
        anonymous: Some("guest".to_string()),
        ..test_config()
    };
    let auth = actual_ldap(&config).expect("anonymous plugin must build");
    let mut missing = MockPluginSession::new();
    setup_header_mocks(&mut missing, &[]);
    missing
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Credential-Identifier" && value == "guest" && value.is_sensitive())
        .times(1)
        .returning(|_, _| Ok(()));
    missing
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Anonymous-Consumer" && value == "true" && value.is_sensitive())
        .times(1)
        .returning(|_, _| Ok(()));
    let mut missing_log = PluginLog::new(PLUGIN_NAME);
    assert_eq!(
        auth.run_request(&mut missing, &mut missing_log).await,
        PluginRunningResult::GoodNext
    );
    assert!(missing_log.contains("Allow anonymous"));

    let mut invalid = MockPluginSession::new();
    setup_header_mocks(&mut invalid, &[("proxy-authorization", "ldap invalid-base64")]);
    invalid.expect_write_response_header().times(1).returning(|_, _| Ok(()));
    invalid.expect_write_response_body().times(1).returning(|body, end| {
        assert_eq!(body.as_deref(), Some(Bytes::from_static(b"401 Unauthorized").as_ref()));
        assert!(end);
        Ok(())
    });
    invalid.expect_shutdown().times(1).returning(|| {});
    let mut invalid_log = PluginLog::new(PLUGIN_NAME);
    assert_eq!(
        auth.run_request(&mut invalid, &mut invalid_log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(invalid_log.contains("Deny invalid header"));
    assert!(!invalid_log.contains("Allow anonymous"));
}

#[test]
fn ldap_new_tls_cases_are_process_isolated() {
    const CHILD_ENV: &str = "EDGION_LDAP_AUTH_NEW_TLS_CHILD";
    const TEST_NAME: &str = "ldap_new_tls_cases_are_process_isolated";
    const TEST_MODULE: &str = "plugins::http::ldap_auth::plugin::tests";

    if std::env::var(CHILD_ENV).as_deref() != Ok(TEST_NAME) {
        let exact_test_name = format!("{TEST_MODULE}::{TEST_NAME}");
        let output = std::process::Command::new(
            std::env::current_exe().expect("the Rust test harness must have a current executable"),
        )
        .args(["--exact", exact_test_name.as_str(), "--nocapture"])
        .env(CHILD_ENV, TEST_NAME)
        .output()
        .expect("the isolated LdapAuth TLS constructor test process must start");
        assert!(
            output.status.success(),
            "isolated LdapAuth TLS constructor test failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    run_ldap_new_tls_case_table();
}

fn run_ldap_new_tls_case_table() {
    use edgion_common::link_sys_core::outbound_tls::set_outbound_global_default;
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    use edgion_resources::resources::common::{OutboundTlsConfig, SecretObjectReference};

    install_test_crypto_provider();
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));

    let ldaps = actual_ldap(&LdapAuthConfig {
        ldaps: true,
        ldap_port: 636,
        ..test_config()
    })
    .expect("LDAPS with system roots must build");
    assert_eq!(
        transport_summary(&ldaps),
        ("ldaps", Some("ldap.example.com".to_string()))
    );
    assert_eq!((ldaps.ldap_host.as_str(), ldaps.ldap_port), ("ldap.example.com", 636));

    let starttls = actual_ldap(&LdapAuthConfig {
        start_tls: true,
        ..test_config()
    })
    .expect("StartTLS with system roots must build");
    assert_eq!(
        transport_summary(&starttls),
        ("starttls", Some("ldap.example.com".to_string()))
    );
    assert_eq!(
        (starttls.ldap_host.as_str(), starttls.ldap_port),
        ("ldap.example.com", 389)
    );

    let skip_verify = actual_ldap(&LdapAuthConfig {
        ldaps: true,
        tls: OutboundTlsLocal {
            verify: Some(false),
            ..Default::default()
        },
        ..test_config()
    })
    .expect("effective verify=false must build the explicit skip-verifier path");
    assert_eq!(
        transport_summary(&skip_verify),
        ("ldaps", Some("ldap.example.com".to_string()))
    );
    assert_eq!(
        (skip_verify.ldap_host.as_str(), skip_verify.ldap_port),
        ("ldap.example.com", 389)
    );

    let san_override = actual_ldap(&LdapAuthConfig {
        ldaps: true,
        tls: OutboundTlsLocal {
            verify: Some(false),
            validation: Some(OutboundTlsValidation {
                subject_alt_names: Some(vec![SubjectAltName {
                    san_type: SubjectAltNameType::Hostname,
                    hostname: Some("ldap.example.com".to_string()),
                    uri: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..test_config()
    })
    .expect("SAN policy must override verify=false and retain secure TLS");
    assert_eq!(
        transport_summary(&san_override),
        ("ldaps", Some("ldap.example.com".to_string()))
    );
    assert_eq!(
        (san_override.ldap_host.as_str(), san_override.ldap_port),
        ("ldap.example.com", 389)
    );

    let missing_ca = actual_ldap(&LdapAuthConfig {
        ldaps: true,
        tls: OutboundTlsLocal {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![CustomCACertificateRef {
                    kind: "Secret".to_string(),
                    name: "missing-ca".to_string(),
                    group: Some(String::new()),
                    namespace: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..test_config()
    });
    assert!(missing_ca.is_err(), "unresolved declared CA must fail closed");

    let invalid_ca = actual_ldap(&LdapAuthConfig {
        ldaps: true,
        tls: OutboundTlsLocal {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![CustomCACertificateRef {
                    kind: "Secret".to_string(),
                    name: "invalid-ca".to_string(),
                    group: Some(String::new()),
                    namespace: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        },
        resolved_ca_certificates: Some(vec![Secret {
            metadata: kube::core::ObjectMeta {
                name: Some("invalid-ca".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            ..secret_with(&[("ca.crt", b"not a PEM certificate")])
        }]),
        ..test_config()
    });
    assert!(invalid_ca.is_err(), "invalid declared CA must fail construction");

    let client_ref = SecretObjectReference {
        group: None,
        kind: None,
        name: "ldap-client".to_string(),
        namespace: Some("default".into()),
    };
    let missing_client = actual_ldap(&LdapAuthConfig {
        ldaps: true,
        tls: OutboundTlsLocal {
            client_certificate_ref: Some(client_ref.clone()),
            ..Default::default()
        },
        ..test_config()
    });
    assert!(
        missing_client.is_err(),
        "unresolved declared client identity must fail closed"
    );

    let invalid_client = actual_ldap(&LdapAuthConfig {
        ldaps: true,
        tls: OutboundTlsLocal {
            client_certificate_ref: Some(client_ref.clone()),
            ..Default::default()
        },
        resolved_client_certificate: Some(Secret {
            metadata: kube::core::ObjectMeta {
                name: Some("ldap-client".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            ..secret_with(&[("tls.crt", b"invalid certificate"), ("tls.key", b"invalid private key")])
        }),
        ..test_config()
    });
    assert!(
        invalid_client.is_err(),
        "invalid declared client identity must fail construction"
    );

    let generated = rcgen::generate_simple_self_signed(vec!["client.example.com".to_string()])
        .expect("test client identity must generate");
    let cert_pem = generated.cert.pem();
    let key_pem = generated.signing_key.serialize_pem();
    let mut global_identity = secret_with(&[("tls.crt", cert_pem.as_bytes()), ("tls.key", key_pem.as_bytes())]);
    global_identity.metadata.name = Some("ldap-client".into());
    global_identity.metadata.namespace = Some("default".into());
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            client_certificate_ref: Some(client_ref),
            resolved_client_certificate: Some(global_identity),
            ..Default::default()
        },
    ));
    let inherited_client = actual_ldap(&LdapAuthConfig {
        ldaps: true,
        ..test_config()
    });
    assert!(
        inherited_client.is_ok(),
        "local TLS must inherit the valid global client identity"
    );

    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            verify: false,
            ..Default::default()
        },
    ));
    let inherited_skip_verify = actual_ldap(&LdapAuthConfig {
        ldaps: true,
        ..test_config()
    });
    assert!(
        inherited_skip_verify.is_ok(),
        "unset local verify must inherit global false"
    );

    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
}

// -----------------------------------------------------------------------
// Helpers for build_ldap_rustls_config SAN tests
// -----------------------------------------------------------------------

fn make_resolved_verify_true(
    san_names: Vec<edgion_resources::resources::backend_tls_policy::SubjectAltName>,
) -> edgion_common::link_sys_core::outbound_tls::ResolvedOutboundTls {
    use edgion_common::link_sys_core::outbound_tls::{EffectiveCa, ResolvedOutboundTls};
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    let san = if san_names.is_empty() {
        None
    } else {
        Some(OutboundTlsValidation {
            subject_alt_names: Some(san_names),
            ..Default::default()
        })
    };
    ResolvedOutboundTls {
        verify: true,
        ca: EffectiveCa::System,
        client_identity: None,
        san,
    }
}

fn make_resolved_verify_false_with_san(
    san_names: Vec<edgion_resources::resources::backend_tls_policy::SubjectAltName>,
) -> edgion_common::link_sys_core::outbound_tls::ResolvedOutboundTls {
    use edgion_common::link_sys_core::outbound_tls::{EffectiveCa, ResolvedOutboundTls};
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    let san = if san_names.is_empty() {
        None
    } else {
        Some(OutboundTlsValidation {
            subject_alt_names: Some(san_names),
            ..Default::default()
        })
    };
    ResolvedOutboundTls {
        verify: false,
        ca: EffectiveCa::System,
        client_identity: None,
        san,
    }
}

/// Non-empty SAN policy (verify=true) → OutboundSanVerifier wraps webpki → returns Ok.
#[test]
fn ldap_rustls_config_with_san_policy_wraps_verifier() {
    install_test_crypto_provider();
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
    let san_policy = vec![SubjectAltName {
        san_type: SubjectAltNameType::Hostname,
        hostname: Some("ldap.example.com".to_string()),
        uri: None,
    }];
    let resolved = make_resolved_verify_true(san_policy);
    let result = build_ldap_rustls_config(&resolved);
    assert!(
        result.is_ok(),
        "SAN-wrapped verifier path must build Ok, got: {:?}",
        result.err()
    );
}

/// Empty SAN policy (verify=true) → standard webpki path → returns Ok.
#[test]
fn ldap_rustls_config_without_san_uses_default_webpki() {
    install_test_crypto_provider();
    let resolved = make_resolved_verify_true(vec![]);
    let result = build_ldap_rustls_config(&resolved);
    assert!(
        result.is_ok(),
        "default webpki (no SAN policy) must build Ok, got: {:?}",
        result.err()
    );
}

/// verify=false + non-empty SAN policy → SAN verifier overrides skip-verify (fail-secure).
/// The function must build Ok via the webpki + OutboundSanVerifier path,
/// not via AcceptAllVerifier.
#[test]
fn ldap_rustls_config_verify_false_with_san_forces_secure_path() {
    install_test_crypto_provider();
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
    let san_policy = vec![SubjectAltName {
        san_type: SubjectAltNameType::Hostname,
        hostname: Some("ldap.example.com".to_string()),
        uri: None,
    }];
    let resolved = make_resolved_verify_false_with_san(san_policy);
    // verify=false is overridden by the SAN policy: the function must build
    // a config using webpki + OutboundSanVerifier rather than AcceptAllVerifier.
    let result = build_ldap_rustls_config(&resolved);
    assert!(
        result.is_ok(),
        "verify=false with SAN policy must build Ok via SAN verifier path, got: {:?}",
        result.err()
    );
}

/// A SAN mismatch surfacing from the `tokio-rustls` handshake must map to
/// `TlsSanMismatch`, not the generic `ConnectionFailed`. tokio-rustls reports
/// handshake failures as `io::Error::new(InvalidData, rustls::Error)`; the
/// mapping is exercised directly because a real TLS handshake is out of scope
/// for a unit test.
#[test]
fn ldap_error_with_san_mismatch_maps_to_tls_san_mismatch() {
    let rustls_err = rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
        std::sync::Arc::new(crate::tls::validation::SanMismatchError {
            reason: "declared [ldap.example.com], actual [127.0.0.1]".to_string().into(),
        }),
    )));
    let io_err = std::io::Error::new(std::io::ErrorKind::InvalidData, rustls_err);

    match map_ldap_connect_error(&io_err) {
        LdapAuthError::TlsSanMismatch(reason) => {
            assert_eq!(reason, "declared [ldap.example.com], actual [127.0.0.1]");
        }
        other => panic!("expected TlsSanMismatch, got {:?}", other),
    }
}

#[test]
fn ldap_error_without_san_mismatch_maps_to_connection_failed() {
    let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused");

    assert!(matches!(
        map_ldap_connect_error(&io_err),
        LdapAuthError::ConnectionFailed
    ));
}

// ── Task 9: dye-suite record_outcome wiring ─────────────────────────────

/// `service_unavailable` is the single choke point for every
/// LDAP-unreachable fail-close branch (connection failed, TLS SAN
/// mismatch, timeout, bind failure). It must record `FailClose` with
/// cause `ldap-unreachable` BEFORE its self-write
/// (`send_gateway_plain_error_response`'s `write_response_header`) —
/// enforced via a mockall `Sequence`.
#[tokio::test]
async fn service_unavailable_records_fail_close_before_response_write() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use mockall::Sequence;

    let auth = ldap_auth_default();
    let mut session = MockPluginSession::new();

    let mut seq = Sequence::new();
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "ldap-unreachable")
        .times(1)
        .in_sequence(&mut seq)
        .return_const(());
    session
        .expect_write_response_header()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});

    let result = auth.service_unavailable(&mut session).await;
    assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
}

/// Credential rejection (`unauthorized`, used for missing/invalid/wrong
/// credentials) is an auth decision, not a dependency failure — it must
/// NOT call `record_outcome`. No `expect_record_outcome` is installed, so
/// any call panics the mock.
#[tokio::test]
async fn unauthorized_does_not_record_outcome() {
    use crate::plugins::runtime::traits::session::MockPluginSession;

    let auth = ldap_auth_default();
    let mut session = MockPluginSession::new();
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});

    let result = auth.unauthorized(&mut session).await;
    assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
}

#[tokio::test]
async fn link_local_literal_is_blocked_at_dial_time() {
    let resolver = ldap_egress_resolver();
    let err = connect_admitted_ldap_tcp(&resolver, "169.254.169.254", 389)
        .await
        .expect_err("link-local LDAP targets must fail closed");
    assert!(matches!(err, LdapAuthError::ConnectionFailed));
}

#[tokio::test]
async fn dns_resolution_failure_fails_closed() {
    let resolver = EgressResolver::new(
        ResolverSpec::StandardDns(edgion_common::dns::StandardDnsServers::new(vec![
            "not-a-dns-server".to_string()
        ])),
        EgressPolicy {
            block_private: default_block_private(),
        },
        egress_dns_cache_ttl(),
    );
    let err = connect_admitted_ldap_tcp(&resolver, "ldap.example.com", 389)
        .await
        .expect_err("LDAP DNS failure must fail closed");
    assert!(matches!(err, LdapAuthError::ConnectionFailed));
}

#[tokio::test]
async fn loopback_literal_is_classified_in_unit_tests() {
    let resolver = ldap_egress_resolver();
    let addrs = resolver
        .resolve_socket_addrs("127.0.0.1", 389)
        .await
        .expect("unit tests allow loopback through EgressResolver");
    assert_eq!(addrs, vec!["127.0.0.1:389".parse().unwrap()]);
}

#[tokio::test]
async fn dial_skips_refused_candidate_and_uses_next() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind admitted fallback listener");
    let admitted = listener.local_addr().expect("listener addr");
    let accept = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept fallback connection");
        stream
    });

    let refused = "127.0.0.1:1".parse().unwrap();
    let stream = dial_admitted_addrs(vec![refused, admitted])
        .await
        .expect("second admitted address must be dialed");
    drop(stream);
    accept.await.expect("accepted fallback connection");
}

#[tokio::test]
async fn all_refused_candidates_fail_closed() {
    let refused = "127.0.0.1:1".parse().unwrap();
    let err = dial_admitted_addrs(vec![refused])
        .await
        .expect_err("no admitted TCP candidate must fail closed");
    assert!(matches!(err, LdapAuthError::ConnectionFailed));
}

fn ldap_tls_owner(start_tls: bool) -> (edgion_resources::EdgionPlugins, Vec<u8>, Vec<u8>) {
    let generated = rcgen::generate_simple_self_signed(vec!["ldap.example.com".into()]).unwrap();
    let pem = generated.cert.pem().into_bytes();
    let key = generated.signing_key.serialize_pem().into_bytes();
    let mut ca = secret_with(&[("ca.crt", &pem)]);
    ca.metadata = kube::core::ObjectMeta {
        name: Some("ca".into()),
        namespace: Some("default".into()),
        resource_version: Some("7".into()),
        ..Default::default()
    };
    let mut client = secret_with(&[("tls.crt", &pem), ("tls.key", &key)]);
    client.metadata = kube::core::ObjectMeta {
        name: Some("client".into()),
        namespace: Some("default".into()),
        resource_version: Some("7".into()),
        ..Default::default()
    };
    let owner = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"ldap","uid":"owner"},
        "spec":{"requestPlugins":[{"type":"LdapAuth","config":{
            "ldapHost":"ldap.example.com","baseDn":"dc=example","attribute":"uid",
            "ldaps":!start_tls,"startTls":start_tls,
            "tls":{"verify":true,"validation":{"caCertificateRefs":[{"kind":"Secret","group":"core","name":"ca"}]},"clientCertificateRef":{"group":"core","name":"client"}},
            "resolvedCaCertificates":[ca],"resolvedClientCertificate":client
        }}]}
    })).unwrap();
    (owner, pem, key)
}

#[test]
fn ldap_actual_tls_identity_controls_both_encrypted_transports() {
    use edgion_common::link_sys_core::outbound_tls::test_util::snapshot;
    use edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
    let defaults = snapshot(Default::default());
    for start_tls in [false, true] {
        let (owner, pem, key) = ldap_tls_owner(start_tls);
        let base = serde_json::to_value(&owner).unwrap();
        for case in [
            "matching",
            "configmap",
            "owner-name",
            "owner-missing",
            "owner-ns",
            "ca-name",
            "ca-missing-name",
            "ca-ns",
            "ca-missing-ns",
            "ca-empty-ns",
            "client-name",
            "client-missing-name",
            "client-ns",
            "client-missing-ns",
            "client-empty-ns",
            "source",
            "client-source",
            "retarget",
            "no-ref",
            "no-ca",
            "no-client",
            "kind",
        ] {
            let mut candidate = base.clone();
            match case {
                "owner-name" => candidate["metadata"]["name"] = serde_json::json!("bad/name"),
                "owner-missing" => candidate["metadata"]["name"] = serde_json::Value::Null,
                "owner-ns" => candidate["metadata"]["namespace"] = serde_json::json!(""),
                _ => {}
            }
            let config = &mut candidate["spec"]["requestPlugins"][0]["config"];
            match case {
                "configmap" => {
                    config["tls"]["validation"]["caCertificateRefs"][0]["kind"] = serde_json::json!("ConfigMap");
                    config["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                        serde_json::json!({(CONTROLLER_CA_SOURCE):"ConfigMap"});
                }
                "ca-name" => config["resolvedCaCertificates"][0]["metadata"]["name"] = serde_json::json!("other"),
                "ca-missing-name" => config["resolvedCaCertificates"][0]["metadata"]["name"] = serde_json::Value::Null,
                "ca-ns" => config["resolvedCaCertificates"][0]["metadata"]["namespace"] = serde_json::json!("other"),
                "ca-missing-ns" => {
                    config["resolvedCaCertificates"][0]["metadata"]["namespace"] = serde_json::Value::Null
                }
                "ca-empty-ns" => config["resolvedCaCertificates"][0]["metadata"]["namespace"] = serde_json::json!(""),
                "client-name" => config["resolvedClientCertificate"]["metadata"]["name"] = serde_json::json!("other"),
                "client-missing-name" => {
                    config["resolvedClientCertificate"]["metadata"]["name"] = serde_json::Value::Null
                }
                "client-ns" => {
                    config["resolvedClientCertificate"]["metadata"]["namespace"] = serde_json::json!("other")
                }
                "client-missing-ns" => {
                    config["resolvedClientCertificate"]["metadata"]["namespace"] = serde_json::Value::Null
                }
                "client-empty-ns" => {
                    config["resolvedClientCertificate"]["metadata"]["namespace"] = serde_json::json!("")
                }
                "source" => {
                    config["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                        serde_json::json!({(CONTROLLER_CA_SOURCE):"ConfigMap"})
                }
                "client-source" => {
                    config["resolvedClientCertificate"]["metadata"]["annotations"] =
                        serde_json::json!({(CONTROLLER_CA_SOURCE):"ConfigMap"})
                }
                "retarget" => config["tls"]["validation"]["caCertificateRefs"][0]["name"] = serde_json::json!("next"),
                "no-ref" => config["tls"]["validation"]["caCertificateRefs"] = serde_json::Value::Null,
                "no-ca" => config["resolvedCaCertificates"] = serde_json::Value::Null,
                "no-client" => config["resolvedClientCertificate"] = serde_json::Value::Null,
                "kind" => config["tls"]["validation"]["caCertificateRefs"][0]["kind"] = serde_json::json!("Service"),
                _ => {}
            }
            let candidate: edgion_resources::EdgionPlugins = serde_json::from_value(candidate).unwrap();
            let prepared = PreparedPluginReferences::from_plugins(&candidate);
            let binding = prepared.bind(&candidate).unwrap();
            let entry = binding.request_entries().next().unwrap();
            if matches!(case, "matching" | "configmap") {
                let proof = LdapAuth::validate_tls_entry(&entry, &defaults).unwrap().unwrap();
                let resolved = resolve_validated_outbound_tls(&proof, true, &defaults).unwrap();
                let EffectiveCa::Custom(bundles) = resolved.ca else {
                    panic!("expected actual CA")
                };
                assert_eq!(bundles, vec![pem.clone()]);
                let (actual_cert, actual_key) = resolved.client_identity.unwrap();
                assert_eq!(actual_cert, pem);
                assert_eq!(actual_key.as_slice(), key.as_slice());
                let auth = LdapAuth::new(&entry, &defaults).unwrap();
                assert_eq!(auth.ldap_host, "ldap.example.com");
                assert_eq!(auth.ldap_port, 389);
                assert_eq!(
                    transport_summary(&auth),
                    (
                        if start_tls { "starttls" } else { "ldaps" },
                        Some("ldap.example.com".to_string())
                    )
                );
            } else {
                assert_eq!(
                    LdapAuth::validate_tls_entry(&entry, &defaults).err().as_deref(),
                    Some("LdapAuth TLS configuration error: invalid owning TLS reference"),
                    "{case}"
                );
                assert_eq!(
                    LdapAuth::new(&entry, &defaults).err().as_deref(),
                    Some("LdapAuth TLS configuration error: invalid owning TLS reference"),
                    "{case}"
                );
            }
        }
    }
}

#[test]
fn ldap_captured_globals_and_plaintext_activation_are_process_isolated() {
    crate::test_subprocess::assert_in_subprocess(
        "plugins::http::ldap_auth::plugin::tests",
        "ldap_captured_globals_and_plaintext_activation_are_process_isolated",
        "EDGION_LDAP_CAPTURED_GLOBAL_CHILD",
        "captured",
        || {
            use edgion_common::link_sys_core::outbound_tls::{
                outbound_global_default, set_outbound_global_default, test_util::snapshot,
            };
            use edgion_resources::resources::common::OutboundTlsConfig;
            let previous = outbound_global_default();
            for start_tls in [false, true] {
                let (mut owner, pem, key) = ldap_tls_owner(start_tls);
                let EdgionPlugin::LdapAuth(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
                    panic!()
                };
                let mut global = OutboundTlsConfig {
                    verify: true,
                    validation: config.tls.validation.take(),
                    client_certificate_ref: config.tls.client_certificate_ref.take(),
                    resolved_ca_certificates: config.resolved_ca_certificates.take(),
                    resolved_client_certificate: config.resolved_client_certificate.take(),
                };
                global.client_certificate_ref.as_mut().unwrap().namespace = Some("default".into());
                for ca in global
                    .validation
                    .as_mut()
                    .unwrap()
                    .ca_certificate_refs
                    .as_mut()
                    .unwrap()
                {
                    ca.namespace = Some("default".into());
                }
                let captured = snapshot(global.clone());
                global.resolved_client_certificate.as_mut().unwrap().metadata.name = Some("wrong".into());
                let invalid = snapshot(global);
                set_outbound_global_default(invalid.clone());
                let prepared = PreparedPluginReferences::from_plugins(&owner);
                let binding = prepared.bind(&owner).unwrap();
                let entry = binding.request_entries().next().unwrap();
                let proof = LdapAuth::validate_tls_entry(&entry, &captured).unwrap().unwrap();
                let resolved = resolve_validated_outbound_tls(&proof, true, &captured).unwrap();
                let EffectiveCa::Custom(bundles) = resolved.ca else {
                    panic!("captured CA")
                };
                assert_eq!(bundles, vec![pem]);
                let (actual_cert, actual_key) = resolved.client_identity.unwrap();
                assert_eq!(actual_cert, bundles[0]);
                assert_eq!(actual_key.as_slice(), key.as_slice());
                assert!(LdapAuth::new(&entry, &captured).is_ok());
                set_outbound_global_default(captured);
                assert!(LdapAuth::new(&entry, &invalid).is_err());
                let (mut local, _, _) = ldap_tls_owner(start_tls);
                let prepared = PreparedPluginReferences::from_plugins(&local);
                let binding = prepared.bind(&local).unwrap();
                assert!(
                    LdapAuth::new(&binding.request_entries().next().unwrap(), &invalid).is_ok(),
                    "local axes override bad global"
                );
                let EdgionPlugin::LdapAuth(config) = &mut local.spec.request_plugins.as_mut().unwrap()[0].plugin else {
                    panic!()
                };
                config.ldaps = false;
                config.start_tls = false;
                config
                    .tls
                    .validation
                    .as_mut()
                    .unwrap()
                    .ca_certificate_refs
                    .as_mut()
                    .unwrap()[0]
                    .name = "bad/name".into();
                config.resolved_client_certificate.as_mut().unwrap().metadata.name = None;
                config.resolved_ca_certificates = Some(vec![secret_with(&[("ca.crt", b"invalid")])]);
                let prepared = PreparedPluginReferences::from_plugins(&local);
                let binding = prepared.bind(&local).unwrap();
                let entry = binding.request_entries().next().unwrap();
                assert!(LdapAuth::validate_tls_entry(&entry, &invalid).unwrap().is_none());
                let plain = LdapAuth::new(&entry, &invalid).unwrap();
                assert_eq!(transport_summary(&plain), ("plain", None));
                assert_eq!((plain.ldap_host.as_str(), plain.ldap_port), ("ldap.example.com", 389));
                local.metadata.name = None;
                let prepared = PreparedPluginReferences::from_plugins(&local);
                let binding = prepared.bind(&local).unwrap();
                assert!(LdapAuth::new(&binding.request_entries().next().unwrap(), &invalid).is_err());
            }
            set_outbound_global_default(previous);
        },
    );
}

use crate::plugins::runtime::traits::session::MockPluginSession;

async fn assert_prepared_challenge(challenge: &PreparedAuthChallenge, expected: &'static str) {
    let mut session = MockPluginSession::new();
    session
        .expect_write_response_header()
        .times(1)
        .returning(move |header, _| {
            assert_eq!(header.headers["www-authenticate"].as_bytes(), expected.as_bytes());
            Ok(())
        });
    session.expect_write_response_body().times(1).returning(|_, _| Ok(()));
    session.expect_shutdown().times(1).returning(|| {});
    send_gateway_auth_error_response(&mut session, 401, challenge, None, "Denied")
        .await
        .unwrap();
}

// ── Bounded response parsing (auth-01 / auth-02) over an in-memory peer ──

const BIND_DN: &str = "uid=u,dc=example,dc=com";

fn bind_response(rc: u8) -> Vec<u8> {
    vec![
        0x30, 0x0c, 0x02, 0x01, 0x01, 0x61, 0x07, 0x0a, 0x01, rc, 0x04, 0x00, 0x04, 0x00,
    ]
}

/// `depth` nested `30 83 L2 L1 L0` headers around `02 01 01`, built iteratively.
fn nested_sequence_flood(depth: usize) -> Vec<u8> {
    let mut inner = vec![0x02, 0x01, 0x01];
    for _ in 0..depth {
        let len = inner.len();
        let mut outer = Vec::with_capacity(len + 5);
        outer.extend_from_slice(&[0x30, 0x83, (len >> 16) as u8, (len >> 8) as u8, len as u8]);
        outer.extend_from_slice(&inner);
        inner = outer;
    }
    inner
}

/// Read one LDAP request frame from the peer half.
async fn read_request(peer: &mut tokio::io::DuplexStream) -> std::io::Result<()> {
    use edgion_ldap::codec::{frame_content_len, length_octets};
    use tokio::io::AsyncReadExt;
    let mut header = [0u8; 2];
    peer.read_exact(&mut header).await?;
    let extra = length_octets(header[0], header[1]).map_err(std::io::Error::other)?;
    let mut octets = vec![0u8; extra];
    peer.read_exact(&mut octets).await?;
    let len = frame_content_len(header[1], &octets).map_err(std::io::Error::other)?;
    let mut body = vec![0u8; len];
    peer.read_exact(&mut body).await.map(|_| ())
}

/// Install an in-memory peer that reads the bind request, writes `reply`
/// (ignoring write errors: the client may hang up mid-flood), then holds its
/// half until the client side closes.
fn scripted_peer(auth: &LdapAuth, capacity: usize, reply: Vec<u8>) -> tokio::task::JoinHandle<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (client, mut peer) = tokio::io::duplex(capacity);
    auth.install_scripted_stream(client);
    tokio::spawn(async move {
        if read_request(&mut peer).await.is_err() {
            return;
        }
        let _ = peer.write_all(&reply).await;
        let mut sink = [0u8; 64];
        while matches!(peer.read(&mut sink).await, Ok(n) if n > 0) {}
    })
}

#[tokio::test]
async fn malformed_bind_responses_fail_closed() {
    let replies = [
        vec![0x30, 0x05, 0x02, 0x01, 0x01, 0x61, 0x00],
        vec![
            0x30, 0x0d, 0x02, 0x01, 0x01, 0x61, 0x08, 0x0a, 0x01, 0x00, 0x04, 0x01, 0xff, 0x04, 0x00,
        ],
        vec![0x30, 0x05, 0x02, 0x01, 0x01, 0x41, 0x00],
        vec![0x30, 0x00],
        nested_sequence_flood(64),
        nested_sequence_flood(200_000),
    ];
    for reply in replies {
        let prefix = reply[..reply.len().min(8)].to_vec();
        let auth = ldap_auth_default();
        let peer = scripted_peer(&auth, 64 * 1024, reply);
        let result = auth.ldap_bind(BIND_DN, "p").await;
        assert!(
            matches!(result, Err(LdapAuthError::BindFailed)),
            "{prefix:02x?}: {result:?}"
        );
        drop(auth);
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn scripted_rc_49_is_invalid_credentials() {
    let auth = ldap_auth_default();
    let peer = scripted_peer(&auth, 4096, bind_response(49));
    assert!(matches!(
        auth.ldap_bind(BIND_DN, "p").await,
        Err(LdapAuthError::InvalidCredentials)
    ));
    peer.await.unwrap();
}

#[tokio::test]
async fn scripted_rc_0_is_ok() {
    let auth = ldap_auth_default();
    let peer = scripted_peer(&auth, 4096, bind_response(0));
    assert!(auth.ldap_bind(BIND_DN, "p").await.is_ok());
    peer.await.unwrap();
}

#[tokio::test]
async fn malformed_bind_response_run_request_returns_503_with_bind_fail_tag() {
    use crate::plugins::runtime::traits::session::MockPluginSession;

    let auth = ldap_auth_default();
    let peer = scripted_peer(&auth, 4096, vec![0x30, 0x05, 0x02, 0x01, 0x01, 0x41, 0x00]);
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("authorization", "ldap dTpw")]);
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "ldap-unreachable")
        .times(1)
        .return_const(());
    session
        .expect_write_response_header()
        .withf(|header, _| header.status.as_u16() == 503)
        .times(1)
        .returning(|_, _| Ok(()));
    session.expect_write_response_body().times(1).returning(|body, end| {
        assert_eq!(body.as_deref(), Some(&b"503 An unexpected error occurred"[..]));
        assert!(end);
        Ok(())
    });
    session.expect_shutdown().times(1).returning(|| {});

    let mut log = PluginLog::new(PLUGIN_NAME);
    assert_eq!(
        auth.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains("ldap bind fail"), "{}", log.render_for_test());
    drop(auth);
    peer.await.unwrap();
}

#[tokio::test]
async fn invalid_tls_server_name_fails_per_request_without_dial() {
    install_test_crypto_provider();
    let (owner, _, _) = ldap_tls_owner(false);
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let defaults = edgion_common::link_sys_core::outbound_tls::test_util::snapshot(Default::default());
    let mut auth = LdapAuth::new(&entry, &defaults).expect("LDAPS fixture must build");
    let LdapTransport::Ldaps { server_name, .. } = &mut auth.transport else {
        panic!("expected LDAPS transport");
    };
    *server_name = None;
    // The dial future records whether it was polled; the missing server name
    // must fail before phase 1 dials (no DNS lookup, no connect).
    let dialed = std::sync::atomic::AtomicBool::new(false);
    let dial = async {
        dialed.store(true, std::sync::atomic::Ordering::SeqCst);
        Err::<tokio::io::DuplexStream, _>(LdapAuthError::Timeout)
    };
    assert!(matches!(
        auth.bind_over(dial, BIND_DN, "p").await,
        Err(LdapAuthError::ConnectionFailed)
    ));
    assert!(!dialed.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn unparsable_tls_host_still_builds_without_server_name() {
    install_test_crypto_provider();
    let auth = actual_ldap(&LdapAuthConfig {
        ldaps: true,
        ldap_host: "ldap host.example.com".to_string(),
        ..test_config()
    })
    .expect("an unparsable TLS host must not fail plugin construction");
    assert_eq!(transport_summary(&auth), ("ldaps", None));
}

#[tokio::test]
async fn peer_close_before_reply_is_bind_failed() {
    let auth = ldap_auth_default();
    let (client, mut peer) = tokio::io::duplex(4096);
    auth.install_scripted_stream(client);
    let task = tokio::spawn(async move {
        let _ = read_request(&mut peer).await;
        drop(peer);
    });
    assert!(matches!(
        auth.ldap_bind(BIND_DN, "p").await,
        Err(LdapAuthError::BindFailed)
    ));
    task.await.unwrap();
}

#[tokio::test]
async fn extra_frame_and_close_after_success_is_ok() {
    let auth = ldap_auth_default();
    let (client, mut peer) = tokio::io::duplex(4096);
    auth.install_scripted_stream(client);
    let task = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        read_request(&mut peer).await.unwrap();
        let mut reply = bind_response(0);
        // A further frame the client never reads (it reads exactly one
        // response per request), then hang up: the unbind write may fail and
        // must not change the successful outcome.
        reply.extend_from_slice(&[
            0x30, 0x0c, 0x02, 0x01, 0x00, 0x78, 0x07, 0x0a, 0x01, 0x34, 0x04, 0x00, 0x04, 0x00,
        ]);
        peer.write_all(&reply).await.unwrap();
        drop(peer);
    });
    assert!(auth.ldap_bind(BIND_DN, "p").await.is_ok());
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn phase2_deadline_bounds_unbind() {
    let auth = actual_ldap(&LdapAuthConfig {
        timeout: 1000,
        ..test_config()
    })
    .unwrap();
    // Capacity 4 cannot hold the 7-byte UnbindRequest, and the peer never
    // reads after replying, so the unbind write blocks until the deadline.
    let (client, mut peer) = tokio::io::duplex(4);
    auth.install_scripted_stream(client);
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        read_request(&mut peer).await.unwrap();
        // The bind consumes 600 ms of the shared 1000 ms phase-2 deadline.
        tokio::time::sleep(Duration::from_millis(600)).await;
        peer.write_all(&bind_response(0)).await.unwrap();
        let _ = released.await;
        drop(peer);
    });
    let started = tokio::time::Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(10), auth.ldap_bind(BIND_DN, "p")).await;
    assert!(matches!(outcome, Ok(Ok(()))), "{outcome:?}");
    // The unbind is cut off by the deadline shared with the bind (1000 ms
    // after phase 2 starts), not by a fresh budget (which would end at 1600 ms).
    assert!(started.elapsed() >= Duration::from_millis(1000));
    assert!(
        started.elapsed() < Duration::from_millis(1600),
        "{:?}",
        started.elapsed()
    );
    let _ = release.send(());
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn phase1_timeout_when_dial_never_completes() {
    install_test_crypto_provider();
    let plain = actual_ldap(&LdapAuthConfig {
        timeout: 1000,
        ..test_config()
    })
    .unwrap();
    let defaults = edgion_common::link_sys_core::outbound_tls::test_util::snapshot(Default::default());
    let mut cases = vec![plain];
    for start_tls in [false, true] {
        let (mut owner, _, _) = ldap_tls_owner(start_tls);
        if let EdgionPlugin::LdapAuth(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin {
            config.timeout = 1000;
        }
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        cases.push(LdapAuth::new(&entry, &defaults).expect("TLS fixture must build"));
    }
    for auth in &cases {
        let dial = std::future::pending::<Result<tokio::io::DuplexStream, LdapAuthError>>();
        let outcome = tokio::time::timeout(Duration::from_secs(10), auth.bind_over(dial, BIND_DN, "p")).await;
        assert!(
            matches!(outcome, Ok(Err(LdapAuthError::Timeout))),
            "{}: {outcome:?}",
            transport_summary(auth).0
        );
    }
}

#[tokio::test(start_paused = true)]
async fn phase2_timeout_when_peer_never_replies() {
    let auth = actual_ldap(&LdapAuthConfig {
        timeout: 1000,
        ..test_config()
    })
    .unwrap();
    let (client, mut peer) = tokio::io::duplex(4096);
    auth.install_scripted_stream(client);
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        read_request(&mut peer).await.unwrap();
        let _ = released.await;
        drop(peer);
    });
    let outcome = tokio::time::timeout(Duration::from_secs(10), auth.ldap_bind(BIND_DN, "p")).await;
    assert!(matches!(outcome, Ok(Err(LdapAuthError::Timeout))), "{outcome:?}");
    let _ = release.send(());
    task.await.unwrap();
}

#[tokio::test]
async fn start_tls_rejection_is_connection_failed() {
    install_test_crypto_provider();
    let (owner, _, _) = ldap_tls_owner(true);
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let defaults = edgion_common::link_sys_core::outbound_tls::test_util::snapshot(Default::default());
    let auth = LdapAuth::new(&entry, &defaults).expect("StartTLS fixture must build");
    assert_eq!(transport_summary(&auth).0, "starttls");
    // ExtendedResponse for message 1 with resultCode 2 (protocolError).
    let rejected = vec![
        0x30, 0x0c, 0x02, 0x01, 0x01, 0x78, 0x07, 0x0a, 0x01, 0x02, 0x04, 0x00, 0x04, 0x00,
    ];
    let peer = scripted_peer(&auth, 4096, rejected);
    assert!(matches!(
        auth.ldap_bind(BIND_DN, "p").await,
        Err(LdapAuthError::ConnectionFailed)
    ));
    drop(auth);
    peer.await.unwrap();
}

#[tokio::test]
async fn peer_close_before_request_is_bind_failed() {
    let auth = ldap_auth_default();
    let (client, peer) = tokio::io::duplex(4096);
    drop(peer);
    auth.install_scripted_stream(client);
    assert!(matches!(
        auth.ldap_bind(BIND_DN, "p").await,
        Err(LdapAuthError::BindFailed)
    ));
}
