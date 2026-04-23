//! CRD `EtcdClientConfig` → `etcd_client::ConnectOptions` mapping.
//!
//! This module bridges the CRD configuration types (serde-driven, user-facing YAML)
//! with etcd-client's `ConnectOptions`. The mapping follows the Library-First principle:
//! we use etcd-client's API directly without inventing our own abstractions.

use std::time::Duration;

use anyhow::Result;
use etcd_client::{Certificate, ConnectOptions, Identity, TlsOptions};

use crate::core::gateway::link_sys::credential::CredentialState;
use crate::core::gateway::link_sys::tls as tls_helper;
use crate::types::credentials::materials::BasicAuthMaterial;
use crate::types::resources::common::TlsConfig;
use crate::types::resources::link_sys::etcd::EtcdClientConfig;

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
/// [`credential::basic_state`](crate::core::gateway::link_sys::credential::basic_state).
/// `Pending` / `Invalid` produce a fail-fast `bail!` (see I4 / M4).
pub fn build_connect_options(
    crd: &EtcdClientConfig,
    auth: &CredentialState<BasicAuthMaterial>,
) -> Result<Option<ConnectOptions>> {
    let mut options = ConnectOptions::new();
    let mut has_options = false;

    if crd.auth.is_some() {
        match auth {
            CredentialState::Resolved(m) => {
                // `BasicAuthResolver` legitimately supports password-only
                // secrets — username may be `None` inside a valid material.
                let user = m.username.as_deref().unwrap_or("");
                options = options.with_user(user, m.password.as_str());
                has_options = true;
            }
            CredentialState::Pending => {
                anyhow::bail!(
                    "Etcd auth is configured via secretRef but credential material \
                     has not been resolved yet; refusing to build client without credentials"
                );
            }
            CredentialState::Invalid(reason) => {
                anyhow::bail!("Etcd credential material is invalid: {reason}");
            }
            CredentialState::NotRequired => {
                anyhow::bail!(
                    "Etcd auth configured in CRD but credential state is NotRequired (internal bug)"
                );
            }
        }
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
/// Uses Rustls (etcd-client default) for TLS connections. CA + client mTLS
/// come from [`tls_helper::build_material`], which merges inline PEM from the
/// CRD with the resolved K8s Secret (`tls.crt` / `tls.key` / `ca.crt`).
fn build_tls_options(tls: &TlsConfig) -> Result<TlsOptions> {
    let mut tls_options = TlsOptions::new();

    if let Some(material) = tls_helper::build_material(tls) {
        if let Some(ca_pem) = &material.ca_pem {
            tls_options = tls_options.ca_certificate(Certificate::from_pem(ca_pem));
        }
        if let (Some(cert), Some(key)) = (&material.client_cert_pem, &material.client_key_pem) {
            tls_options = tls_options.identity(Identity::from_pem(cert, key.as_slice()));
        }
    }

    // etcd-client TlsOptions does not expose an insecure_skip_verify knob;
    // surfacing a warning lets operators know the flag is ignored here.
    if tls.insecure_skip_verify == Some(true) {
        tracing::warn!(
            "Etcd: insecure_skip_verify is not directly supported by etcd-client TlsOptions; \
             use a custom channel if needed"
        );
    }

    Ok(tls_options)
}

/// Get default connect options with a reasonable dial timeout.
/// Used when no specific options are configured but we still want a timeout.
pub fn default_connect_options() -> ConnectOptions {
    ConnectOptions::new().with_connect_timeout(Duration::from_millis(DEFAULT_DIAL_TIMEOUT_MS))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::link_sys::credential::{CredentialState, InvalidReason};
    use crate::types::resources::link_sys::etcd::*;

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
        use crate::types::resources::common::SecretReference;
        EtcdClientConfig {
            auth: Some(EtcdAuth {
                secret_ref: SecretReference {
                    name: "etcd-creds".to_string(),
                    namespace: None,
                },
                secret: None,
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

    /// I4 / M4 regression: `Pending` state → fail-fast.
    #[test]
    fn test_with_auth_pending_fails_fast() {
        let crd = crd_with_secret_ref();
        let err = build_connect_options(&crd, &CredentialState::Pending).expect_err("should refuse pending");
        assert!(
            err.to_string().contains("has not been resolved"),
            "unexpected error message: {err}"
        );
    }

    /// M4 regression: `Invalid` state → fail-fast, with reason surfaced.
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
        let crd = EtcdClientConfig {
            tls: Some(TlsConfig {
                enabled: true,
                certs: None,
                insecure_skip_verify: None,
            }),
            ..minimal_config(vec!["https://localhost:2379".to_string()])
        };
        let options = build_connect_options(&crd, &no_auth()).unwrap();
        assert!(options.is_some());
    }

    #[test]
    fn test_tls_disabled_returns_none() {
        let crd = EtcdClientConfig {
            tls: Some(TlsConfig {
                enabled: false,
                certs: None,
                insecure_skip_verify: None,
            }),
            ..minimal_config(vec!["http://localhost:2379".to_string()])
        };
        let options = build_connect_options(&crd, &no_auth()).unwrap();
        assert!(options.is_none());
    }
}
