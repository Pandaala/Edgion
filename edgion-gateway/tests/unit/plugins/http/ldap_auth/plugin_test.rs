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

/// In-memory connector: `refused` addresses fail like a refused TCP connect,
/// any other address "connects" and returns it.
fn fake_connect(
    refused: &'static [std::net::SocketAddr],
) -> impl FnMut(std::net::SocketAddr) -> std::future::Ready<std::io::Result<std::net::SocketAddr>> {
    move |addr| {
        std::future::ready(if refused.contains(&addr) {
            Err(std::io::ErrorKind::ConnectionRefused.into())
        } else {
            Ok(addr)
        })
    }
}

const REFUSED: std::net::SocketAddr =
    std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1)), 389);
const ADMITTED: std::net::SocketAddr =
    std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 2)), 389);

#[tokio::test]
async fn dial_skips_refused_candidate_and_uses_next() {
    let connected = dial_admitted_addrs(vec![REFUSED, ADMITTED], fake_connect(&[REFUSED]))
        .await
        .expect("second admitted address must be dialed");
    assert_eq!(connected, ADMITTED);
}

#[tokio::test]
async fn all_refused_candidates_fail_closed() {
    let err = dial_admitted_addrs(vec![REFUSED], fake_connect(&[REFUSED]))
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
        let result = auth
            .ldap_bind(Exchange::Bind {
                dn: BIND_DN,
                password: "p",
            })
            .await;
        assert!(
            matches!(result, Err(LdapAuthError::BindFailed(BindFailure::Protocol))),
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
        auth.ldap_bind(Exchange::Bind {
            dn: BIND_DN,
            password: "p"
        })
        .await,
        Err(LdapAuthError::InvalidCredentials)
    ));
    peer.await.unwrap();
}

#[tokio::test]
async fn scripted_rc_0_is_ok() {
    let auth = ldap_auth_default();
    let peer = scripted_peer(&auth, 4096, bind_response(0));
    assert!(auth
        .ldap_bind(Exchange::Bind {
            dn: BIND_DN,
            password: "p"
        })
        .await
        .is_ok());
    peer.await.unwrap();
}

#[tokio::test]
async fn malformed_bind_response_run_request_returns_503_with_protocol_tag() {
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
    assert!(log.contains("ldap bind protocol error"), "{}", log.render_for_test());
    assert!(!log.contains("ldap bind fail"), "{}", log.render_for_test());
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
        auth.bind_over(
            dial,
            Exchange::Bind {
                dn: BIND_DN,
                password: "p"
            }
        )
        .await,
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
        auth.ldap_bind(Exchange::Bind {
            dn: BIND_DN,
            password: "p"
        })
        .await,
        Err(LdapAuthError::BindFailed(BindFailure::Transport))
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
    assert!(auth
        .ldap_bind(Exchange::Bind {
            dn: BIND_DN,
            password: "p"
        })
        .await
        .is_ok());
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
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        auth.ldap_bind(Exchange::Bind {
            dn: BIND_DN,
            password: "p",
        }),
    )
    .await;
    assert!(matches!(outcome, Ok(Ok(None))), "{outcome:?}");
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
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            auth.bind_over(
                dial,
                Exchange::Bind {
                    dn: BIND_DN,
                    password: "p",
                },
            ),
        )
        .await;
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
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        auth.ldap_bind(Exchange::Bind {
            dn: BIND_DN,
            password: "p",
        }),
    )
    .await;
    assert!(matches!(outcome, Ok(Err(LdapAuthError::Timeout))), "{outcome:?}");
    let _ = release.send(());
    task.await.unwrap();
}

#[tokio::test]
async fn start_tls_rejection_is_reported_as_rejected() {
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
        auth.ldap_bind(Exchange::Bind {
            dn: BIND_DN,
            password: "p"
        })
        .await,
        Err(LdapAuthError::StartTlsRejected)
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
        auth.ldap_bind(Exchange::Bind {
            dn: BIND_DN,
            password: "p"
        })
        .await,
        Err(LdapAuthError::BindFailed(BindFailure::Transport))
    ));
}

// ── Identity-header hardening (all modes) ──────────────────────────────

fn expect_401(session: &mut crate::plugins::runtime::traits::session::MockPluginSession) {
    session
        .expect_write_response_header()
        .withf(|header, _| header.status.as_u16() == 401)
        .times(1)
        .returning(|_, _| Ok(()));
    session.expect_write_response_body().times(1).returning(|_, _| Ok(()));
    session.expect_shutdown().times(1).returning(|| {});
}

fn ldap_header(user_and_password: &str) -> String {
    format!("ldap {}", general_purpose::STANDARD.encode(user_and_password))
}

#[tokio::test]
async fn spoofed_identity_with_control_char_username_is_401() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    for payload in [
        "alice\u{1}:pw",
        "al\tice:pw",
        "alice\u{0}x:pw",
        "al\u{7f}ice:pw",
        "al\u{85}ice:pw",
    ] {
        let auth = ldap_auth_default();
        // An in-memory peer instead of a dial: a regression fails here, offline.
        let peer = scripted_conversation(&auth, Vec::new());
        let header = ldap_header(payload);
        let mut session = MockPluginSession::new();
        setup_header_mocks(
            &mut session,
            &[("authorization", header.as_str()), ("x-credential-identifier", "admin")],
        );
        // No set_request_header_value expectation: any identity write panics the mock.
        expect_401(&mut session);
        let mut log = PluginLog::new(PLUGIN_NAME);
        assert_eq!(
            auth.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest,
            "{payload:?}"
        );
        assert!(
            log.contains("Deny bad username"),
            "{payload:?}: {}",
            log.render_for_test()
        );
        drop(auth);
        assert!(peer.await.unwrap().is_empty(), "{payload:?}: no LDAP request");
    }
}

#[tokio::test]
async fn identity_header_write_failure_denies() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    let auth = ldap_auth_default();
    let peer = scripted_peer(&auth, 4096, bind_response(0));
    let header = ldap_header("bob:pw");
    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[("authorization", header.as_str()), ("x-credential-identifier", "admin")],
    );
    session
        .expect_set_request_header_value()
        .times(1)
        .returning(|_, _| Err("header store rejected the value".into()));
    session
        .expect_record_error()
        .withf(|code| *code == edgion_resources::EdgionErrorCode::AuthHeaderMutationRejected)
        .times(1)
        .return_const(());
    expect_401(&mut session);
    let mut log = PluginLog::new(PLUGIN_NAME);
    assert_eq!(
        auth.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains("identity header rejected"), "{}", log.render_for_test());
    assert!(!log.contains("OK user"));
    drop(auth);
    peer.await.unwrap();
}

#[tokio::test]
async fn anonymous_header_write_failure_denies() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    let auth = actual_ldap(&LdapAuthConfig {
        anonymous: Some("guest".to_string()),
        ..test_config()
    })
    .unwrap();
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("x-credential-identifier", "admin")]);
    session
        .expect_set_request_header_value()
        .times(1)
        .returning(|_, _| Err("header store rejected the value".into()));
    session
        .expect_record_error()
        .withf(|code| *code == edgion_resources::EdgionErrorCode::AuthHeaderMutationRejected)
        .times(1)
        .return_const(());
    expect_401(&mut session);
    let mut log = PluginLog::new(PLUGIN_NAME);
    assert_eq!(
        auth.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains("identity header rejected"));
    assert!(!log.contains("Allow anonymous"));
}

#[tokio::test]
async fn successful_bind_overwrites_spoofed_identity() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    let auth = ldap_auth_default();
    let peer = scripted_peer(&auth, 4096, bind_response(0));
    let header = ldap_header("bob:pw");
    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[("authorization", header.as_str()), ("x-credential-identifier", "admin")],
    );
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Credential-Identifier" && value == "bob")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_remove_request_header()
        .withf(|name| name == "X-Anonymous-Consumer")
        .times(1)
        .returning(|_| Ok(()));
    let mut log = PluginLog::new(PLUGIN_NAME);
    assert_eq!(
        auth.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    drop(auth);
    peer.await.unwrap();
}

#[tokio::test]
async fn hidden_credential_removal_failure_denies() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    let auth = actual_ldap(&LdapAuthConfig {
        hide_credentials: true,
        ..test_config()
    })
    .unwrap();
    let peer = scripted_peer(&auth, 4096, bind_response(0));
    let header = ldap_header("bob:pw");
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("authorization", header.as_str())]);
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Credential-Identifier" && value == "bob")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_remove_request_header()
        .withf(|name| name == "X-Anonymous-Consumer")
        .times(1)
        .returning(|_| Ok(()));
    session
        .expect_remove_request_header()
        .withf(|name| name == "authorization")
        .times(1)
        .returning(|_| Err("header store rejected the removal".into()));
    session
        .expect_record_error()
        .withf(|code| *code == edgion_resources::EdgionErrorCode::AuthHeaderMutationRejected)
        .times(1)
        .return_const(());
    expect_401(&mut session);
    let mut log = PluginLog::new(PLUGIN_NAME);
    assert_eq!(
        auth.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains("credential removal failed"), "{}", log.render_for_test());
    assert!(!log.contains("OK user"), "{}", log.render_for_test());
    drop(auth);
    peer.await.unwrap();
}

// ── Search mode ─────────────────────────────────────────────────────────

const SERVICE_DN: &str = "cn=svc,dc=example,dc=com";

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    assert!(content.len() < 0x80);
    let mut out = vec![tag, content.len() as u8];
    out.extend_from_slice(content);
    out
}

fn bind_reply(id: u8, rc: u8) -> Vec<u8> {
    tlv(
        0x30,
        &[tlv(0x02, &[id]), tlv(0x61, &[0x0a, 0x01, rc, 0x04, 0x00, 0x04, 0x00])].concat(),
    )
}

fn entry_reply(id: u8, dn: &[u8], uid: Option<&[u8]>) -> Vec<u8> {
    let attributes = uid
        .map(|value| tlv(0x30, &[tlv(0x04, b"uid"), tlv(0x31, &tlv(0x04, value))].concat()))
        .unwrap_or_default();
    tlv(
        0x30,
        &[
            tlv(0x02, &[id]),
            tlv(0x64, &[tlv(0x04, dn), tlv(0x30, &attributes)].concat()),
        ]
        .concat(),
    )
}

fn entry_with_uids(id: u8, dn: &[u8], uids: &[&[u8]]) -> Vec<u8> {
    let values: Vec<u8> = uids.iter().flat_map(|value| tlv(0x04, value)).collect();
    let attributes = tlv(0x30, &[tlv(0x04, b"uid"), tlv(0x31, &values)].concat());
    tlv(
        0x30,
        &[
            tlv(0x02, &[id]),
            tlv(0x64, &[tlv(0x04, dn), tlv(0x30, &attributes)].concat()),
        ]
        .concat(),
    )
}

fn reference_reply(id: u8) -> Vec<u8> {
    tlv(0x30, &[tlv(0x02, &[id]), tlv(0x73, &tlv(0x04, b"ldap://x/"))].concat())
}

fn done_reply(id: u8, rc: u8) -> Vec<u8> {
    tlv(
        0x30,
        &[tlv(0x02, &[id]), tlv(0x65, &[0x0a, 0x01, rc, 0x04, 0x00, 0x04, 0x00])].concat(),
    )
}

fn search_config() -> LdapAuthConfig {
    LdapAuthConfig {
        ldaps: true,
        bind_dn: Some(SERVICE_DN.to_string()),
        bind_password_ref: Some(edgion_resources::resources::gateway::SecretObjectReference {
            group: None,
            kind: None,
            name: "ldap-svc".to_string(),
            namespace: None,
        }),
        resolved_bind_password: Some("svc-pw".to_string()),
        search_base: Some("dc=example,dc=com".to_string()),
        search_filter: Some("(&(objectClass=person)(uid={username}))".to_string()),
        ..test_config()
    }
}

/// A search-mode plugin whose transport is switched to plain so the scripted
/// in-memory peer can answer without a TLS handshake.
fn search_auth(config: LdapAuthConfig) -> LdapAuth {
    install_test_crypto_provider();
    let mut auth = actual_ldap(&config).expect("search-mode plugin must build");
    assert!(matches!(auth.bind_dn_plan, BindDnPlan::Search(_)));
    auth.transport = LdapTransport::Plain;
    auth
}

/// Peer that answers each request in turn with the next reply and records
/// every request frame (including a trailing unbind, if any).
fn scripted_conversation(auth: &LdapAuth, replies: Vec<Vec<u8>>) -> tokio::task::JoinHandle<Vec<Vec<u8>>> {
    use edgion_ldap::codec::{frame_content_len, length_octets};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (client, mut peer) = tokio::io::duplex(64 * 1024);
    auth.install_scripted_stream(client);
    tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut replies = replies.into_iter();
        loop {
            let mut header = [0u8; 2];
            if peer.read_exact(&mut header).await.is_err() {
                return requests;
            }
            let Ok(extra) = length_octets(header[0], header[1]) else {
                return requests;
            };
            let mut octets = vec![0u8; extra];
            if peer.read_exact(&mut octets).await.is_err() {
                return requests;
            }
            let Ok(len) = frame_content_len(header[1], &octets) else {
                return requests;
            };
            let mut body = vec![0u8; len];
            if peer.read_exact(&mut body).await.is_err() {
                return requests;
            }
            requests.push([header.to_vec(), octets, body].concat());
            if let Some(reply) = replies.next() {
                let _ = peer.write_all(&reply).await;
            }
        }
    })
}

fn search_plan(auth: &LdapAuth) -> &SearchPlan {
    match &auth.bind_dn_plan {
        BindDnPlan::Search(plan) => plan,
        _ => panic!("search plan expected"),
    }
}

async fn search_exchange(auth: &LdapAuth, username: &str, password: &str) -> Result<Option<String>, LdapAuthError> {
    auth.ldap_bind(Exchange::Search {
        plan: search_plan(auth),
        service_password: "svc-pw",
        username,
        password,
    })
    .await
}

fn user_dn() -> &'static [u8] {
    b"uid=bob,ou=staff,dc=example,dc=com"
}

#[tokio::test]
async fn search_mode_success_returns_canonical_identity_and_binds_found_dn() {
    let auth = search_auth(search_config());
    let peer = scripted_conversation(
        &auth,
        vec![
            bind_reply(1, 0),
            [entry_reply(2, user_dn(), Some(b"bob")), done_reply(2, 0)].concat(),
            bind_reply(3, 0),
        ],
    );
    assert_eq!(
        search_exchange(&auth, "BOB", "pw").await.unwrap().as_deref(),
        Some("bob")
    );
    drop(auth);
    let requests = peer.await.unwrap();
    assert_eq!(requests.len(), 4, "service bind, search, user bind, unbind");
    assert!(requests[0]
        .windows(SERVICE_DN.len())
        .any(|w| w == SERVICE_DN.as_bytes()));
    assert!(requests[0].windows(6).any(|w| w == b"svc-pw"));
    assert_eq!(requests[1][5], 0x63, "SearchRequest");
    assert!(requests[1].windows(3).any(|w| w == b"BOB"));
    assert!(requests[2].windows(user_dn().len()).any(|w| w == user_dn()));
    assert_eq!(requests[3][5], 0x42, "UnbindRequest");
}

#[tokio::test]
async fn search_mode_outcome_mapping() {
    type Expect = fn(&Result<Option<String>, LdapAuthError>) -> bool;
    let invalid: Expect = |r| matches!(r, Err(LdapAuthError::InvalidCredentials));
    let search_failed: Expect = |r| matches!(r, Err(LdapAuthError::SearchFailed));
    let cases: Vec<(&str, Vec<Vec<u8>>, Expect)> = vec![
        ("not found", vec![bind_reply(1, 0), done_reply(2, 0)], invalid),
        (
            "two entries",
            vec![
                bind_reply(1, 0),
                [
                    entry_reply(2, b"uid=a,dc=example,dc=com", Some(b"a")),
                    entry_reply(2, b"uid=b,dc=example,dc=com", Some(b"b")),
                ]
                .concat(),
            ],
            invalid,
        ),
        (
            "size limit",
            vec![
                bind_reply(1, 0),
                [entry_reply(2, user_dn(), Some(b"bob")), done_reply(2, 4)].concat(),
            ],
            invalid,
        ),
        (
            "no identity value",
            vec![
                bind_reply(1, 0),
                [entry_reply(2, user_dn(), None), done_reply(2, 0)].concat(),
            ],
            invalid,
        ),
        (
            "user wrong password",
            vec![
                bind_reply(1, 0),
                [entry_reply(2, user_dn(), Some(b"bob")), done_reply(2, 0)].concat(),
                bind_reply(3, 49),
            ],
            invalid,
        ),
        (
            "user bind other rc",
            vec![
                bind_reply(1, 0),
                [entry_reply(2, user_dn(), Some(b"bob")), done_reply(2, 0)].concat(),
                bind_reply(3, 53),
            ],
            |r| matches!(r, Err(LdapAuthError::BindFailed(BindFailure::Result))),
        ),
        (
            "user bind protocol error",
            vec![
                bind_reply(1, 0),
                [entry_reply(2, user_dn(), Some(b"bob")), done_reply(2, 0)].concat(),
                bind_reply(9, 0),
            ],
            |r| matches!(r, Err(LdapAuthError::BindFailed(BindFailure::Protocol))),
        ),
        ("service password rejected", vec![bind_reply(1, 49)], |r| {
            matches!(r, Err(LdapAuthError::ServiceBindRejected))
        }),
        ("service bind other rc", vec![bind_reply(1, 1)], search_failed),
        (
            "referral result",
            vec![bind_reply(1, 0), done_reply(2, 10)],
            search_failed,
        ),
        (
            "no such object",
            vec![bind_reply(1, 0), done_reply(2, 32)],
            search_failed,
        ),
        (
            "unparsable returned dn",
            vec![
                bind_reply(1, 0),
                [entry_reply(2, b"svc@example.com", Some(b"bob")), done_reply(2, 0)].concat(),
            ],
            invalid,
        ),
        (
            "empty returned dn",
            vec![
                bind_reply(1, 0),
                [entry_reply(2, b"", Some(b"bob")), done_reply(2, 0)].concat(),
            ],
            invalid,
        ),
        (
            "identity with surrounding space",
            vec![
                bind_reply(1, 0),
                [entry_reply(2, user_dn(), Some(b" admin")), done_reply(2, 0)].concat(),
            ],
            invalid,
        ),
        (
            "multi-valued identity",
            vec![
                bind_reply(1, 0),
                [entry_with_uids(2, user_dn(), &[b"a", b"b"]), done_reply(2, 0)].concat(),
            ],
            invalid,
        ),
        (
            "too many references",
            vec![
                bind_reply(1, 0),
                (0..=edgion_ldap::client::MAX_SEARCH_REFERENCES)
                    .flat_map(|_| reference_reply(2))
                    .collect(),
            ],
            search_failed,
        ),
    ];
    for (name, replies, expect) in cases {
        let auth = search_auth(search_config());
        let peer = scripted_conversation(&auth, replies);
        let result = search_exchange(&auth, "bob", "pw").await;
        assert!(expect(&result), "{name}: {result:?}");
        drop(auth);
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn found_service_dn_spellings_are_all_refused() {
    for spelling in [
        "2.5.4.3=svc,0.9.2342.19200300.100.1.25=example,dc=com",
        "commonName=SVC,domainComponent=example,DC=com",
        "cn=#0403737663,dc=example,dc=com",
        "cn= svc ;dc=example;dc=com",
    ] {
        let auth = search_auth(search_config());
        let peer = scripted_conversation(
            &auth,
            vec![
                bind_reply(1, 0),
                [entry_reply(2, spelling.as_bytes(), Some(b"svc")), done_reply(2, 0)].concat(),
            ],
        );
        let result = search_exchange(&auth, "svc", "guess").await;
        assert!(matches!(result, Err(LdapAuthError::InvalidCredentials)), "{spelling}");
        drop(auth);
        let requests = peer.await.unwrap();
        assert_eq!(requests.len(), 3, "{spelling}: no user bind");
    }
}

#[tokio::test]
async fn found_service_dn_is_uniform_401_without_user_bind() {
    let auth = search_auth(search_config());
    let peer = scripted_conversation(
        &auth,
        vec![
            bind_reply(1, 0),
            [
                entry_reply(2, b"CN=svc, DC=example,DC=com", Some(b"svc")),
                done_reply(2, 0),
            ]
            .concat(),
        ],
    );
    assert!(matches!(
        search_exchange(&auth, "svc", "guess").await,
        Err(LdapAuthError::InvalidCredentials)
    ));
    drop(auth);
    let requests = peer.await.unwrap();
    // Service bind, search, then straight to unbind: no bind as the service DN.
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2][5], 0x42);
}

#[tokio::test]
async fn search_mode_injection_username_is_sent_as_raw_assertion_bytes() {
    let auth = search_auth(search_config());
    let peer = scripted_conversation(&auth, vec![bind_reply(1, 0), done_reply(2, 0)]);
    let hostile = "*)(uid=*";
    assert!(matches!(
        search_exchange(&auth, hostile, "pw").await,
        Err(LdapAuthError::InvalidCredentials)
    ));
    drop(auth);
    let requests = peer.await.unwrap();
    // The filter keeps its compiled shape: an AND of two equality items whose
    // second assertion value is exactly the hostile username.
    let filter = edgion_ldap::filter::SearchFilter::compile("(&(objectClass=person)(uid={username}))").unwrap();
    let mut expected = Vec::new();
    filter.encode(hostile.as_bytes(), &mut expected);
    assert!(requests[1].windows(expected.len()).any(|w| w == expected.as_slice()));
}

#[tokio::test(start_paused = true)]
async fn search_mode_phase2_timeout() {
    let auth = search_auth(LdapAuthConfig {
        timeout: 1000,
        ..search_config()
    });
    // The peer answers the service bind but never the search.
    let peer = scripted_conversation(&auth, vec![bind_reply(1, 0)]);
    let outcome = tokio::time::timeout(Duration::from_secs(10), search_exchange(&auth, "bob", "pw")).await;
    assert!(matches!(outcome, Ok(Err(LdapAuthError::Timeout))), "{outcome:?}");
    drop(auth);
    peer.abort();
}

async fn run_search_request(
    auth: &LdapAuth,
    header: &str,
    status: u16,
    configure: impl FnOnce(&mut crate::plugins::runtime::traits::session::MockPluginSession),
) -> (PluginRunningResult, PluginLog) {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[("authorization", header), ("x-credential-identifier", "admin")],
    );
    configure(&mut session);
    if status != 200 {
        session
            .expect_write_response_header()
            .withf(move |h, _| h.status.as_u16() == status)
            .times(1)
            .returning(|_, _| Ok(()));
        session.expect_write_response_body().times(1).returning(|_, _| Ok(()));
        session.expect_shutdown().times(1).returning(|| {});
    }
    let mut log = PluginLog::new(PLUGIN_NAME);
    let result = auth.run_request(&mut session, &mut log).await;
    (result, log)
}

#[tokio::test]
async fn search_mode_run_request_forwards_canonical_identity() {
    let auth = search_auth(search_config());
    let peer = scripted_conversation(
        &auth,
        vec![
            bind_reply(1, 0),
            [entry_reply(2, user_dn(), Some(b"bob")), done_reply(2, 0)].concat(),
            bind_reply(3, 0),
        ],
    );
    let header = ldap_header("BOB:pw");
    let (result, _log) = run_search_request(&auth, &header, 200, |session| {
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "X-Credential-Identifier" && value == "bob")
            .times(1)
            .returning(|_, _| Ok(()));
        session.expect_remove_request_header().times(1).returning(|_| Ok(()));
    })
    .await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    drop(auth);
    peer.await.unwrap();
}

#[tokio::test]
async fn search_mode_run_request_failure_statuses_and_tags() {
    use metrics_util::debugging::DebuggingRecorder;
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);
    // `edgion_ldap_auth_errors_total` value for one `kind` label.
    let errors = |kind: &str| -> u64 {
        snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .filter(|(key, _, _, _)| {
                key.key().name() == crate::observe::metrics::registry::names::LDAP_AUTH_ERRORS_TOTAL
                    && key
                        .key()
                        .labels()
                        .any(|label| label.key() == "kind" && label.value() == kind)
            })
            .map(|(_, _, _, value)| match value {
                metrics_util::debugging::DebugValue::Counter(count) => count,
                _ => 0,
            })
            .sum()
    };

    // Wrong service password: generic 401 with a fixed tag.
    let auth = search_auth(search_config());
    let peer = scripted_conversation(&auth, vec![bind_reply(1, 49)]);
    let (result, log) = run_search_request(&auth, &ldap_header("bob:pw"), 401, |_| {}).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("ldap service bind fail"), "{}", log.render_for_test());
    assert_eq!(errors("service_bind_fail"), 1);
    drop(auth);
    peer.await.unwrap();

    // Search failure: 503 with the fail-close dye.
    let auth = search_auth(search_config());
    let peer = scripted_conversation(&auth, vec![bind_reply(1, 0), done_reply(2, 32)]);
    let (result, log) = run_search_request(&auth, &ldap_header("bob:pw"), 503, |session| {
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "ldap-unreachable")
            .times(1)
            .return_const(());
    })
    .await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("ldap search fail"), "{}", log.render_for_test());
    assert_eq!(errors("search_fail"), 1);
    drop(auth);
    peer.await.unwrap();

    // Unknown user: the uniform credential tag.
    let auth = search_auth(search_config());
    let peer = scripted_conversation(&auth, vec![bind_reply(1, 0), done_reply(2, 0)]);
    let (_, log) = run_search_request(&auth, &ldap_header("nobody:pw"), 401, |_| {}).await;
    assert!(log.contains("Deny bad credential"), "{}", log.render_for_test());
    drop(auth);
    peer.await.unwrap();
}

#[tokio::test]
async fn search_mode_pre_io_rejections() {
    // Unresolved service password: generic 401, no LDAP I/O.
    let auth = search_auth(LdapAuthConfig {
        resolved_bind_password: None,
        ..search_config()
    });
    let peer = scripted_conversation(&auth, Vec::new());
    let (result, log) = run_search_request(&auth, &ldap_header("bob:pw"), 401, |_| {}).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(
        log.contains("ldap service credential unavailable"),
        "{}",
        log.render_for_test()
    );
    drop(auth);
    assert!(peer.await.unwrap().is_empty(), "no LDAP request");

    // Over-long username: 401 before any I/O.
    let auth = search_auth(search_config());
    let peer = scripted_conversation(&auth, Vec::new());
    let long = format!("{}:pw", "u".repeat(MAX_SEARCH_USERNAME_LEN + 1));
    let (_, log) = run_search_request(&auth, &ldap_header(&long), 401, |_| {}).await;
    assert!(log.contains("Deny bad username"), "{}", log.render_for_test());
    drop(auth);
    assert!(peer.await.unwrap().is_empty(), "no LDAP request");

    // Exactly the cap still searches.
    let auth = search_auth(search_config());
    let peer = scripted_conversation(&auth, vec![bind_reply(1, 0), done_reply(2, 0)]);
    let at_cap = "u".repeat(MAX_SEARCH_USERNAME_LEN);
    let (_, log) = run_search_request(&auth, &ldap_header(&format!("{at_cap}:pw")), 401, |_| {}).await;
    assert!(log.contains("Deny bad credential"), "{}", log.render_for_test());
    drop(auth);
    let requests = peer.await.unwrap();
    // The long-form search request carries the full username.
    assert!(requests[1].windows(at_cap.len()).any(|w| w == at_cap.as_bytes()));
}

#[test]
fn constructor_runs_cold_validation() {
    install_test_crypto_provider();
    let plain_search = LdapAuthConfig {
        ldaps: false,
        start_tls: false,
        ..search_config()
    };
    let error = actual_ldap(&plain_search)
        .err()
        .expect("plaintext search mode must not build");
    assert!(error.contains("bindDn requires ldaps or startTls"), "{error}");
}

#[tokio::test]
async fn anonymous_overwrites_spoofed_identity() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    let auth = actual_ldap(&LdapAuthConfig {
        anonymous: Some("guest".to_string()),
        ..test_config()
    })
    .unwrap();
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("x-credential-identifier", "admin")]);
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Credential-Identifier" && value == "guest")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Anonymous-Consumer" && value == "true")
        .times(1)
        .returning(|_, _| Ok(()));
    let mut log = PluginLog::new(PLUGIN_NAME);
    assert_eq!(
        auth.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[test]
fn search_plan_time_limit_and_defaults() {
    let auth = search_auth(LdapAuthConfig {
        timeout: 1500,
        search_base: None,
        search_filter: None,
        ..search_config()
    });
    let plan = search_plan(&auth);
    assert_eq!(plan.time_limit_secs, 2);
    assert_eq!(plan.base, "dc=example,dc=com");
    assert_eq!(plan.attribute, "uid");
    assert_eq!(plan.bind_dn, SERVICE_DN);
    let huge = search_auth(LdapAuthConfig {
        timeout: u64::MAX,
        ..search_config()
    });
    assert_eq!(search_plan(&huge).time_limit_secs, i32::MAX as u32);
}

#[tokio::test(start_paused = true)]
async fn input_only_rejections_apply_the_failure_delay() {
    const DELAY_MS: u64 = 700;
    // Control character (bind mode) and over-long username (search mode).
    let control = actual_ldap(&LdapAuthConfig {
        auth_failure_delay_ms: DELAY_MS,
        ..test_config()
    })
    .unwrap();
    let search = search_auth(LdapAuthConfig {
        auth_failure_delay_ms: DELAY_MS,
        ..search_config()
    });
    let long = format!("{}:pw", "u".repeat(MAX_SEARCH_USERNAME_LEN + 1));
    for (auth, payload) in [(&control, "al\u{1}ice:pw".to_string()), (&search, long)] {
        let peer = scripted_conversation(auth, Vec::new());
        let started = tokio::time::Instant::now();
        let (result, log) = run_search_request(auth, &ldap_header(&payload), 401, |_| {}).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny bad username"), "{}", log.render_for_test());
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(DELAY_MS),
            "failure delay applied"
        );
        drop(peer);
    }
}

// ── Group requirements ──────────────────────────────────────────────────

const GROUP_DN: &str = "cn=gw-users,ou=groups,dc=example,dc=com";

fn group_search_config() -> LdapAuthConfig {
    LdapAuthConfig {
        required_groups_any_of: Some(vec![format!(" {GROUP_DN} ")]),
        ..search_config()
    }
}

/// Replies for service bind, user search (one entry), user bind rc `user_rc`.
fn found_then_user_bind(user_rc: u8) -> Vec<Vec<u8>> {
    vec![
        bind_reply(1, 0),
        [entry_reply(2, user_dn(), Some(b"bob")), done_reply(2, 0)].concat(),
        bind_reply(3, user_rc),
    ]
}

#[tokio::test]
async fn group_member_rebinds_as_service_and_checks_base_entry() {
    let auth = search_auth(group_search_config());
    let mut replies = found_then_user_bind(0);
    replies.push(bind_reply(4, 0));
    replies.push([entry_reply(5, user_dn(), None), done_reply(5, 0)].concat());
    let peer = scripted_conversation(&auth, replies);
    let result = search_exchange(&auth, "bob", "pw").await;
    assert_eq!(result.unwrap().as_deref(), Some("bob"));
    drop(auth);
    let requests = peer.await.unwrap();
    // service bind, search, user bind, service re-bind, group search, unbind
    assert_eq!(requests.len(), 6);
    assert!(requests[3]
        .windows(SERVICE_DN.len())
        .any(|w| w == SERVICE_DN.as_bytes()));
    assert!(requests[3].windows(6).any(|w| w == b"svc-pw"));
    assert_eq!(requests[4][5], 0x63, "SearchRequest");
    // Base = the found DN, baseObject scope, sizeLimit 1, attribute 1.1, trimmed group DN.
    let base = [&[0x04, user_dn().len() as u8][..], user_dn()].concat();
    let at = requests[4]
        .windows(base.len())
        .position(|w| w == base.as_slice())
        .unwrap()
        + base.len();
    assert_eq!(
        &requests[4][at..at + 9],
        &[0x0a, 0x01, 0x00, 0x0a, 0x01, 0x00, 0x02, 0x01, 0x01]
    );
    assert!(requests[4].windows(GROUP_DN.len()).any(|w| w == GROUP_DN.as_bytes()));
    assert!(requests[4].ends_with(&[0x30, 0x05, 0x04, 0x03, b'1', b'.', b'1']));
    assert_eq!(requests[5][5], 0x42, "UnbindRequest");
}

#[tokio::test]
async fn wrong_password_never_reaches_the_group_check() {
    let auth = search_auth(group_search_config());
    let peer = scripted_conversation(&auth, found_then_user_bind(49));
    assert!(matches!(
        search_exchange(&auth, "bob", "wrong").await,
        Err(LdapAuthError::InvalidCredentials)
    ));
    drop(auth);
    let requests = peer.await.unwrap();
    // service bind, search, user bind, unbind: no re-bind, no group search.
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[3][5], 0x42, "UnbindRequest");
}

#[tokio::test]
async fn group_check_outcome_mapping() {
    type Expect = fn(&Result<Option<String>, LdapAuthError>) -> bool;
    let not_member: Expect = |r| matches!(r, Err(LdapAuthError::NotInRequiredGroup));
    let search_failed: Expect = |r| matches!(r, Err(LdapAuthError::SearchFailed));
    let cases: Vec<(&str, Vec<Vec<u8>>, Expect)> = vec![
        ("no entry", vec![bind_reply(4, 0), done_reply(5, 0)], not_member),
        ("size limit", vec![bind_reply(4, 0), done_reply(5, 4)], not_member),
        ("re-bind rejected", vec![bind_reply(4, 49)], |r| {
            matches!(r, Err(LdapAuthError::ServiceBindRejected))
        }),
        ("re-bind other rc", vec![bind_reply(4, 1)], search_failed),
        (
            "re-bind protocol error (wrong message id)",
            vec![bind_reply(9, 0)],
            search_failed,
        ),
        (
            "second entry",
            vec![
                bind_reply(4, 0),
                [entry_reply(5, user_dn(), None), entry_reply(5, user_dn(), None)].concat(),
            ],
            not_member,
        ),
        (
            "group frame protocol error (wrong message id)",
            vec![bind_reply(4, 0), done_reply(9, 0)],
            search_failed,
        ),
        (
            "no such object",
            vec![bind_reply(4, 0), done_reply(5, 32)],
            search_failed,
        ),
        ("referral", vec![bind_reply(4, 0), done_reply(5, 10)], search_failed),
        (
            "too many references",
            vec![
                bind_reply(4, 0),
                (0..=edgion_ldap::client::MAX_SEARCH_REFERENCES)
                    .flat_map(|_| reference_reply(5))
                    .collect(),
            ],
            search_failed,
        ),
    ];
    for (name, tail, expect) in cases {
        let auth = search_auth(group_search_config());
        let mut replies = found_then_user_bind(0);
        replies.extend(tail);
        let peer = scripted_conversation(&auth, replies);
        let result = search_exchange(&auth, "bob", "pw").await;
        assert!(expect(&result), "{name}: {result:?}");
        drop(auth);
        peer.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn group_rebind_timeout() {
    let auth = search_auth(LdapAuthConfig {
        timeout: 1000,
        ..group_search_config()
    });
    // The peer never answers the service re-bind.
    let peer = scripted_conversation(&auth, found_then_user_bind(0));
    let outcome = tokio::time::timeout(Duration::from_secs(10), search_exchange(&auth, "bob", "pw")).await;
    assert!(matches!(outcome, Ok(Err(LdapAuthError::Timeout))), "{outcome:?}");
    drop(auth);
    peer.abort();
}

#[tokio::test(start_paused = true)]
async fn group_check_timeout() {
    let auth = search_auth(LdapAuthConfig {
        timeout: 1000,
        ..group_search_config()
    });
    // The peer answers the re-bind but never the group search.
    let mut replies = found_then_user_bind(0);
    replies.push(bind_reply(4, 0));
    let peer = scripted_conversation(&auth, replies);
    let outcome = tokio::time::timeout(Duration::from_secs(10), search_exchange(&auth, "bob", "pw")).await;
    assert!(matches!(outcome, Ok(Err(LdapAuthError::Timeout))), "{outcome:?}");
    drop(auth);
    peer.abort();
}

#[tokio::test(start_paused = true)]
async fn not_in_group_is_403_without_identity_and_with_delay() {
    const DELAY_MS: u64 = 300;
    let auth = search_auth(LdapAuthConfig {
        auth_failure_delay_ms: DELAY_MS,
        ..group_search_config()
    });
    let mut replies = found_then_user_bind(0);
    replies.push(bind_reply(4, 0));
    replies.push(done_reply(5, 0));
    let peer = scripted_conversation(&auth, replies);
    let started = tokio::time::Instant::now();
    // Status 200 here means "no self-written response": the 403 is an ErrResponse.
    // No set_request_header expectation: any identity write panics the mock.
    let (result, log) = run_search_request(&auth, &ldap_header("bob:pw"), 200, |_| {}).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(b"403 Forbidden")),
        }
    );
    assert!(log.contains("Deny not in required group"), "{}", log.render_for_test());
    assert!(started.elapsed() >= Duration::from_millis(DELAY_MS));
    drop(auth);
    peer.await.unwrap();
}

#[test]
fn group_filter_is_built_from_trimmed_config() {
    let auth = search_auth(LdapAuthConfig {
        group_attribute: " memberOf ".to_string(),
        required_groups_all_of: Some(vec![" cn=a,dc=x ".to_string()]),
        nested_groups: true,
        ..group_search_config()
    });
    let groups = search_plan(&auth).groups.as_ref().expect("group filter");
    let expected = SearchFilter::group_requirement("memberOf", &[GROUP_DN], &["cn=a,dc=x"], true).unwrap();
    assert_eq!(groups, &expected);
    assert!(search_plan(&search_auth(search_config())).groups.is_none());
}

#[test]
fn bind_failure_tags_are_distinct_and_static() {
    assert_eq!(BindFailure::Result.tag(), "ldap bind fail");
    assert_eq!(BindFailure::Transport.tag(), "ldap bind transport error");
    assert_eq!(BindFailure::Tls.tag(), "ldap bind tls error");
    assert_eq!(BindFailure::Protocol.tag(), "ldap bind protocol error");
}

#[test]
fn bind_failure_classes() {
    use edgion_ldap::Error;
    use std::io::{Error as IoError, ErrorKind};
    assert_eq!(bind_failure(&Error::Closed), BindFailure::Transport);
    assert_eq!(
        bind_failure(&Error::Io(IoError::from(ErrorKind::ConnectionReset))),
        BindFailure::Transport
    );
    // rustls reports a post-handshake alert (for example a rejected client
    // certificate under TLS 1.3) as InvalidData.
    assert_eq!(
        bind_failure(&Error::Io(IoError::new(ErrorKind::InvalidData, "alert"))),
        BindFailure::Tls
    );
    assert_eq!(bind_failure(&Error::Truncated), BindFailure::Protocol);
    assert_eq!(
        bind_failure(&Error::UnexpectedMessageId { expected: 1, actual: 2 }),
        BindFailure::Protocol
    );
}

#[tokio::test]
async fn bind_failure_classes_pick_their_tags() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    // (peer reply or None for "close after the request", expected tag)
    let cases: [(Option<Vec<u8>>, &str); 3] = [
        (Some(bind_response(53)), "ldap bind fail"),
        (None, "ldap bind transport error"),
        (
            Some(vec![0x30, 0x05, 0x02, 0x01, 0x09, 0x61, 0x00]),
            "ldap bind protocol error",
        ),
    ];
    for (reply, tag) in cases {
        let auth = ldap_auth_default();
        let peer = match reply {
            Some(reply) => scripted_peer(&auth, 4096, reply),
            None => {
                let (client, mut peer) = tokio::io::duplex(4096);
                auth.install_scripted_stream(client);
                tokio::spawn(async move {
                    let _ = read_request(&mut peer).await;
                })
            }
        };
        let header = ldap_header("bob:pw");
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("authorization", header.as_str())]);
        session.expect_record_outcome().times(1).return_const(());
        session
            .expect_write_response_header()
            .withf(|header, _| header.status.as_u16() == 503)
            .times(1)
            .returning(|_, _| Ok(()));
        session.expect_write_response_body().times(1).returning(|_, _| Ok(()));
        session.expect_shutdown().times(1).returning(|| {});
        let mut log = PluginLog::new(PLUGIN_NAME);
        auth.run_request(&mut session, &mut log).await;
        assert!(log.contains(tag), "{tag}: {}", log.render_for_test());
        drop(auth);
        let _ = peer.await;
    }
}

#[tokio::test]
async fn start_tls_rejection_run_request_tag_and_metric() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    install_test_crypto_provider();
    let (owner, _, _) = ldap_tls_owner(true);
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let defaults = edgion_common::link_sys_core::outbound_tls::test_util::snapshot(Default::default());
    let auth = LdapAuth::new(&entry, &defaults).expect("StartTLS fixture must build");
    let rejected = vec![
        0x30, 0x0c, 0x02, 0x01, 0x01, 0x78, 0x07, 0x0a, 0x01, 0x02, 0x04, 0x00, 0x04, 0x00,
    ];
    let peer = scripted_peer(&auth, 4096, rejected);
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let header = ldap_header("bob:pw");
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("authorization", header.as_str())]);
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
    session.expect_write_response_body().times(1).returning(|_, _| Ok(()));
    session.expect_shutdown().times(1).returning(|| {});
    let mut log = PluginLog::new(PLUGIN_NAME);
    auth.run_request(&mut session, &mut log).await;
    assert!(log.contains("ldap starttls rejected"), "{}", log.render_for_test());
    assert!(!log.contains("ldap unreachable"), "{}", log.render_for_test());
    let count: u64 = snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .filter(|(key, _, _, _)| {
            key.key().name() == crate::observe::metrics::registry::names::LDAP_AUTH_ERRORS_TOTAL
                && key
                    .key()
                    .labels()
                    .any(|label| label.key() == "kind" && label.value() == "starttls_rejected")
        })
        .map(|(_, _, _, value)| match value {
            DebugValue::Counter(count) => count,
            _ => 0,
        })
        .sum();
    assert_eq!(count, 1, "the catalog must admit kind=starttls_rejected");
    drop(auth);
    peer.await.unwrap();
}
