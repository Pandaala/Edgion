//! SAN-pinning guard: reject outbound TLS `validation.subjectAltNames` /
//! `validation.hostname` on providers where enforcement is not yet implemented.
//!
//! # Design
//!
//! The `validation` schema fields are exposed on all outbound-TLS-capable
//! providers so operators can write the config against the shared shape, but the
//! data-plane enforcement lands provider-by-provider. If an operator configures
//! a field a provider does not enforce, the config would be silently ignored —
//! worse than a clear rejection. This module surfaces that case as an
//! `Accepted=False` validation error (via the standard `preparse` error-string
//! mechanism) so the operator learns immediately.
//!
//! Two independent gates, because the two fields have different rollout state:
//!
//! - **`subjectAltNames`** (SAN pinning) — enforced per-provider via
//!   `SAN_ENFORCED_*`. oidc/ldap enforce it today; ext_proc/link_sys/egc do not.
//! - **`hostname`** (SNI override) — implemented on *no* outbound provider yet,
//!   so it is rejected uniformly via [`HOSTNAME_SNI_ENFORCED`], independent of
//!   `SAN_ENFORCED_*`. This closes the trap where a provider that enforces
//!   `subjectAltNames` (oidc/ldap) would otherwise accept-and-ignore `hostname`.
//!
//! # Enablement
//!
//! As each provider's verifier lands, flip the corresponding `SAN_ENFORCED_*`
//! constant to `true` (or `HOSTNAME_SNI_ENFORCED` once SNI override is wired) and
//! redeploy. [`check_validation_block`] short-circuits on the flipped gate, so
//! the error disappears without any other code change.

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

/// `validation.hostname` (SNI override + implicit peer-name check) is not
/// implemented on ANY outbound provider yet — unlike `subjectAltNames`, which
/// oidc/ldap already enforce. So it is rejected uniformly across every provider
/// (independent of the per-provider `SAN_ENFORCED_*` gates) until a provider
/// actually wires SNI override. Flip to `true` (or split per-provider) once it
/// lands. Keeping this separate from `SAN_ENFORCED_*` avoids the trap where a
/// provider that enforces `subjectAltNames` silently accepts-and-ignores
/// `hostname`.
pub const HOSTNAME_SNI_ENFORCED: bool = false;

// ============================================================================
// Sentinel predicates
// ============================================================================

/// Returns `true` iff `validation` declares a non-empty `subjectAltNames` list.
pub fn subject_alt_names_requested(validation: Option<&OutboundTlsValidation>) -> bool {
    validation.is_some_and(|v| v.subject_alt_names.as_ref().is_some_and(|s| !s.is_empty()))
}

/// Returns `true` iff `validation` sets a `hostname` (SNI override).
pub fn hostname_requested(validation: Option<&OutboundTlsValidation>) -> bool {
    validation.is_some_and(|v| v.hostname.is_some())
}

// ============================================================================
// Canonical error messages + per-block check
// ============================================================================

fn san_not_supported_msg(provider: &str) -> String {
    format!(
        "outbound TLS subjectAltNames is not supported for {} in Phase 1 \
         (SAN pinning enforcement deferred to Phase 2)",
        provider
    )
}

fn hostname_not_supported_msg(provider: &str) -> String {
    format!(
        "outbound TLS validation.hostname (SNI override) is not implemented for {} \
         (would be silently ignored); remove it or use subjectAltNames for SAN pinning",
        provider
    )
}

/// Check one provider's outbound `validation` block against both gates:
/// - `subjectAltNames` → gated by the per-provider `san_enforced`.
/// - `hostname` → gated by the uniform [`HOSTNAME_SNI_ENFORCED`].
///
/// Returns one error string per unsupported-but-configured field.
fn check_validation_block(
    validation: Option<&OutboundTlsValidation>,
    provider: &str,
    san_enforced: bool,
) -> Vec<String> {
    let mut errors = Vec::new();
    if !san_enforced && subject_alt_names_requested(validation) {
        errors.push(san_not_supported_msg(provider));
    }
    if !HOSTNAME_SNI_ENFORCED && hostname_requested(validation) {
        errors.push(hostname_not_supported_msg(provider));
    }
    errors
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
                    if !entry.is_enabled() || entry.policy_action.is_some() {
                        continue;
                    }
                    match &entry.plugin {
                        EdgionPlugin::OpenidConnect(config) => {
                            errors.extend(check_validation_block(
                                config.tls.validation.as_ref(),
                                "OpenidConnect",
                                SAN_ENFORCED_OIDC,
                            ));
                        }
                        EdgionPlugin::LdapAuth(config) => {
                            errors.extend(check_validation_block(
                                config.tls.validation.as_ref(),
                                "LdapAuth",
                                SAN_ENFORCED_LDAP,
                            ));
                        }
                        EdgionPlugin::ExtProc(config) => {
                            errors.extend(check_validation_block(
                                config.grpc_service.tls.outbound.validation.as_ref(),
                                "ExtProc",
                                SAN_ENFORCED_EXT_PROC,
                            ));
                        }
                        // No outbound TLS `validation` block to guard. Exhaustive
                        // (no `_`) on purpose: this is an admission gate for
                        // "SAN pinning declared but not enforced", so a new
                        // plugin that dials outbound TLS must be added above
                        // rather than silently escaping the check. Mirrors the
                        // exhaustive `SystemConfig` match in `check_link_sys_san`.
                        EdgionPlugin::RequestHeaderModifier(_)
                        | EdgionPlugin::ResponseHeaderModifier(_)
                        | EdgionPlugin::RequestRedirect(_)
                        | EdgionPlugin::UrlRewrite(_)
                        | EdgionPlugin::RequestMirror(_)
                        | EdgionPlugin::BasicAuth(_)
                        | EdgionPlugin::Cors(_)
                        | EdgionPlugin::Csrf(_)
                        | EdgionPlugin::IpRestriction(_)
                        | EdgionPlugin::JwtAuth(_)
                        | EdgionPlugin::JweDecrypt(_)
                        | EdgionPlugin::HmacAuth(_)
                        | EdgionPlugin::HeaderCertAuth(_)
                        | EdgionPlugin::KeyAuth(_)
                        | EdgionPlugin::Mock(_)
                        | EdgionPlugin::FaultInjection(_)
                        | EdgionPlugin::DebugAccessLogToHeader(_)
                        | EdgionPlugin::ProxyRewrite(_)
                        | EdgionPlugin::RequestRestriction(_)
                        | EdgionPlugin::RequestBodyBuffer(_)
                        | EdgionPlugin::JsonSchemaValidation(_)
                        | EdgionPlugin::ResponseRewrite(_)
                        | EdgionPlugin::RateLimit(_)
                        | EdgionPlugin::RateLimitRedis(_)
                        | EdgionPlugin::CtxSet(_)
                        | EdgionPlugin::RealIp(_)
                        | EdgionPlugin::ForwardAuth(_)
                        | EdgionPlugin::BandwidthLimit(_)
                        | EdgionPlugin::DirectEndpoint(_)
                        | EdgionPlugin::AllEndpointStatus(_)
                        | EdgionPlugin::DynamicInternalUpstream(_)
                        | EdgionPlugin::DynamicExternalUpstream(_)
                        | EdgionPlugin::Dsl(_)
                        | EdgionPlugin::RegionRoute(_)
                        | EdgionPlugin::TraceContext(_)
                        | EdgionPlugin::GlobalAccessControl(_)
                        | EdgionPlugin::Canary(_)
                        | EdgionPlugin::Wasm(_) => {}
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
    // Extract the outbound validation block from whichever provider is configured.
    let validation_opt: Option<&OutboundTlsValidation> = match &ls.spec.config {
        SystemConfig::Redis(c) => c.tls.as_ref().and_then(|t| t.outbound.validation.as_ref()),
        SystemConfig::Etcd(c) => c.tls.as_ref().and_then(|t| t.outbound.validation.as_ref()),
        SystemConfig::Elasticsearch(c) => c.tls.as_ref().and_then(|t| t.outbound.validation.as_ref()),
        SystemConfig::Kafka(c) => c.tls.as_ref().and_then(|t| t.outbound.validation.as_ref()),
        SystemConfig::Webhook(c) => c.conn.tls.as_ref().and_then(|t| t.outbound.validation.as_ref()),
        // httpdns `connection` reuses the same ExternalHttpConnection shape as Webhook's `conn`,
        // so its outbound TLS validation block is guarded identically (not silently skipped).
        SystemConfig::HttpDns(c) => c
            .connection
            .as_ref()
            .and_then(|conn| conn.tls.as_ref())
            .and_then(|t| t.outbound.validation.as_ref()),
    };
    let provider = match &ls.spec.config {
        SystemConfig::Redis(_) => "Redis",
        SystemConfig::Etcd(_) => "Etcd",
        SystemConfig::Elasticsearch(_) => "Elasticsearch",
        SystemConfig::Kafka(_) => "Kafka",
        SystemConfig::Webhook(_) => "Webhook",
        SystemConfig::HttpDns(_) => "HttpDns",
    };
    check_validation_block(validation_opt, provider, SAN_ENFORCED_LINK_SYS)
}

/// Check the EdgionGatewayConfig global outbound TLS validation block for SAN
/// pinning intent while `SAN_ENFORCED_EGC` is false.
///
/// Returns at most one error string.
pub fn check_egc_san(egc: &EdgionGatewayConfig) -> Vec<String> {
    let validation_opt = egc.spec.outbound_tls.as_ref().and_then(|ob| ob.validation.as_ref());
    check_validation_block(validation_opt, "EdgionGatewayConfig", SAN_ENFORCED_EGC)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
    use edgion_resources::resources::common::OutboundTlsValidation;

    fn san_only() -> OutboundTlsValidation {
        OutboundTlsValidation {
            subject_alt_names: Some(vec![SubjectAltName {
                san_type: SubjectAltNameType::Hostname,
                hostname: Some("idp.example.com".to_string()),
                uri: None,
            }]),
            ..Default::default()
        }
    }

    fn hostname_only() -> OutboundTlsValidation {
        OutboundTlsValidation {
            hostname: Some("idp.example.com".to_string()),
            ..Default::default()
        }
    }

    // ── predicates ────────────────────────────────────────────────────────────

    #[test]
    fn predicates_on_empty_and_none() {
        assert!(!subject_alt_names_requested(None));
        assert!(!hostname_requested(None));
        let empty = OutboundTlsValidation::default();
        assert!(!subject_alt_names_requested(Some(&empty)));
        assert!(!hostname_requested(Some(&empty)));
    }

    #[test]
    fn only_ca_refs_is_neither_san_nor_hostname() {
        use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
        let v = OutboundTlsValidation {
            ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                kind: "Secret".to_string(),
                name: "my-ca".to_string(),
                namespace: None,
                ..Default::default()
            }]),
            ..Default::default()
        };
        assert!(!subject_alt_names_requested(Some(&v)));
        assert!(!hostname_requested(Some(&v)));
    }

    #[test]
    fn subject_alt_names_and_hostname_predicates_are_independent() {
        assert!(subject_alt_names_requested(Some(&san_only())));
        assert!(!hostname_requested(Some(&san_only())));
        assert!(hostname_requested(Some(&hostname_only())));
        assert!(!subject_alt_names_requested(Some(&hostname_only())));
    }

    #[test]
    fn empty_subject_alt_names_vec_is_not_requested() {
        let v = OutboundTlsValidation {
            subject_alt_names: Some(vec![]),
            ..Default::default()
        };
        assert!(!subject_alt_names_requested(Some(&v)));
    }

    // ── check_validation_block: the SAN gate is per-provider, hostname is not ──

    #[test]
    fn san_rejected_when_provider_gate_off() {
        let errs = check_validation_block(Some(&san_only()), "ExtProc", false);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("subjectAltNames"));
    }

    #[test]
    fn san_allowed_when_provider_gate_on() {
        // oidc/ldap-style: subjectAltNames is enforced, so it passes the guard.
        assert!(check_validation_block(Some(&san_only()), "OpenidConnect", true).is_empty());
    }

    #[test]
    fn hostname_rejected_even_when_san_gate_on() {
        // The key fix: a provider that enforces subjectAltNames (gate on) must
        // still reject hostname, because SNI override is implemented nowhere.
        assert!(!HOSTNAME_SNI_ENFORCED, "test assumes SNI override not yet enforced");
        let errs = check_validation_block(Some(&hostname_only()), "OpenidConnect", true);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("hostname"));
    }

    #[test]
    fn both_fields_yield_two_errors_when_both_unsupported() {
        let v = OutboundTlsValidation {
            hostname: Some("idp.example.com".to_string()),
            subject_alt_names: san_only().subject_alt_names,
            ..Default::default()
        };
        let errs = check_validation_block(Some(&v), "ExtProc", false);
        assert_eq!(
            errs.len(),
            2,
            "expected both subjectAltNames and hostname errors, got: {errs:?}"
        );
    }

    #[test]
    fn policy_stamped_plugin_is_inert_for_san_guard() {
        use edgion_resources::resources::edgion_plugins::{
            EdgionPlugin, EdgionPluginsSpec, ExtProcConfig, RequestFilterEntry,
        };
        use edgion_resources::resources::plugin_policy::PluginPolicyAction;

        let mut config = ExtProcConfig::default();
        config.grpc_service.tls.outbound.validation = Some(hostname_only());
        let mut entry = RequestFilterEntry::new(EdgionPlugin::ExtProc(config));
        entry.policy_action = Some(PluginPolicyAction::Bypass);
        let plugins = EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                request_plugins: Some(vec![entry]),
                ..Default::default()
            },
            status: None,
        };

        assert!(check_plugins_san(&plugins).is_empty());
    }
}
