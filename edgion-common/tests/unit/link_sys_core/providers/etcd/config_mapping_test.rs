use super::*;
use crate::link_sys_core::credential::{CredentialState, InvalidReason};
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::etcd::*;
use edgion_resources::resources::link_sys::{LinkSys, LinkSysSpec};
use std::sync::Arc;

fn owned_tls(config: &EtcdClientConfig) -> Result<Option<TlsOptions>> {
    let owner = LinkSys {
        metadata: kube::core::ObjectMeta {
            name: Some("etcd".into()),
            namespace: Some("test".into()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            config: SystemConfig::Etcd(config.clone()),
            current_status: None,
            unknown_fields: vec![],
        },
        status: None,
    };
    let input = PreparedLinkSysTls::from_link_sys(Arc::new(owner))?;
    input.with_tls_selection(|proof, defaults| {
        if proof.active_field().is_none() {
            return Ok(default_tls_options(&config.endpoints));
        }
        let resolved = outbound_tls::resolve_validated_outbound_tls_audited(proof, false, "Etcd", defaults)
            .map_err(tls_configuration_error)?;
        tls_options_from_resolved(resolved).map(Some)
    })
}

// Exercise the private scalar mapper and auth mismatch arm; this helper is
// test-only and creates no public detached resource-construction entry.
fn build_connect_options(
    config: &EtcdClientConfig,
    state: &CredentialState<BasicAuthMaterial>,
) -> Result<Option<ConnectOptions>> {
    validate_fields(&config.endpoints, config.timeout.as_ref(), config.keep_alive.as_ref())?;
    let auth = auth_material(config.auth.is_some(), state)?;
    Ok(build_base_options(
        config.timeout.as_ref(),
        config.keep_alive.as_ref(),
        auth,
        owned_tls(config)?,
    ))
}

fn build_tls_options(tls: &TlsConfig) -> Result<Option<TlsOptions>> {
    let mut config = minimal_config(vec!["https://localhost:2379".into()]);
    config.tls = Some(tls.clone());
    owned_tls(&config)
}

/// Shorthand: `NotRequired` state (no auth configured).
fn no_auth() -> CredentialState<BasicAuthMaterial> {
    CredentialState::NotRequired
}

/// Zero-impact guard (spec §10.5): the SHARED builder must never inject
/// defaults of its own. An unconfigured LinkSys etcd client yields `None`
/// here and the prepared ordinary/election views use a dial-only fallback —
/// no h2 keepalive. The conf_center's always-on keepalive lives in
/// `EtcdConfig::to_bootstrap_config` (edgion-controller), not here; if this
/// test fails, a conf_center default has leaked into every etcd client.
#[test]
fn unconfigured_client_gets_no_injected_options() {
    let crd = minimal_config(vec!["http://127.0.0.1:2379".to_string()]);
    assert!(
        build_connect_options(&crd, &no_auth()).unwrap().is_none(),
        "shared builder must not inject defaults for an unconfigured client"
    );
}

/// Create a minimal EtcdClientConfig for testing
fn minimal_config(endpoints: Vec<String>) -> EtcdClientConfig {
    EtcdClientConfig {
        endpoints,
        auth: None,
        tls: None,
        timeout: None,
        keep_alive: None,
        namespace: None,
    }
}

fn crd_with_secret_ref() -> EtcdClientConfig {
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    EtcdClientConfig {
        auth: Some(EtcdAuth {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "etcd-creds".to_string(),
                    namespace: None,
                },
                secret: None,
            },
        }),
        ..minimal_config(vec!["http://localhost:2379".to_string()])
    }
}

#[test]
fn test_minimal_config_returns_none() {
    let crd = minimal_config(vec!["http://localhost:2379".to_string()]);
    let options = build_connect_options(&crd, &no_auth()).unwrap();
    assert!(options.is_none());
}

/// `Pending` state → fail-fast.
#[test]
fn test_with_auth_pending_fails_fast() {
    let crd = crd_with_secret_ref();
    let err = build_connect_options(&crd, &CredentialState::Pending).expect_err("should refuse pending");
    assert!(
        err.to_string().contains("has not been resolved"),
        "unexpected error message: {err}"
    );
}

/// `Invalid` state → fail-fast, with reason surfaced.
#[test]
fn test_with_auth_invalid_fails_fast() {
    let crd = crd_with_secret_ref();
    let state = CredentialState::Invalid(InvalidReason::ParseFailed("missing keys: [password]".into()));
    let err = build_connect_options(&crd, &state).expect_err("should refuse invalid");
    assert!(
        err.to_string().contains("invalid") && err.to_string().contains("missing"),
        "unexpected error message: {err}"
    );
}

/// Happy path: auth configured and material resolved → options built.
#[test]
fn test_with_auth_and_material() {
    let crd = crd_with_secret_ref();
    let state = CredentialState::Resolved(BasicAuthMaterial {
        username: Some("root".to_string()),
        password: "pw".to_string(),
    });
    let options = build_connect_options(&crd, &state).unwrap();
    assert!(options.is_some());
}

/// Invariant guard: the CRD-vs-state mismatch arm. If a future refactor
/// breaks `credential::basic_state` so it returns `NotRequired` even when
/// CRD has `auth`, this test locks the fail-fast arm.
#[test]
fn test_crd_has_auth_but_state_not_required_is_internal_bug() {
    let crd = crd_with_secret_ref();
    let err = build_connect_options(&crd, &CredentialState::NotRequired).expect_err("should refuse");
    assert!(
        err.to_string().contains("internal bug"),
        "unexpected error message: {err}"
    );
}

/// Password-only material must be accepted (etcd allows empty username).
#[test]
fn test_with_auth_password_only_material_is_accepted() {
    let crd = crd_with_secret_ref();
    let state = CredentialState::Resolved(BasicAuthMaterial {
        username: None,
        password: "pw".to_string(),
    });
    let options = build_connect_options(&crd, &state).unwrap();
    assert!(options.is_some());
}

#[test]
fn test_with_dial_timeout() {
    let crd = EtcdClientConfig {
        timeout: Some(EtcdTimeout {
            dial: Some(3000),
            request: None,
        }),
        ..minimal_config(vec!["http://localhost:2379".to_string()])
    };
    let options = build_connect_options(&crd, &no_auth()).unwrap();
    assert!(options.is_some());
}

#[test]
fn test_dial_timeout_clamped() {
    let crd = EtcdClientConfig {
        timeout: Some(EtcdTimeout {
            dial: Some(999_999),
            request: None,
        }),
        ..minimal_config(vec!["http://localhost:2379".to_string()])
    };
    let options = build_connect_options(&crd, &no_auth()).unwrap();
    assert!(options.is_some());
}

#[test]
fn test_with_keepalive() {
    let crd = EtcdClientConfig {
        keep_alive: Some(EtcdKeepAlive {
            time: Some(30),
            timeout: Some(10),
            permit_without_stream: Some(true),
        }),
        ..minimal_config(vec!["http://localhost:2379".to_string()])
    };
    let options = build_connect_options(&crd, &no_auth()).unwrap();
    assert!(options.is_some());
}

#[test]
fn test_keepalive_partial_returns_error() {
    let crd = EtcdClientConfig {
        keep_alive: Some(EtcdKeepAlive {
            time: Some(30),
            timeout: None,
            permit_without_stream: None,
        }),
        ..minimal_config(vec!["http://localhost:2379".to_string()])
    };
    let error = build_connect_options(&crd, &no_auth()).unwrap_err();
    assert!(error.to_string().contains("keepAlive requires time and timeout"));
}

#[test]
fn test_with_tls_enabled_no_certs() {
    use edgion_resources::resources::common::OutboundTlsLocal;
    let _guard = outbound_tls::outbound_tls_test_guard();
    let crd = EtcdClientConfig {
        tls: Some(TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal::default(),
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        }),
        ..minimal_config(vec!["https://localhost:2379".to_string()])
    };
    let options = build_connect_options(&crd, &no_auth()).unwrap();
    assert!(options.is_some());
}

/// `verify=false` must fail fast for Etcd rather than silently being
/// ignored — etcd-client has no skip-verify knob and an operator who set it
/// (e.g. for self-signed dev certs) would otherwise get strict verification
/// with no signal.
#[test]
fn test_insecure_skip_verify_bails() {
    use edgion_resources::resources::common::OutboundTlsLocal;
    let _guard = outbound_tls::outbound_tls_test_guard();
    let crd = EtcdClientConfig {
        tls: Some(TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal {
                verify: Some(false),
                ..Default::default()
            },
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        }),
        ..minimal_config(vec!["https://localhost:2379".to_string()])
    };
    let err = build_connect_options(&crd, &no_auth()).expect_err("should refuse verify=false");
    assert!(err.to_string().contains("Etcd TLS"), "unexpected error message: {err}");
}

#[test]
fn test_tls_disabled_returns_none() {
    use edgion_resources::resources::common::OutboundTlsLocal;
    let crd = EtcdClientConfig {
        tls: Some(TlsConfig {
            enabled: false,
            outbound: OutboundTlsLocal::default(),
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        }),
        ..minimal_config(vec!["http://localhost:2379".to_string()])
    };
    let options = build_connect_options(&crd, &no_auth()).unwrap();
    assert!(options.is_none());
}

#[test]
fn test_health_probe_timeout_uses_configured_request() {
    let crd = EtcdClientConfig {
        timeout: Some(EtcdTimeout {
            dial: None,
            request: Some(10_000),
        }),
        ..minimal_config(vec!["http://localhost:2379".to_string()])
    };
    assert_eq!(health_probe_timeout(&crd), Duration::from_millis(10_000));
}

#[test]
fn test_health_probe_timeout_clamps_to_max() {
    let crd = EtcdClientConfig {
        timeout: Some(EtcdTimeout {
            dial: None,
            request: Some(999_999),
        }),
        ..minimal_config(vec!["http://localhost:2379".to_string()])
    };
    // MAX_REQUEST_TIMEOUT_MS = 60_000
    assert_eq!(health_probe_timeout(&crd), Duration::from_millis(60_000));
}

#[test]
fn test_health_probe_timeout_defaults_when_unset() {
    let crd = minimal_config(vec!["http://localhost:2379".to_string()]);
    assert_eq!(health_probe_timeout(&crd), Duration::from_millis(5_000));

    let crd2 = EtcdClientConfig {
        timeout: Some(EtcdTimeout {
            dial: Some(3_000),
            request: None,
        }),
        ..minimal_config(vec!["http://localhost:2379".to_string()])
    };
    assert_eq!(health_probe_timeout(&crd2), Duration::from_millis(5_000));
}

#[test]
fn etcd_rejects_inherited_hostname_policy() {
    use crate::link_sys_core::outbound_tls;
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    use edgion_resources::resources::common::{OutboundTlsConfig, OutboundTlsLocal};
    let _guard = outbound_tls::outbound_tls_test_guard();
    outbound_tls::set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            validation: Some(OutboundTlsValidation {
                hostname: Some("example.com".into()),
                well_known_ca_certificates: Some(
                    edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::System,
                ),
                ..Default::default()
            }),
            ..Default::default()
        },
    ));
    let tls = TlsConfig {
        enabled: true,
        outbound: OutboundTlsLocal::default(),
        resolved_ca_certificates: None,
        resolved_client_certificate: None,
    };
    assert!(build_tls_options(&tls).is_err());
}

#[test]
fn ordinary_and_election_timeout_policy_preserves_request_only_and_base_options() {
    let fallback = Some(Duration::from_millis(5_000));
    let unary = Some(Duration::from_millis(700));
    for (base, request, ordinary_dial, ordinary_request, election_dial) in [
        (false, None, fallback, None, fallback),
        (false, Some(700), None, unary, fallback),
        (true, None, None, None, None),
        (true, Some(700), None, unary, None),
    ] {
        let (ordinary, election) = connection_timeout_overrides(base, request);
        assert_eq!(
            ordinary,
            ConnectionTimeoutOverrides {
                dial: ordinary_dial,
                request: ordinary_request
            }
        );
        assert_eq!(
            election,
            ConnectionTimeoutOverrides {
                dial: election_dial,
                request: None
            }
        );
    }
    // A supplied dial/auth/TLS/keepalive base never acquires a fallback dial;
    // election retains that base and adds no unary request deadline.
    let (ordinary, election) = connection_timeout_overrides(true, Some(u64::MAX));
    assert_eq!(ordinary.request, Some(Duration::from_millis(MAX_REQUEST_TIMEOUT_MS)));
    assert_eq!(ordinary.dial, None);
    assert_eq!(election.request, None);
    assert_eq!(election.dial, None);
    assert_eq!(
        clamped_dial_timeout(u64::MAX),
        Duration::from_millis(MAX_DIAL_TIMEOUT_MS)
    );
    assert_eq!(clamped_dial_timeout(750), Duration::from_millis(750));
}

#[test]
fn system_trust_is_passed_explicitly_and_custom_ca_replaces_it() {
    // tonic adds no roots by itself: System must arrive as the process bundle.
    assert_eq!(
        ca_pem_bundles(&EffectiveCa::System),
        vec![crate::system_trust::system_trust().pem()]
    );
    let custom = EffectiveCa::Custom(vec![b"first".to_vec(), b"second".to_vec()]);
    assert_eq!(ca_pem_bundles(&custom), vec![&b"first"[..], &b"second"[..]]);
}

#[test]
fn omitted_tls_gives_all_https_endpoints_system_trust_only() {
    let endpoints = |list: &[&str]| list.iter().map(|e| e.to_string()).collect::<Vec<_>>();
    assert!(uses_default_system_tls(&endpoints(&[
        "https://a:2379",
        "https://b:2379"
    ])));
    // etcd-client rejects TLS options for http:// and would apply them to
    // scheme-less endpoints, so those keep plaintext.
    assert!(!uses_default_system_tls(&endpoints(&[
        "https://a:2379",
        "http://b:2379"
    ])));
    assert!(!uses_default_system_tls(&endpoints(&["a:2379"])));
    assert!(!uses_default_system_tls(&endpoints(&["http://a:2379"])));
    assert!(!uses_default_system_tls(&[]));
    assert!(default_tls_options(&endpoints(&["https://a:2379"])).is_some());
    assert!(default_tls_options(&endpoints(&["http://a:2379"])).is_none());
}
