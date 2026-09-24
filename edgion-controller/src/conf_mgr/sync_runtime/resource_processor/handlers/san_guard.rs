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

/// Guardrail uses the managed HTTP client path and its OutboundSanVerifier.
pub const SAN_ENFORCED_AI_GUARD: bool = true;
pub const SAN_ENFORCED_GUARDRAIL: bool = true;

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

/// Scan all plugin stages in `ep` for OIDC / LdapAuth / ExtProc / Guardrail entries that
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
                        EdgionPlugin::Guardrail(config) => {
                            errors.extend(check_validation_block(
                                config
                                    .conn
                                    .tls
                                    .as_ref()
                                    .and_then(|tls| tls.outbound.validation.as_ref()),
                                "Guardrail",
                                SAN_ENFORCED_GUARDRAIL,
                            ));
                        }
                        EdgionPlugin::AiGuard(config) => {
                            if let Some(evaluator) = config.evaluator.as_ref() {
                                errors.extend(check_validation_block(
                                    evaluator
                                        .conn
                                        .tls
                                        .as_ref()
                                        .and_then(|tls| tls.outbound.validation.as_ref()),
                                    "AiGuard",
                                    SAN_ENFORCED_AI_GUARD,
                                ));
                            }
                        }
                        // Not SAN-enforced at plugin level; outbound TLS is on the
                        // LinkSys Webhook resource (`SAN_ENFORCED_LINK_SYS`). Same as ForwardAuth.
                        EdgionPlugin::WebhookKeyGet(_) => {}
                        // No outbound TLS `validation` block to guard. Exhaustive
                        // (no `_`) on purpose: this is an admission gate for
                        // "SAN pinning declared but not enforced", so a new
                        // plugin that dials outbound TLS must be added above
                        // rather than silently escaping the check. Mirrors the
                        // exhaustive `SystemConfig` match in `check_link_sys_san`.
                        EdgionPlugin::RequestHeaderModifier(_)
                        | EdgionPlugin::AiProxy(_)
                        | EdgionPlugin::ResponseHeaderModifier(_)
                        | EdgionPlugin::RequestRedirect(_)
                        | EdgionPlugin::UrlRewrite(_)
                        | EdgionPlugin::RequestMirror(_)
                        | EdgionPlugin::BasicAuth(_)
                        | EdgionPlugin::CredentialInjector(_)
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
                        | EdgionPlugin::FormJsonTransform(_)
                        | EdgionPlugin::ResponseRewrite(_)
                        | EdgionPlugin::RateLimitLocal(_)
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
                        | EdgionPlugin::RequestId(_)
                        | EdgionPlugin::RequestAccessPolicy(_)
                        | EdgionPlugin::Canary(_)
                        | EdgionPlugin::GeoIpLocation(_)
                        | EdgionPlugin::Wasm(_) => {}
                    }
                }
            }
        };
    }

    scan_stage!(ep.spec.request_plugins);

    errors
}

/// Check the LinkSys provider's outbound TLS validation block for SAN pinning
/// intent while `SAN_ENFORCED_LINK_SYS` is false.
///
/// Returns at most one error string (per-resource, not per-endpoint).
pub fn check_link_sys_san(ls: &LinkSys) -> Vec<String> {
    // Extract the outbound validation block from whichever provider is configured.
    let tls = match &ls.spec.config {
        SystemConfig::Redis(c) => c.tls.as_ref(),
        SystemConfig::Etcd(c) => c.tls.as_ref(),
        SystemConfig::Otlp(c) => c.tls.as_ref(),
        SystemConfig::Elasticsearch(c) => c.tls.as_ref(),
        SystemConfig::Kafka(c) => c.tls.as_ref(),
        SystemConfig::Webhook(c) => c.conn.tls.as_ref(),
        // httpdns `connection` reuses the same ExternalHttpConnection shape as Webhook's `conn`,
        // so its outbound TLS validation block is guarded identically (not silently skipped).
        SystemConfig::HttpDns(c) => c.connection.as_ref().and_then(|conn| conn.tls.as_ref()),
        SystemConfig::CredentialSource(c) => match &c.provider {
            edgion_resources::resources::link_sys::credential_source::CredentialSourceProvider::OAuth2ClientCredentials(
                oauth2,
            ) => oauth2.tls.as_ref(),
        },
    };
    let validation_opt = tls
        .filter(|tls| tls.enabled)
        .and_then(|tls| tls.outbound.validation.as_ref());
    let provider = match &ls.spec.config {
        SystemConfig::Redis(_) => "Redis",
        SystemConfig::Etcd(_) => "Etcd",
        SystemConfig::Otlp(_) => "OTLP",
        SystemConfig::Elasticsearch(_) => "Elasticsearch",
        SystemConfig::Kafka(_) => "Kafka",
        SystemConfig::Webhook(_) => "Webhook",
        SystemConfig::HttpDns(_) => "HttpDns",
        SystemConfig::CredentialSource(_) => "CredentialSource",
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
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/san_guard_test.rs"]
mod tests;
