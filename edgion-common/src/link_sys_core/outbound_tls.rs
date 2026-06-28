//! Shared outbound TLS resolver for all outbound callers (LinkSys providers,
//! plugins, etc.).
//!
//! Provides a single entry point [`resolve_outbound_tls`] that:
//! - inherits the cluster-wide default from [`outbound_global_default`]
//! - applies per-call overrides from [`OutboundTlsLocal`]
//! - resolves CA material from inline PEM or a resolved Kubernetes Secret

use std::sync::{LazyLock, RwLock};

use edgion_resources::constants::secret_keys::tls as tls_keys;
use edgion_resources::edgion_status::EdgionStatus;
use edgion_resources::resources::common::{CaSource, OutboundTlsConfig, OutboundTlsLocal};
use k8s_openapi::api::core::v1::Secret;
use zeroize::Zeroizing;

// ============================================================================
// Output types
// ============================================================================

/// The effective CA trust source after resolving global defaults and local
/// overrides.
pub enum EffectiveCa {
    /// Built-in webpki roots compiled into the binary.
    System,
    /// PEM bundle that REPLACES the public roots (private CA mode).
    Custom(Vec<u8>),
}

/// Resolved outbound TLS configuration, ready for consumption by a provider.
pub struct ResolvedOutboundTls {
    pub verify: bool,
    pub ca: EffectiveCa,
    /// `(cert_pem, key_pem)` when a client-identity Secret was provided.
    pub client_identity: Option<(Vec<u8>, Zeroizing<Vec<u8>>)>,
}

// ============================================================================
// Cluster-wide default store
// ============================================================================

static OUTBOUND_GLOBAL_DEFAULT: LazyLock<RwLock<OutboundTlsConfig>> =
    LazyLock::new(|| RwLock::new(OutboundTlsConfig::default()));

/// Returns the cluster-wide outbound TLS default.
///
/// Initialized to `OutboundTlsConfig::default()` (`verify = true`, no CA
/// override). Updated by [`set_outbound_global_default`] when an
/// `EdgionGatewayConfig` is applied.
pub fn outbound_global_default() -> OutboundTlsConfig {
    OUTBOUND_GLOBAL_DEFAULT
        .read()
        .expect("global outbound TLS default lock poisoned")
        .clone()
}

/// Replace the cluster-wide outbound TLS default.
///
/// Called by the Gateway when an `EdgionGatewayConfig` carrying an
/// `outboundTls` block is applied.
pub fn set_outbound_global_default(cfg: OutboundTlsConfig) {
    *OUTBOUND_GLOBAL_DEFAULT
        .write()
        .expect("global outbound TLS default lock poisoned") = cfg;
}

// ============================================================================
// Aggregation
// ============================================================================

/// Safest-wins aggregation across multiple `EdgionGatewayConfig` outbound
/// blocks.
///
/// Returns a config whose `verify` is `false` ONLY when `!blocks.is_empty()`
/// AND every block has `verify == false`; otherwise `true`.
/// CA-material aggregation is done by the gateway caller, not here.
pub fn aggregate_verify(blocks: &[OutboundTlsConfig]) -> OutboundTlsConfig {
    if blocks.is_empty() {
        return OutboundTlsConfig::default();
    }
    let verify = blocks.iter().any(|b| b.verify);
    OutboundTlsConfig {
        verify,
        ..OutboundTlsConfig::default()
    }
}

// ============================================================================
// Resolver
// ============================================================================

/// The one resolver every outbound caller uses.
///
/// # Arguments
///
/// - `local` — per-call override; all-`None` fields inherit the global default.
/// - `resolved_ca` — the resolved K8s Secret when `caSource == SecretRef`.
/// - `resolved_client` — the resolved K8s Secret for client mTLS identity.
/// - `provider_can_disable` — whether this provider exposes a skip-verify knob.
///   Pass `false` for providers like Etcd whose TLS stack always verifies.
/// - `context` — short provider name embedded in log messages.
///
/// # CA source inference
///
/// When neither `local.caSource` nor the global default `caSource` is set, the
/// resolver infers the source from material presence (fail-secure):
/// - `caSecretRef` present (local or global) or `resolved_ca` is `Some` → `SecretRef`
/// - `caCert` (inline PEM) present (local or global) → `Inline`
/// - otherwise → `System` (built-in webpki roots)
///
/// An explicit `caSource` always wins; this inference only applies when the
/// discriminator is omitted.
///
/// # Global SecretRef
///
/// A GLOBAL `outboundTls.caSource: SecretRef` default IS supported: the
/// Controller resolves `outboundTls.caSecretRef` and ships the resolved CA
/// Secret via conf_sync (`OutboundTlsConfig::resolved_ca_secret`). An inheriting
/// call with no per-call `resolved_ca` falls back to that global resolved Secret.
///
/// # Known limitations
///
/// - M4: When `verify=false`, client mTLS identity is intentionally dropped
///   (the skip-verify path sends no client certificate).
///
/// # Operator responsibility for verify=false
///
/// When an operator explicitly sets `verify=false`, it is honored without an
/// environment check — the operator owns this decision. A WARN is emitted at
/// configuration-load time as an audit trail. `verify=false` is rejected ONLY
/// when `provider_can_disable == false` (a technical SDK-capability fact: e.g.
/// etcd-client and the rustls Redis connector have no skip-verify API, so
/// honoring the request would silently still verify).
///
/// # Errors
///
/// - [`EdgionStatus::OutboundTlsVerifyUnsupported`] — `verify=false` requested
///   but `provider_can_disable == false`.
/// - [`EdgionStatus::OutboundTlsCaMissing`] — `caSource` is `Inline` or
///   `SecretRef` but the CA PEM is absent.
/// - [`EdgionStatus::OutboundTlsClientCertInvalid`] — `resolved_client` is
///   `Some` but the Secret is missing `tls.crt` or `tls.key`.
pub fn resolve_outbound_tls(
    local: &OutboundTlsLocal,
    resolved_ca: Option<&Secret>,
    resolved_client: Option<&Secret>,
    provider_can_disable: bool,
    context: &str,
) -> Result<ResolvedOutboundTls, EdgionStatus> {
    let global = outbound_global_default();
    let verify = local.verify.unwrap_or(global.verify);

    if !verify {
        if !provider_can_disable {
            tracing::warn!(
                provider = context,
                "outbound TLS verify=false requested but provider does not support skip-verify"
            );
            return Err(EdgionStatus::OutboundTlsVerifyUnsupported);
        }
        // Operator explicitly disabled verification; honored as their responsibility.
        tracing::warn!(
            provider = context,
            "outbound TLS verification disabled (verify=false) — operator-configured, insecure"
        );
        return Ok(ResolvedOutboundTls {
            verify: false,
            ca: EffectiveCa::System,
            client_identity: None,
        });
    }

    // verify == true: resolve CA source.
    // When neither local nor global sets a discriminator, infer from material
    // presence (fail-secure: a present CA ref/cert means "trust that CA").
    let ca_source = local.ca_source.or(global.ca_source).unwrap_or_else(|| {
        if resolved_ca.is_some() || local.ca_secret_ref.is_some() || global.ca_secret_ref.is_some() {
            CaSource::SecretRef
        } else if local.ca_cert.is_some() || global.ca_cert.is_some() {
            CaSource::Inline
        } else {
            CaSource::System
        }
    });

    let ca = match ca_source {
        CaSource::System => EffectiveCa::System,
        CaSource::Inline => {
            let pem = local
                .ca_cert
                .as_deref()
                .or(global.ca_cert.as_deref())
                .ok_or(EdgionStatus::OutboundTlsCaMissing)?;
            EffectiveCa::Custom(pem.as_bytes().to_vec())
        }
        CaSource::SecretRef => {
            // Prefer the per-call resolved Secret; fall back to the cluster-wide
            // resolved CA Secret so an inheriting call (no local CA) uses the
            // global default that the Controller resolved and shipped.
            let pem = resolved_ca
                .or(global.resolved_ca_secret.as_ref())
                .and_then(|s| s.data.as_ref())
                .and_then(|d| d.get(tls_keys::CA_CERT))
                .map(|v| v.0.clone())
                .ok_or(EdgionStatus::OutboundTlsCaMissing)?;
            EffectiveCa::Custom(pem)
        }
    };

    // Resolve optional client mTLS identity
    let client_identity = match resolved_client {
        Some(secret) => {
            let cert = secret
                .data
                .as_ref()
                .and_then(|d| d.get(tls_keys::CERT))
                .map(|v| v.0.clone())
                .ok_or(EdgionStatus::OutboundTlsClientCertInvalid)?;
            let key = secret
                .data
                .as_ref()
                .and_then(|d| d.get(tls_keys::KEY))
                .map(|v| Zeroizing::new(v.0.clone()))
                .ok_or(EdgionStatus::OutboundTlsClientCertInvalid)?;
            Some((cert, key))
        }
        None => None,
    };

    Ok(ResolvedOutboundTls {
        verify,
        ca,
        client_identity,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_verify_safest_wins() {
        let mut a = OutboundTlsConfig::default();
        a.verify = false;
        let b = OutboundTlsConfig::default(); // verify = true
        assert!(aggregate_verify(&[a.clone(), b]).verify);
        let mut c = OutboundTlsConfig::default();
        c.verify = false;
        assert!(!aggregate_verify(&[a, c]).verify);
    }

    #[test]
    fn verify_false_is_honored_when_provider_can_disable() {
        set_outbound_global_default(OutboundTlsConfig::default());
        let local = OutboundTlsLocal {
            verify: Some(false),
            ..Default::default()
        };
        let r = resolve_outbound_tls(&local, None, None, true, "Test").expect("verify=false must be honored");
        assert!(
            !r.verify,
            "resolved verify must be false when operator sets verify=false"
        );
    }

    #[test]
    fn gate_rejects_skip_when_provider_cannot_disable() {
        let local = OutboundTlsLocal {
            verify: Some(false),
            ..Default::default()
        };
        assert!(resolve_outbound_tls(&local, None, None, false, "Redis").is_err());
    }

    #[test]
    fn inherits_global_verify_true_when_local_none() {
        set_outbound_global_default(OutboundTlsConfig {
            verify: true,
            ..Default::default()
        });
        let r = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test").unwrap();
        assert!(r.verify);
    }

    /// When `caSecretRef` is set but `caSource` is absent, the resolver must
    /// infer `CaSource::SecretRef` and read the CA from `resolved_ca`.
    /// This guards the case where an operator provides a K8s Secret reference
    /// without an explicit discriminator.
    #[test]
    fn ca_secret_ref_without_ca_source_infers_secret_ref() {
        use edgion_resources::resources::common::SecretObjectReference;
        use k8s_openapi::ByteString;
        use std::collections::BTreeMap;

        set_outbound_global_default(OutboundTlsConfig::default());

        let ca_pem = b"-----BEGIN CERTIFICATE-----\nMIIBdummy\n-----END CERTIFICATE-----\n";
        let mut data = BTreeMap::new();
        data.insert(tls_keys::CA_CERT.to_string(), ByteString(ca_pem.to_vec()));
        let resolved_ca = k8s_openapi::api::core::v1::Secret {
            data: Some(data),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        };

        let local = OutboundTlsLocal {
            ca_secret_ref: Some(SecretObjectReference {
                name: "my-ca-secret".to_string(),
                group: None,
                kind: None,
                namespace: None,
            }),
            ca_source: None,
            ..Default::default()
        };

        let result =
            resolve_outbound_tls(&local, Some(&resolved_ca), None, true, "Test").expect("should resolve successfully");
        assert!(
            matches!(result.ca, EffectiveCa::Custom(_)),
            "expected EffectiveCa::Custom when caSecretRef is set and ca_source is None"
        );
    }

    /// A global `caSource: SecretRef` with a resolved CA Secret must let an
    /// inheriting call (no local CA) resolve `EffectiveCa::Custom` from the global
    /// resolved Secret.
    #[test]
    fn global_secretref_resolves_ca_from_global_secret() {
        use k8s_openapi::ByteString;
        use std::collections::BTreeMap;

        let ca_pem = b"-----BEGIN CERTIFICATE-----\nMIIBglobal\n-----END CERTIFICATE-----\n";
        let mut data = BTreeMap::new();
        data.insert(tls_keys::CA_CERT.to_string(), ByteString(ca_pem.to_vec()));
        let global_ca = Secret {
            data: Some(data),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        };

        set_outbound_global_default(OutboundTlsConfig {
            ca_source: Some(CaSource::SecretRef),
            resolved_ca_secret: Some(global_ca),
            ..Default::default()
        });

        let result = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test")
            .expect("global SecretRef should resolve");
        assert!(
            matches!(result.ca, EffectiveCa::Custom(_)),
            "expected EffectiveCa::Custom from the global resolved CA Secret"
        );

        // Reset the global default so other tests are unaffected.
        set_outbound_global_default(OutboundTlsConfig::default());
    }

    /// When `ca_cert` (inline PEM) is set but `caSource` is absent, the resolver
    /// must infer `CaSource::Inline` and return `EffectiveCa::Custom`.
    #[test]
    fn ca_cert_without_ca_source_infers_inline() {
        set_outbound_global_default(OutboundTlsConfig::default());

        let ca_pem = "-----BEGIN CERTIFICATE-----\nMIIBinline\n-----END CERTIFICATE-----\n";
        let local = OutboundTlsLocal {
            ca_cert: Some(ca_pem.to_string()),
            ca_source: None,
            ..Default::default()
        };

        let result = resolve_outbound_tls(&local, None, None, true, "Test").expect("should resolve successfully");
        assert!(
            matches!(result.ca, EffectiveCa::Custom(_)),
            "expected EffectiveCa::Custom when ca_cert is set and ca_source is None"
        );
    }
}
