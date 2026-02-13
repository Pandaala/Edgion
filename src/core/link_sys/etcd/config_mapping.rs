//! CRD `EtcdClientConfig` → `etcd_client::ConnectOptions` mapping.
//!
//! This module bridges the CRD configuration types (serde-driven, user-facing YAML)
//! with etcd-client's `ConnectOptions`. The mapping follows the Library-First principle:
//! we use etcd-client's API directly without inventing our own abstractions.

use std::time::Duration;

use anyhow::Result;
use etcd_client::{Certificate, ConnectOptions, Identity, TlsOptions};

use crate::types::resources::link_sys::etcd::{EtcdClientConfig, EtcdTls};

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
pub fn build_connect_options(crd: &EtcdClientConfig) -> Result<Option<ConnectOptions>> {
    let mut options = ConnectOptions::new();
    let mut has_options = false;

    // Authentication (secret_ref is resolved externally before calling this function)
    if let Some(auth) = &crd.auth {
        if let (Some(user), Some(pass)) = (&auth.username, &auth.password) {
            options = options.with_user(user, pass);
            has_options = true;
        }
    }

    // TLS
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
            options = options.with_keep_alive(
                Duration::from_secs(time),
                Duration::from_secs(timeout),
            );
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
/// Uses Rustls (etcd-client default) for TLS connections.
/// Supports CA certificate and client certificate (mTLS).
fn build_tls_options(tls: &EtcdTls) -> Result<TlsOptions> {
    let mut tls_options = TlsOptions::new();

    if let Some(certs) = &tls.certs {
        // CA certificate
        if let Some(ca_pem) = &certs.ca_cert {
            let ca = Certificate::from_pem(ca_pem);
            tls_options = tls_options.ca_certificate(ca);
        }

        // Client certificate + key (mTLS)
        if let (Some(cert_pem), Some(key_pem)) = (&certs.client_cert, &certs.client_key) {
            let identity = Identity::from_pem(cert_pem, key_pem);
            tls_options = tls_options.identity(identity);
        }

        // Note: certs.secret_ref is resolved externally (controller side)
        // and populated into ca_cert/client_cert/client_key fields.
    }

    // etcd-client TlsOptions does not have insecure_skip_verify.
    // If needed, would require custom tonic channel configuration.
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
    ConnectOptions::new()
        .with_connect_timeout(Duration::from_millis(DEFAULT_DIAL_TIMEOUT_MS))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::link_sys::etcd::*;

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

    #[test]
    fn test_minimal_config_returns_none() {
        let crd = minimal_config(vec!["http://localhost:2379".to_string()]);
        let options = build_connect_options(&crd).unwrap();
        assert!(options.is_none());
    }

    #[test]
    fn test_with_auth() {
        let crd = EtcdClientConfig {
            auth: Some(EtcdAuth {
                username: Some("user".to_string()),
                password: Some("pass".to_string()),
                secret_ref: None,
            }),
            ..minimal_config(vec!["http://localhost:2379".to_string()])
        };
        let options = build_connect_options(&crd).unwrap();
        assert!(options.is_some());
    }

    #[test]
    fn test_auth_without_password_returns_none() {
        let crd = EtcdClientConfig {
            auth: Some(EtcdAuth {
                username: Some("user".to_string()),
                password: None,
                secret_ref: None,
            }),
            ..minimal_config(vec!["http://localhost:2379".to_string()])
        };
        let options = build_connect_options(&crd).unwrap();
        // No password → no auth configured → no options
        assert!(options.is_none());
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
        let options = build_connect_options(&crd).unwrap();
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
        // Should not error — just clamp to MAX_DIAL_TIMEOUT_MS
        let options = build_connect_options(&crd).unwrap();
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
        let options = build_connect_options(&crd).unwrap();
        assert!(options.is_some());
    }

    #[test]
    fn test_keepalive_partial_returns_none() {
        // Only time set, no timeout → skip keepalive
        let crd = EtcdClientConfig {
            keep_alive: Some(EtcdKeepAlive {
                time: Some(30),
                timeout: None,
                permit_without_stream: None,
            }),
            ..minimal_config(vec!["http://localhost:2379".to_string()])
        };
        let options = build_connect_options(&crd).unwrap();
        assert!(options.is_none());
    }

    #[test]
    fn test_with_tls_enabled_no_certs() {
        let crd = EtcdClientConfig {
            tls: Some(EtcdTls {
                enabled: true,
                certs: None,
                insecure_skip_verify: None,
            }),
            ..minimal_config(vec!["https://localhost:2379".to_string()])
        };
        let options = build_connect_options(&crd).unwrap();
        assert!(options.is_some());
    }

    #[test]
    fn test_tls_disabled_returns_none() {
        let crd = EtcdClientConfig {
            tls: Some(EtcdTls {
                enabled: false,
                certs: None,
                insecure_skip_verify: None,
            }),
            ..minimal_config(vec!["http://localhost:2379".to_string()])
        };
        let options = build_connect_options(&crd).unwrap();
        assert!(options.is_none());
    }
}
