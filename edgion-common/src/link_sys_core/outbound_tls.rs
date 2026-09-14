//! Shared outbound TLS resolver for all outbound callers (LinkSys providers,
//! plugins, etc.).
//!
//! Provides a single entry point [`resolve_outbound_tls`] that:
//! - inherits the cluster-wide default from [`outbound_global_default`]
//! - applies per-call overrides from [`OutboundTlsLocal`]
//! - resolves CA material from resolved Kubernetes Secrets (one per ref)

use std::sync::LazyLock;

use edgion_resources::constants::secret_keys::tls as tls_keys;
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::{OutboundTlsConfig, OutboundTlsLocal};
use edgion_resources::EdgionErrorCode;
use k8s_openapi::api::core::v1::Secret;
use parking_lot::RwLock;
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

#[cfg(test)]
static OUTBOUND_TLS_TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Serializes tests that resolve the process-wide outbound TLS default and
/// gives each test a clean default without leaking mutations to the next test.
#[cfg(test)]
pub(crate) struct OutboundTlsTestGuard {
    _lock: parking_lot::MutexGuard<'static, ()>,
    previous: OutboundTlsConfig,
}

#[cfg(test)]
impl Drop for OutboundTlsTestGuard {
    fn drop(&mut self) {
        set_outbound_global_default(std::mem::take(&mut self.previous));
    }
}

#[cfg(test)]
pub(crate) fn outbound_tls_test_guard() -> OutboundTlsTestGuard {
    let lock = OUTBOUND_TLS_TEST_MUTEX.lock();
    let previous = outbound_global_default();
    set_outbound_global_default(OutboundTlsConfig::default());
    OutboundTlsTestGuard { _lock: lock, previous }
}

/// Returns the cluster-wide outbound TLS default.
///
/// Initialized to `OutboundTlsConfig::default()` (`verify = true`, no CA
/// override). Updated by [`set_outbound_global_default`] when an
/// `EdgionGatewayConfig` is applied.
pub fn outbound_global_default() -> OutboundTlsConfig {
    OUTBOUND_GLOBAL_DEFAULT.read().clone()
}

/// Replace the cluster-wide outbound TLS default.
///
/// Called by the Gateway when an `EdgionGatewayConfig` carrying an
/// `outboundTls` block is applied. Returns `true` when the effective stored
/// configuration changed.
pub fn set_outbound_global_default(cfg: OutboundTlsConfig) -> bool {
    let mut current = OUTBOUND_GLOBAL_DEFAULT.write();
    if *current == cfg {
        return false;
    }
    *current = cfg;
    true
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
/// - `resolved_ca` — the resolved K8s Secrets for the **local**
///   `validation.caCertificateRefs` (one Secret per ref, in the same order).
///   Required when `local.validation` declares CA refs; there is no fallback to
///   the global bundles for a local block (see the precedence section below).
///   Ignored when the local block declares no `validation` (the global block
///   then supplies both refs and resolved bytes).
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
/// # Validation-block precedence and global CA fallback
///
/// The `validation` block has **whole-block** precedence: a present
/// `local.validation` shadows the global block entirely (fields are not merged).
/// CA bytes are sourced from whichever block wins:
/// - local block wins → CA bytes come **only** from the per-call `resolved_ca`.
///   If the local block declares `caCertificateRefs` but `resolved_ca` is
///   `None`/empty, this fails with [`EdgionErrorCode::OutboundTlsCaMissing`] rather
///   than silently falling back to the global bundles (which belong to the
///   global refs, not the local ones).
/// - global block wins (no local `validation`) → CA bytes come from the
///   Controller-shipped `global.resolved_ca_certificates`.
///
/// Client-certificate inheritance follows the same ownership rule independently
/// of the validation block: a local `clientCertificateRef` uses only the
/// per-call `resolved_client`; otherwise a configured global ref uses only
/// `global.resolved_client_certificate`. A declared ref without its matching
/// resolved Secret fails closed instead of silently disabling mTLS.
///
/// # Fail-secure: SAN pinning overrides verify=false
///
/// A non-empty `validation.subjectAltNames` policy forces full certificate
/// verification even when the operator set `verify=false` — SAN pinning is
/// meaningless without chain verification. In that case the returned `verify`
/// is `true`, CA material is resolved, and the client mTLS identity is retained.
///
/// # Known limitations
///
/// - M4: When the effective `verify` is `false` (skip-verify actually in force,
///   i.e. no SAN policy raised it), client mTLS identity is intentionally
///   dropped — the skip-verify path sends no client certificate.
///
/// # Operator responsibility for verify=false
///
/// When an operator explicitly sets `verify=false` (and no SAN policy overrides
/// it), it is honored without an environment check — the operator owns this
/// decision. A WARN is emitted at configuration-load time as an audit trail.
/// `verify=false` is rejected ONLY when `provider_can_disable == false` (a
/// technical SDK-capability fact: e.g. etcd-client and the rustls Redis
/// connector have no skip-verify API, so honoring the request would silently
/// still verify).
///
/// # Errors
///
/// - [`EdgionErrorCode::OutboundTlsVerifyUnsupported`] — `verify=false` requested
///   but `provider_can_disable == false`.
/// - [`EdgionErrorCode::OutboundTlsCaMissing`] — `caCertificateRefs` declared but
///   no resolved Secret carries a valid CA PEM bundle.
/// - [`EdgionErrorCode::OutboundTlsClientCertInvalid`] — a declared client ref
///   has no matching resolved Secret, or the effective Secret is missing
///   `tls.crt` or `tls.key`.
pub fn resolve_outbound_tls(
    local: &OutboundTlsLocal,
    resolved_ca: Option<&[Secret]>,
    resolved_client: Option<&Secret>,
    provider_can_disable: bool,
    context: &str,
) -> Result<ResolvedOutboundTls, EdgionErrorCode> {
    let global = outbound_global_default();
    let result = resolve_outbound_tls_with_defaults(local, resolved_ca, resolved_client, provider_can_disable, &global);
    match &result {
        Err(EdgionErrorCode::OutboundTlsVerifyUnsupported) => {
            tracing::warn!(
                provider = context,
                "outbound TLS verify=false requested but provider does not support skip-verify"
            );
        }
        Ok(resolved) if !resolved.verify => {
            tracing::warn!(
                provider = context,
                "outbound TLS verification disabled (verify=false) — operator-configured, insecure"
            );
        }
        Ok(_) if !local.verify.unwrap_or(global.verify) => {
            tracing::warn!(
                provider = context,
                "outbound TLS verify=false overridden by subjectAltNames policy — full certificate verification enforced (fail-secure)"
            );
        }
        _ => {}
    }
    result
}

/// Resolve outbound TLS against an explicit defaults snapshot.
///
/// Backend-policy consumers pass empty defaults to avoid inheriting unrelated
/// LinkSys trust or client credentials. Ordinary outbound clients use
/// `resolve_outbound_tls` to retain process-default inheritance.
/// This pure resolver emits no logs; request-path callers own their diagnostics.
pub fn resolve_outbound_tls_with_defaults(
    local: &OutboundTlsLocal,
    resolved_ca: Option<&[Secret]>,
    resolved_client: Option<&Secret>,
    provider_can_disable: bool,
    global: &OutboundTlsConfig,
) -> Result<ResolvedOutboundTls, EdgionErrorCode> {
    // Whole-block precedence: a present local `validation` shadows the global
    // block entirely (fields are not merged). CA bytes are sourced from whichever
    // block wins — the per-call `resolved_ca` for a local block, or the
    // Controller-shipped global bundles when inheriting the global block. This
    // deliberately does NOT fall back from local to global CA: the global
    // resolved bundles correspond to the global refs, not the local ones.
    let (validation, resolved_ca_effective): (Option<&OutboundTlsValidation>, Option<&[Secret]>) =
        if local.validation.is_some() {
            (local.validation.as_ref(), resolved_ca)
        } else {
            (global.validation.as_ref(), global.resolved_ca_certificates.as_deref())
        };
    let (client_ref_configured, resolved_client_effective) = if local.client_certificate_ref.is_some() {
        (true, resolved_client)
    } else if global.client_certificate_ref.is_some() {
        (true, global.resolved_client_certificate.as_ref())
    } else {
        // Preserve direct programmatic callers that supply already-resolved
        // identity material without an operator-facing reference.
        (false, resolved_client)
    };

    // Fail-secure: a non-empty subjectAltNames policy forces full verification
    // even if the operator set verify=false — SAN pinning is meaningless without
    // chain verification, so it overrides the skip-verify request.
    let san_pinned = validation
        .and_then(|v| v.subject_alt_names.as_ref())
        .is_some_and(|s| !s.is_empty());
    let requested_verify = local.verify.unwrap_or(global.verify);
    let verify = requested_verify || san_pinned;

    if client_ref_configured && resolved_client_effective.is_none() {
        return Err(EdgionErrorCode::OutboundTlsClientCertInvalid);
    }

    if !verify {
        if !provider_can_disable {
            return Err(EdgionErrorCode::OutboundTlsVerifyUnsupported);
        }
        // Operator explicitly disabled verification; honored as their responsibility.
        return Ok(ResolvedOutboundTls {
            verify: false,
            ca: EffectiveCa::System,
            client_identity: None,
            san: validation.cloned(),
        });
    }

    // verify == true: resolve CA source from the winning validation block.
    let ca = match validation {
        Some(v) if v.ca_certificate_refs.as_ref().is_some_and(|r| !r.is_empty()) => {
            // CA bytes must come from the block that won precedence above; no
            // cross-block fallback (see the tuple selection comment).
            let refs = v
                .ca_certificate_refs
                .as_ref()
                .ok_or(EdgionErrorCode::OutboundTlsCaMissing)?;
            let secrets: &[Secret] = resolved_ca_effective
                .filter(|s| !s.is_empty())
                .ok_or(EdgionErrorCode::OutboundTlsCaMissing)?;
            if secrets.len() != refs.len() {
                return Err(EdgionErrorCode::OutboundTlsCaMissing);
            }
            let bundles: Vec<Vec<u8>> = secrets
                .iter()
                .map(|secret| {
                    secret
                        .data
                        .as_ref()
                        .and_then(|data| data.get(tls_keys::CA_CERT))
                        .map(|value| value.0.clone())
                        .ok_or(EdgionErrorCode::OutboundTlsCaMissing)
                })
                .collect::<Result<_, _>>()?;
            EffectiveCa::Custom(bundles)
        }
        // wellKnownCACertificates: System, or both absent → public roots.
        _ => EffectiveCa::System,
    };

    // Resolve optional client mTLS identity
    let client_identity = match resolved_client_effective {
        Some(secret) => {
            let cert = secret
                .data
                .as_ref()
                .and_then(|d| d.get(tls_keys::CERT))
                .map(|v| v.0.clone())
                .ok_or(EdgionErrorCode::OutboundTlsClientCertInvalid)?;
            let key = secret
                .data
                .as_ref()
                .and_then(|d| d.get(tls_keys::KEY))
                .map(|v| Zeroizing::new(v.0.clone()))
                .ok_or(EdgionErrorCode::OutboundTlsClientCertInvalid)?;
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
    use edgion_resources::resources::backend_tls_policy::{
        BackendTLSPolicyCACertificateRef, SubjectAltName, SubjectAltNameType, WellKnownCACertificates,
    };
    use edgion_resources::resources::common::SecretObjectReference;
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

    fn client_secret() -> Secret {
        let mut data = BTreeMap::new();
        data.insert(tls_keys::CERT.to_string(), ByteString(b"cert-pem".to_vec()));
        data.insert(tls_keys::KEY.to_string(), ByteString(b"key-pem".to_vec()));
        Secret {
            data: Some(data),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        }
    }

    fn secret_ref(name: &str) -> SecretObjectReference {
        SecretObjectReference {
            group: None,
            kind: None,
            name: name.to_string(),
            namespace: None,
        }
    }

    fn hostname_san(h: &str) -> SubjectAltName {
        SubjectAltName {
            san_type: SubjectAltNameType::Hostname,
            hostname: Some(h.to_string()),
            uri: None,
        }
    }

    #[test]
    fn aggregate_verify_safest_wins() {
        let a = OutboundTlsConfig {
            verify: false,
            ..Default::default()
        };
        let b = OutboundTlsConfig::default(); // verify = true
        assert!(aggregate_verify(&[a.clone(), b]).verify);
        let c = OutboundTlsConfig {
            verify: false,
            ..Default::default()
        };
        assert!(!aggregate_verify(&[a, c]).verify);
    }

    #[test]
    fn setting_global_default_reports_only_effective_changes() {
        let _guard = outbound_tls_test_guard();
        assert!(!set_outbound_global_default(OutboundTlsConfig::default()));
        assert!(set_outbound_global_default(OutboundTlsConfig {
            verify: false,
            ..Default::default()
        }));
        assert!(!set_outbound_global_default(OutboundTlsConfig {
            verify: false,
            ..Default::default()
        }));
    }

    #[test]
    fn verify_false_is_honored_when_provider_can_disable() {
        let _guard = outbound_tls_test_guard();
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
        let _guard = outbound_tls_test_guard();
        let local = OutboundTlsLocal {
            verify: Some(false),
            ..Default::default()
        };
        assert!(resolve_outbound_tls(&local, None, None, false, "Redis").is_err());
    }

    #[test]
    fn inherits_global_verify_true_when_local_none() {
        let _guard = outbound_tls_test_guard();
        set_outbound_global_default(OutboundTlsConfig {
            verify: true,
            ..Default::default()
        });
        let r = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test").unwrap();
        assert!(r.verify);
    }

    #[test]
    fn ca_refs_present_yields_multi_bundle_custom() {
        let _guard = outbound_tls_test_guard();
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
        let _guard = outbound_tls_test_guard();
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
        let _guard = outbound_tls_test_guard();
        set_outbound_global_default(OutboundTlsConfig::default());
        let r = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test").unwrap();
        assert!(matches!(r.ca, EffectiveCa::System));
    }

    /// Fail-secure: a non-empty subjectAltNames policy overrides verify=false, so
    /// the effective `verify` becomes true and the client mTLS identity is retained
    /// (rather than dropped by the skip-verify path).
    #[test]
    fn san_policy_overrides_verify_false_and_keeps_client_identity() {
        let _guard = outbound_tls_test_guard();
        set_outbound_global_default(OutboundTlsConfig::default());
        let local = OutboundTlsLocal {
            verify: Some(false),
            validation: Some(OutboundTlsValidation {
                subject_alt_names: Some(vec![hostname_san("idp.example.com")]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let r = resolve_outbound_tls(&local, None, Some(&client_secret()), true, "Test").unwrap();
        assert!(r.verify, "SAN policy must force verify=true (fail-secure)");
        assert!(
            r.client_identity.is_some(),
            "client identity must be retained when SAN raises verify"
        );
    }

    /// verify=false with no SAN policy is a genuine skip-verify path: client
    /// identity is dropped (M4).
    #[test]
    fn verify_false_without_san_drops_client_identity() {
        let _guard = outbound_tls_test_guard();
        set_outbound_global_default(OutboundTlsConfig::default());
        let local = OutboundTlsLocal {
            verify: Some(false),
            ..Default::default()
        };
        let r = resolve_outbound_tls(&local, None, Some(&client_secret()), true, "Test").unwrap();
        assert!(!r.verify);
        assert!(
            r.client_identity.is_none(),
            "skip-verify path must drop client identity"
        );
    }

    /// A local validation block declaring caCertificateRefs but with no per-call
    /// resolved bytes must fail with OutboundTlsCaMissing — it must NOT silently
    /// fall back to the global resolved CA (which belongs to the global refs).
    #[test]
    fn local_ca_refs_without_resolved_bytes_errors_no_global_fallback() {
        let _guard = outbound_tls_test_guard();
        // Global has its own resolved CA that must NOT be borrowed by the local block.
        set_outbound_global_default(OutboundTlsConfig {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![cert_ref("Secret", "global-ca")]),
                ..Default::default()
            }),
            resolved_ca_certificates: Some(vec![ca_secret(
                b"-----BEGIN CERTIFICATE-----\nGLOBAL\n-----END CERTIFICATE-----\n",
            )]),
            ..Default::default()
        });
        let local = OutboundTlsLocal {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![cert_ref("Secret", "local-ca")]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let r = resolve_outbound_tls(&local, None, None, true, "Test");
        assert!(
            matches!(r, Err(EdgionErrorCode::OutboundTlsCaMissing)),
            "local CA refs with no resolved bytes must error, got: {:?}",
            r.map(|_| "Ok(resolved)")
        );
    }

    #[test]
    fn partially_resolved_ca_refs_fail_closed() {
        let _guard = outbound_tls_test_guard();
        set_outbound_global_default(OutboundTlsConfig::default());
        let local = OutboundTlsLocal {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![cert_ref("Secret", "ca-a"), cert_ref("Secret", "ca-b")]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let only_one_secret = [ca_secret(
            b"-----BEGIN CERTIFICATE-----\nA\n-----END CERTIFICATE-----\n",
        )];

        let result = resolve_outbound_tls(&local, Some(&only_one_secret), None, true, "Test");
        assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsCaMissing)));
    }

    /// When the local block declares no validation, the global validation block
    /// wins and its Controller-shipped resolved CA bundles are used.
    #[test]
    fn inherits_global_ca_when_local_validation_absent() {
        let _guard = outbound_tls_test_guard();
        set_outbound_global_default(OutboundTlsConfig {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![cert_ref("Secret", "global-ca")]),
                ..Default::default()
            }),
            resolved_ca_certificates: Some(vec![ca_secret(
                b"-----BEGIN CERTIFICATE-----\nGLOBAL\n-----END CERTIFICATE-----\n",
            )]),
            ..Default::default()
        });
        let r = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test").unwrap();
        match r.ca {
            EffectiveCa::Custom(b) => assert_eq!(b.len(), 1),
            _ => panic!("expected global resolved CA to be inherited as Custom"),
        }
    }

    #[test]
    fn local_client_ref_without_resolved_secret_fails_closed() {
        let _guard = outbound_tls_test_guard();
        set_outbound_global_default(OutboundTlsConfig::default());
        let local = OutboundTlsLocal {
            client_certificate_ref: Some(secret_ref("local-client")),
            ..Default::default()
        };

        let result = resolve_outbound_tls(&local, None, None, true, "Test");
        assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsClientCertInvalid)));
    }

    #[test]
    fn global_client_ref_without_resolved_secret_fails_closed() {
        let _guard = outbound_tls_test_guard();
        set_outbound_global_default(OutboundTlsConfig {
            client_certificate_ref: Some(secret_ref("global-client")),
            ..Default::default()
        });

        let result = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test");
        assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsClientCertInvalid)));
        set_outbound_global_default(OutboundTlsConfig::default());
    }

    #[test]
    fn explicit_defaults_do_not_inherit_process_client_identity() {
        let _guard = outbound_tls_test_guard();
        set_outbound_global_default(OutboundTlsConfig {
            client_certificate_ref: Some(secret_ref("global-client")),
            resolved_client_certificate: Some(client_secret()),
            ..Default::default()
        });
        let resolved = resolve_outbound_tls_with_defaults(
            &OutboundTlsLocal::default(),
            None,
            None,
            false,
            &OutboundTlsConfig::default(),
        )
        .expect("explicit defaults must resolve independently");
        assert!(resolved.verify);
        assert!(matches!(resolved.ca, EffectiveCa::System));
        assert!(resolved.client_identity.is_none());
    }

    #[test]
    fn global_client_identity_is_inherited_when_local_ref_is_absent() {
        let _guard = outbound_tls_test_guard();
        set_outbound_global_default(OutboundTlsConfig {
            client_certificate_ref: Some(secret_ref("global-client")),
            resolved_client_certificate: Some(client_secret()),
            ..Default::default()
        });

        let resolved = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test")
            .expect("global client identity must resolve");
        assert!(resolved.client_identity.is_some());
        set_outbound_global_default(OutboundTlsConfig::default());
    }

    #[test]
    fn local_client_ref_does_not_borrow_global_resolved_identity() {
        let _guard = outbound_tls_test_guard();
        set_outbound_global_default(OutboundTlsConfig {
            client_certificate_ref: Some(secret_ref("global-client")),
            resolved_client_certificate: Some(client_secret()),
            ..Default::default()
        });
        let local = OutboundTlsLocal {
            client_certificate_ref: Some(secret_ref("local-client")),
            ..Default::default()
        };

        let result = resolve_outbound_tls(&local, None, None, true, "Test");
        assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsClientCertInvalid)));
        set_outbound_global_default(OutboundTlsConfig::default());
    }
}
