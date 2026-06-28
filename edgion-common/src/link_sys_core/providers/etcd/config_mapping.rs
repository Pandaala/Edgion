//! CRD `EtcdClientConfig` → `etcd_client::ConnectOptions` mapping.
//!
//! This module bridges the CRD configuration types (serde-driven, user-facing YAML)
//! with etcd-client's `ConnectOptions`. The mapping follows the Library-First principle:
//! we use etcd-client's API directly without inventing our own abstractions.

use std::time::Duration;

use anyhow::Result;
use etcd_client::{Certificate, ConnectOptions, Identity, TlsOptions};

use crate::link_sys_core::credential::CredentialState;
use crate::link_sys_core::outbound_tls::{self, EffectiveCa};
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;

// ============================================================================
// Safety ceilings for connection configuration
// ============================================================================

const MAX_DIAL_TIMEOUT_MS: u64 = 30_000;
const MAX_REQUEST_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_DIAL_TIMEOUT_MS: u64 = 5_000;

// ============================================================================
// Config mapping (CRD → etcd-client ConnectOptions)
// ============================================================================

/// Map `EtcdClientConfig` (CRD) to `etcd_client::ConnectOptions`.
///
/// Returns `None` if no special options are needed (minimal config).
/// The caller passes the options to `Client::connect(endpoints, options)`.
///
/// `auth` is the [`CredentialState`] produced by
/// [`credential::basic_state`](crate::link_sys_core::credential::basic_state).
/// `Pending` / `Invalid` produce a fail-fast `bail!`.
pub fn build_connect_options(
    crd: &EtcdClientConfig,
    auth: &CredentialState<BasicAuthMaterial>,
) -> Result<Option<ConnectOptions>> {
    let mut options = ConnectOptions::new();
    let mut has_options = false;

    if crd.auth.is_some() {
        let m = crate::link_sys_core::credential::require_basic_material(auth, "Etcd")?.ok_or_else(|| {
            anyhow::anyhow!("Etcd auth configured in CRD but credential state is NotRequired (internal bug)")
        })?;
        let user = m.username.as_deref().unwrap_or("");
        options = options.with_user(user, m.password.as_str());
        has_options = true;
    }

    // TLS — parse CA / mTLS material from the CRD (inline + resolved Secret)
    // and feed it into etcd-client's `TlsOptions`.
    if let Some(tls) = &crd.tls {
        if tls.enabled {
            let tls_options = build_tls_options(tls)?;
            options = options.with_tls(tls_options);
            has_options = true;
        }
    }

    // Dial (connection) timeout
    if let Some(timeout) = &crd.timeout {
        if let Some(dial_ms) = timeout.dial {
            let dial = dial_ms.min(MAX_DIAL_TIMEOUT_MS);
            options = options.with_connect_timeout(Duration::from_millis(dial));
            has_options = true;
        }
        // Per-request timeout
        if let Some(request_ms) = timeout.request {
            let request = request_ms.min(MAX_REQUEST_TIMEOUT_MS);
            options = options.with_timeout(Duration::from_millis(request));
            has_options = true;
        }
    }

    // HTTP/2 keep-alive
    if let Some(ka) = &crd.keep_alive {
        if let (Some(time), Some(timeout)) = (ka.time, ka.timeout) {
            options = options.with_keep_alive(Duration::from_secs(time), Duration::from_secs(timeout));
            has_options = true;

            if ka.permit_without_stream == Some(true) {
                options = options.with_keep_alive_while_idle(true);
            }
        }
    }

    Ok(if has_options { Some(options) } else { None })
}

/// Build TLS options from CRD config.
///
/// Uses the shared [`outbound_tls::resolve_outbound_tls`] resolver with
/// `provider_can_disable: false` — etcd-client's TLS stack always verifies
/// certificates and has no skip-verify knob, so the resolver will return
/// [`EdgionStatus::OutboundTlsVerifyUnsupported`] (mapped to an error here)
/// if `verify=false` is configured, preventing a silent ignore of the
/// operator's intent.
fn build_tls_options(tls: &TlsConfig) -> Result<TlsOptions> {
    let resolved = outbound_tls::resolve_outbound_tls(
        &tls.outbound,
        tls.resolved_ca_secret.as_ref(),
        tls.resolved_client_secret.as_ref(),
        false, // etcd-client cannot disable TLS verification
        "Etcd",
    )
    .map_err(|e| {
        anyhow::anyhow!(
            "Etcd TLS configuration error: {e:?}. \
             etcd-client does not support skip-verify; \
             use a proper CA certificate or connect without TLS."
        )
    })?;

    let mut tls_options = TlsOptions::new();

    if let EffectiveCa::Custom(ca_pem) = &resolved.ca {
        tls_options = tls_options.ca_certificate(Certificate::from_pem(ca_pem));
    }

    if let Some((cert_pem, key_pem)) = &resolved.client_identity {
        tls_options = tls_options.identity(Identity::from_pem(cert_pem, key_pem.as_slice()));
    }

    Ok(tls_options)
}

/// Get default connect options with a reasonable dial timeout.
/// Used when no specific options are configured but we still want a timeout.
pub fn default_connect_options() -> ConnectOptions {
    ConnectOptions::new().with_connect_timeout(Duration::from_millis(DEFAULT_DIAL_TIMEOUT_MS))
}

/// Default ceiling for live health-probe `status()` calls when the CRD does not
/// set an explicit per-request timeout. 5s matches the Redis provider's
/// DEFAULT_COMMAND_TIMEOUT_MS, keeping cross-provider health semantics aligned.
const DEFAULT_HEALTH_PROBE_TIMEOUT_MS: u64 = 5_000;

/// Resolve the per-call timeout for a health-probe `status()`: prefer the
/// operator-configured request timeout, else the 5s default. Clamped to
/// `MAX_REQUEST_TIMEOUT_MS` so a misconfiguration cannot disable the ceiling.
///
/// etcd-client already honors a configured request timeout internally; this
/// wrap is a hard backstop whose real value is protecting the *unconfigured*
/// case (where a stuck etcd would otherwise hang the call forever).
pub fn health_probe_timeout(crd: &EtcdClientConfig) -> Duration {
    crd.timeout
        .as_ref()
        .and_then(|t| t.request)
        .map(|ms| Duration::from_millis(ms.min(MAX_REQUEST_TIMEOUT_MS)))
        .unwrap_or_else(|| Duration::from_millis(DEFAULT_HEALTH_PROBE_TIMEOUT_MS))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link_sys_core::credential::{CredentialState, InvalidReason};
    use edgion_resources::resources::link_sys::etcd::*;

    /// Shorthand: `NotRequired` state (no auth configured).
    fn no_auth() -> CredentialState<BasicAuthMaterial> {
        CredentialState::NotRequired
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
            auto_sync_interval: None,
            max_call_send_size: None,
            max_call_recv_size: None,
            user_agent: None,
            reject_old_cluster: None,
            observability: None,
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
                keep_alive: None,
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
                keep_alive: None,
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
    fn test_keepalive_partial_returns_none() {
        let crd = EtcdClientConfig {
            keep_alive: Some(EtcdKeepAlive {
                time: Some(30),
                timeout: None,
                permit_without_stream: None,
            }),
            ..minimal_config(vec!["http://localhost:2379".to_string()])
        };
        let options = build_connect_options(&crd, &no_auth()).unwrap();
        assert!(options.is_none());
    }

    #[test]
    fn test_with_tls_enabled_no_certs() {
        use edgion_resources::resources::common::OutboundTlsLocal;
        let crd = EtcdClientConfig {
            tls: Some(TlsConfig {
                enabled: true,
                outbound: OutboundTlsLocal::default(),
                resolved_ca_secret: None,
                resolved_client_secret: None,
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
        let crd = EtcdClientConfig {
            tls: Some(TlsConfig {
                enabled: true,
                outbound: OutboundTlsLocal {
                    verify: Some(false),
                    ..Default::default()
                },
                resolved_ca_secret: None,
                resolved_client_secret: None,
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
                resolved_ca_secret: None,
                resolved_client_secret: None,
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
                keep_alive: None,
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
                keep_alive: None,
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
                keep_alive: None,
            }),
            ..minimal_config(vec!["http://localhost:2379".to_string()])
        };
        assert_eq!(health_probe_timeout(&crd2), Duration::from_millis(5_000));
    }
}
