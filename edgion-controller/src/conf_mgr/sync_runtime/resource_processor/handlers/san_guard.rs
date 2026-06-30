//! SAN-pinning guard: reject outbound TLS `subjectAltNames`/`hostname`
//! on providers where enforcement is not yet implemented.
//!
//! # Design
//!
//! SAN pinning (verifying the peer certificate against an operator-supplied
//! list of Subject Alternative Names) is a Phase 2+ feature. Phase 1 exposes
//! the `validation.subjectAltNames` / `validation.hostname` schema fields on
//! all outbound-TLS-capable providers so operators can write the config ahead
//! of time, but the data-plane verifier is not wired yet.
//!
//! If an operator configures SAN pinning on an unsupported provider, the config
//! would be silently ignored, which is worse than a clear rejection. This module
//! surfaces that case as an `Accepted=False` validation error (via the standard
//! `preparse` error-string mechanism) so the operator learns immediately.
//!
//! # Phase 2+ enablement
//!
//! As each provider's verifier lands, flip the corresponding `SAN_ENFORCED_*`
//! constant to `true` and redeploy. The guard `san_pinning_requested(v) && !SAN_ENFORCED_X`
//! short-circuits, so the error disappears without any other code change.
//!
//! Each constant is intentionally per-provider so different providers can be
//! rolled out independently (e.g., oidc/ldap in Phase 2, ext_proc/link_sys/egc later).

use edgion_resources::prelude_resources::{EdgionGatewayConfig, EdgionPlugins, LinkSys};
use edgion_resources::resources::common::OutboundTlsValidation;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::link_sys::SystemConfig;

// ============================================================================
// Per-provider enforcement gates (Phase 1: all false)
// ============================================================================

/// Phase 2: SAN pinning is enforced for the OpenidConnect plugin.
/// OIDC reqwest path uses use_preconfigured_tls + OutboundSanVerifier.
pub const SAN_ENFORCED_OIDC: bool = true;

/// Phase 2: SAN pinning is enforced for the LdapAuth plugin.
/// LDAP rustls path uses OutboundSanVerifier wrap.
pub const SAN_ENFORCED_LDAP: bool = true;

/// Phase 1: SAN pinning is NOT enforced for the ExtProc plugin.
/// Phase 2 will flip this to `true` once the gRPC TLS verifier is wired.
pub const SAN_ENFORCED_EXT_PROC: bool = false;

/// Phase 1: SAN pinning is NOT enforced for LinkSys providers.
/// Phase 2 will flip this (or split per-provider) when verifiers land.
pub const SAN_ENFORCED_LINK_SYS: bool = false;

/// Phase 1: SAN pinning is NOT enforced for EdgionGatewayConfig outbound TLS.
/// Phase 2 will flip this to `true` once the EGC global TLS verifier is wired.
pub const SAN_ENFORCED_EGC: bool = false;

// ============================================================================
// Sentinel predicate
// ============================================================================

/// Returns `true` iff `validation` expresses SAN pinning intent:
/// a non-empty `subjectAltNames` list **or** a non-None `hostname`.
///
/// Used by the Phase 1 guard: callers check
/// `san_pinning_requested(v) && !SAN_ENFORCED_X` to decide whether to emit
/// an `Accepted=False` validation error. Phase 2 flips the const to `true` to
/// lift the guard without any structural change here.
pub fn san_pinning_requested(validation: Option<&OutboundTlsValidation>) -> bool {
    validation.is_some_and(|v| v.subject_alt_names.as_ref().is_some_and(|s| !s.is_empty()) || v.hostname.is_some())
}

// ============================================================================
// Canonical error message
// ============================================================================

fn san_not_supported_msg(provider: &str) -> String {
    format!(
        "outbound TLS subjectAltNames/hostname is not supported for {} in Phase 1 \
         (SAN pinning enforcement deferred to Phase 2)",
        provider
    )
}

// ============================================================================
// Per-resource-type check functions
// ============================================================================

/// Scan all plugin stages in `ep` for OIDC / LdapAuth / ExtProc entries that
/// declare SAN pinning intent while the corresponding enforcement gate is off.
///
/// Returns one error string per offending plugin entry.
pub fn check_plugins_san(ep: &EdgionPlugins) -> Vec<String> {
    let mut errors = Vec::new();

    macro_rules! scan_stage {
        ($stage:expr) => {
            if let Some(entries) = &$stage {
                for entry in entries {
                    if !entry.is_enabled() {
                        continue;
                    }
                    match &entry.plugin {
                        EdgionPlugin::OpenidConnect(config)
                            if !SAN_ENFORCED_OIDC && san_pinning_requested(config.tls.validation.as_ref()) =>
                        {
                            errors.push(san_not_supported_msg("OpenidConnect"));
                        }
                        EdgionPlugin::LdapAuth(config)
                            if !SAN_ENFORCED_LDAP && san_pinning_requested(config.tls.validation.as_ref()) =>
                        {
                            errors.push(san_not_supported_msg("LdapAuth"));
                        }
                        EdgionPlugin::ExtProc(config)
                            if !SAN_ENFORCED_EXT_PROC
                                && san_pinning_requested(config.grpc_service.tls.outbound.validation.as_ref()) =>
                        {
                            errors.push(san_not_supported_msg("ExtProc"));
                        }
                        _ => {}
                    }
                }
            }
        };
    }

    scan_stage!(ep.spec.request_plugins);
    // ExtProc also runs in the upstream_response stage; scan it too.
    scan_stage!(ep.spec.upstream_response_plugins);

    errors
}

/// Check the LinkSys provider's outbound TLS validation block for SAN pinning
/// intent while `SAN_ENFORCED_LINK_SYS` is false.
///
/// Returns at most one error string (per-resource, not per-endpoint).
pub fn check_link_sys_san(ls: &LinkSys) -> Vec<String> {
    if SAN_ENFORCED_LINK_SYS {
        return vec![];
    }

    // Extract the outbound validation block from whichever provider is configured.
    let validation_opt: Option<&OutboundTlsValidation> = match &ls.spec.config {
        SystemConfig::Redis(c) => c.tls.as_ref().and_then(|t| t.outbound.validation.as_ref()),
        SystemConfig::Etcd(c) => c.tls.as_ref().and_then(|t| t.outbound.validation.as_ref()),
        SystemConfig::Elasticsearch(c) => c.tls.as_ref().and_then(|t| t.outbound.validation.as_ref()),
        SystemConfig::Kafka(c) => c.tls.as_ref().and_then(|t| t.outbound.validation.as_ref()),
        SystemConfig::Webhook(c) => c.conn.tls.as_ref().and_then(|t| t.outbound.validation.as_ref()),
    };

    if san_pinning_requested(validation_opt) {
        let provider = match &ls.spec.config {
            SystemConfig::Redis(_) => "Redis",
            SystemConfig::Etcd(_) => "Etcd",
            SystemConfig::Elasticsearch(_) => "Elasticsearch",
            SystemConfig::Kafka(_) => "Kafka",
            SystemConfig::Webhook(_) => "Webhook",
        };
        vec![san_not_supported_msg(provider)]
    } else {
        vec![]
    }
}

/// Check the EdgionGatewayConfig global outbound TLS validation block for SAN
/// pinning intent while `SAN_ENFORCED_EGC` is false.
///
/// Returns at most one error string.
pub fn check_egc_san(egc: &EdgionGatewayConfig) -> Vec<String> {
    if SAN_ENFORCED_EGC {
        return vec![];
    }

    let validation_opt = egc.spec.outbound_tls.as_ref().and_then(|ob| ob.validation.as_ref());

    if san_pinning_requested(validation_opt) {
        vec![san_not_supported_msg("EdgionGatewayConfig")]
    } else {
        vec![]
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::backend_tls_policy::SubjectAltName;
    use edgion_resources::resources::common::OutboundTlsValidation;

    // ── san_pinning_requested ─────────────────────────────────────────────────

    #[test]
    fn no_validation_block_is_not_pinning() {
        assert!(!san_pinning_requested(None));
    }

    #[test]
    fn empty_validation_block_is_not_pinning() {
        let v = OutboundTlsValidation {
            ca_certificate_refs: None,
            well_known_ca_certificates: None,
            hostname: None,
            subject_alt_names: None,
        };
        assert!(!san_pinning_requested(Some(&v)));
    }

    #[test]
    fn only_ca_refs_is_not_pinning() {
        use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
        let v = OutboundTlsValidation {
            ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                kind: "Secret".to_string(),
                name: "my-ca".to_string(),
                namespace: None,
                ..Default::default()
            }]),
            well_known_ca_certificates: None,
            hostname: None,
            subject_alt_names: None,
        };
        assert!(!san_pinning_requested(Some(&v)));
    }

    #[test]
    fn hostname_triggers_pinning() {
        let v = OutboundTlsValidation {
            ca_certificate_refs: None,
            well_known_ca_certificates: None,
            hostname: Some("idp.example.com".to_string()),
            subject_alt_names: None,
        };
        assert!(san_pinning_requested(Some(&v)));
    }

    #[test]
    fn subject_alt_names_triggers_pinning() {
        use edgion_resources::resources::backend_tls_policy::SubjectAltNameType;
        let v = OutboundTlsValidation {
            ca_certificate_refs: None,
            well_known_ca_certificates: None,
            hostname: None,
            subject_alt_names: Some(vec![SubjectAltName {
                san_type: SubjectAltNameType::Hostname,
                hostname: Some("idp.example.com".to_string()),
                uri: None,
            }]),
        };
        assert!(san_pinning_requested(Some(&v)));
    }

    #[test]
    fn empty_subject_alt_names_vec_is_not_pinning() {
        let v = OutboundTlsValidation {
            ca_certificate_refs: None,
            well_known_ca_certificates: None,
            hostname: None,
            subject_alt_names: Some(vec![]),
        };
        assert!(!san_pinning_requested(Some(&v)));
    }
}
