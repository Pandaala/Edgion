//! Shared outbound TLS resolver for all outbound callers (LinkSys providers,
//! plugins, etc.).
//!
//! Provides a single entry point [`resolve_outbound_tls`] that:
//! - inherits the cluster-wide default from [`outbound_global_default`]
//! - applies per-call overrides from [`OutboundTlsLocal`]
//! - resolves CA material from resolved Kubernetes Secrets (one per ref)

use std::sync::{LazyLock, RwLock};

use edgion_resources::constants::secret_keys::tls as tls_keys;
use edgion_resources::edgion_status::EdgionStatus;
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::{OutboundTlsConfig, OutboundTlsLocal};
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
    /// PEM bundles that REPLACE the public roots (private CA mode).
    /// Each inner `Vec<u8>` is one complete PEM bundle (may contain multiple
    /// certificates).
    Custom(Vec<Vec<u8>>),
}

/// Resolved outbound TLS configuration, ready for consumption by a provider.
pub struct ResolvedOutboundTls {
    pub verify: bool,
    pub ca: EffectiveCa,
    /// `(cert_pem, key_pem)` when a client-identity Secret was provided.
    pub client_identity: Option<(Vec<u8>, Zeroizing<Vec<u8>>)>,
    /// Carries the resolved `OutboundTlsValidation` for Phase 2 SNI/SAN
    /// enforcement. Unused by the data plane in Phase 1.
    pub san: Option<OutboundTlsValidation>,
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
/// - `resolved_ca` — the resolved K8s Secrets for `validation.caCertificateRefs`
///   (one Secret per ref, in the same order). Pass `None` or `Some(&[])` when
///   no per-call CA is available; the resolver falls back to the global resolved
///   CA certificates from the cluster-wide default.
/// - `resolved_client` — the resolved K8s Secret for client mTLS identity.
/// - `provider_can_disable` — whether this provider exposes a skip-verify knob.
///   Pass `false` for providers like Etcd whose TLS stack always verifies.
/// - `context` — short provider name embedded in log messages.
///
/// # CA source determination
///
/// When `verify == true`, the CA source is determined from the `validation` block
/// (local takes precedence over global):
/// - `caCertificateRefs` present and non-empty → `EffectiveCa::Custom` (multi-bundle)
///   using the resolved Secrets; fails with `OutboundTlsCaMissing` if no resolved
///   Secret carries a valid `tls.ca.crt` PEM.
/// - `wellKnownCACertificates: System` or both absent → `EffectiveCa::System`
///   (built-in webpki roots).
///
/// # Global CA fallback
///
/// An inheriting call (no per-call `resolved_ca`) falls back to the global
/// resolved CA certificates when `validation.caCertificateRefs` is declared at
/// the global level and the Controller has shipped the resolved bundles.
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
/// - [`EdgionStatus::OutboundTlsCaMissing`] — `caCertificateRefs` declared but
///   no resolved Secret carries a valid CA PEM bundle.
/// - [`EdgionStatus::OutboundTlsClientCertInvalid`] — `resolved_client` is
///   `Some` but the Secret is missing `tls.crt` or `tls.key`.
pub fn resolve_outbound_tls(
    local: &OutboundTlsLocal,
    resolved_ca: Option<&[Secret]>,
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
        // Preserve any SAN policy even when verify=false so that downstream
        // TLS builders can enforce SAN pinning regardless of the skip-verify
        // flag (fail-secure: a non-empty SAN policy overrides verify=false).
        let san = local.validation.as_ref().or(global.validation.as_ref()).cloned();
        return Ok(ResolvedOutboundTls {
            verify: false,
            ca: EffectiveCa::System,
            client_identity: None,
            san,
        });
    }

    // verify == true: resolve CA source from the validation block.
    // Local validation takes precedence over global.
    let validation = local.validation.as_ref().or(global.validation.as_ref());
    let ca = match validation {
        Some(v) if v.ca_certificate_refs.as_ref().is_some_and(|r| !r.is_empty()) => {
            // Prefer per-call resolved bundles; fall back to the global resolved
            // bundles so an inheriting call (no local CA) uses the cluster-wide
            // default that the Controller resolved and shipped.
            let secrets: &[Secret] = resolved_ca
                .filter(|s| !s.is_empty())
                .or(global.resolved_ca_certificates.as_deref())
                .ok_or(EdgionStatus::OutboundTlsCaMissing)?;
            let bundles: Vec<Vec<u8>> = secrets
                .iter()
                .filter_map(|s| s.data.as_ref()?.get(tls_keys::CA_CERT).map(|v| v.0.clone()))
                .collect();
            if bundles.is_empty() {
                return Err(EdgionStatus::OutboundTlsCaMissing);
            }
            EffectiveCa::Custom(bundles)
        }
        // wellKnownCACertificates: System, or both absent → public roots.
        _ => EffectiveCa::System,
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
        san: validation.cloned(),
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::backend_tls_policy::{BackendTLSPolicyCACertificateRef, WellKnownCACertificates};
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    fn cert_ref(kind: &str, name: &str) -> BackendTLSPolicyCACertificateRef {
        BackendTLSPolicyCACertificateRef {
            group: String::new(),
            kind: kind.into(),
            name: name.into(),
            namespace: None,
        }
    }

    fn ca_secret(pem: &[u8]) -> Secret {
        let mut data = BTreeMap::new();
        data.insert(tls_keys::CA_CERT.to_string(), ByteString(pem.to_vec()));
        Secret {
            data: Some(data),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        }
    }

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

    #[test]
    fn ca_refs_present_yields_multi_bundle_custom() {
        set_outbound_global_default(OutboundTlsConfig::default());
        let s1 = ca_secret(b"-----BEGIN CERTIFICATE-----\nA\n-----END CERTIFICATE-----\n");
        let s2 = ca_secret(b"-----BEGIN CERTIFICATE-----\nB\n-----END CERTIFICATE-----\n");
        let local = OutboundTlsLocal {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![cert_ref("Secret", "a"), cert_ref("Secret", "b")]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let r = resolve_outbound_tls(&local, Some(&[s1, s2]), None, true, "Test").unwrap();
        match r.ca {
            EffectiveCa::Custom(b) => assert_eq!(b.len(), 2),
            _ => panic!("expected multi-bundle Custom"),
        }
    }

    #[test]
    fn well_known_system_yields_system() {
        set_outbound_global_default(OutboundTlsConfig::default());
        let local = OutboundTlsLocal {
            validation: Some(OutboundTlsValidation {
                well_known_ca_certificates: Some(WellKnownCACertificates::System),
                ..Default::default()
            }),
            ..Default::default()
        };
        let r = resolve_outbound_tls(&local, None, None, true, "Test").unwrap();
        assert!(matches!(r.ca, EffectiveCa::System));
    }

    #[test]
    fn both_absent_defaults_to_system_public_roots() {
        set_outbound_global_default(OutboundTlsConfig::default());
        let r = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test").unwrap();
        assert!(matches!(r.ca, EffectiveCa::System));
    }
}
