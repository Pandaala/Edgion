//! Common utilities shared across EdgionPlugins
//!
//! Provides shared infrastructure for plugins that need to make HTTP requests
//! to external services (e.g., ForwardAuth, OPA, Webhook), plus small helpers
//! reused across closely related plugins (e.g., the rate-limit family).

use std::fmt::Write as _;
use std::time::Duration;

use crate::plugins::runtime::{PluginLog, PluginSession};
use edgion_resources::common::KeyGet;
use edgion_resources::resources::edgion_plugins::OnKeyMiss;

pub mod auth_common;
mod bounded_regex_replacement;
mod capped_writer;
pub mod http_client;
pub mod http_util;
pub mod json_request;
pub mod outbound;

pub use auth_common::{resolve_claim_path, send_gateway_auth_error_response, set_claims_headers, Claims};
pub(crate) use bounded_regex_replacement::{
    replace_all_bounded, BoundedRegexReplacementError, CompiledRegexReplacement,
};
pub use capped_writer::CappedWriter;
pub use http_client::{
    connection_listed_hop_by_hop, get_http_client, is_hop_by_hop, is_hop_by_hop_or_listed, HOP_BY_HOP_HEADERS,
};
pub use json_request::{form_request_headers_are_supported, json_request_headers_are_supported};

/// Outcome of resolving a rate-limit key from the configured `key` sources.
///
/// The terminal `Allow` / `Deny` variants let each caller map them to its own
/// `PluginRunningResult`: `Allow` to `GoodNext`, `Deny` to the plugin-specific
/// `reject_request` (whose signature differs between `RateLimit` and
/// `RateLimitRedis`, so the rejection itself stays caller-side).
pub enum RateLimitKey {
    /// A key was resolved, including the intentional empty global-bucket key.
    Resolved(String),
    /// No key resolved and `OnKeyMiss::Allow` — let the request through.
    Allow,
    /// No key resolved and `OnKeyMiss::Deny` — reject the request.
    Deny,
}

/// Resolve the rate-limit key from `keys` via `session.key_get`, composing the
/// resolved parts with [`compose_rate_limit_key`].
///
/// Shared by the `RateLimit` and `RateLimitRedis` plugins so the key-extraction
/// policy (separator, default-key fallback, `OnKeyMiss` handling, and the
/// PluginLog tags) lives in exactly one place. The PluginLog tags distinguish the
/// three key-miss outcomes: `"key miss:default"` (rate-limited under the shared
/// default key), `"key miss:allow"` (full bypass via `OnKeyMiss::Allow`), and
/// `"Deny key miss"` (the canonical key-miss-denial tag, distinct from a
/// quota-exceeded denial).
///
/// `plugin_log` entries stay short (≤40B) and never contain the resolved key,
/// which can be a user-identifying value (API key, user_id).
pub fn resolve_rate_limit_key(
    session: &mut dyn PluginSession,
    keys: &[KeyGet],
    default_key: Option<&str>,
    on_missing: &OnKeyMiss,
    plugin_log: &mut PluginLog,
) -> RateLimitKey {
    match keys {
        // An empty `keys` list is not a key miss: there is no source that could
        // have been missed, so the plugin still rate-limits under one global
        // empty-string bucket. Routing it through the miss path would silently
        // disable the limiter whenever `onKeyMiss` is `Allow`.
        [] => return RateLimitKey::Resolved(String::new()),
        // The common single-source path can move the value returned by KeyGet
        // directly into the result instead of allocating a Vec and cloning it.
        [key] => {
            if let Some(value) = session.key_get(key) {
                return RateLimitKey::Resolved(value);
            }
        }
        _ => {
            let mut parts: Vec<Option<String>> = Vec::with_capacity(keys.len());
            for key in keys {
                parts.push(session.key_get(key));
            }
            if parts.iter().any(Option::is_some) {
                return RateLimitKey::Resolved(compose_rate_limit_key(&parts));
            }
        }
    }

    // No keys resolved - fall back to the configured default key if present.
    // Distinct tag from the OnKeyMiss::Allow path below: here the request IS
    // rate-limited (counted against the shared default-key bucket), it is not a bypass.
    if let Some(default_key) = default_key {
        plugin_log.push("key miss:default");
        return RateLimitKey::Resolved(default_key.to_string());
    }

    // No default key - apply the onKeyMiss policy.
    match on_missing {
        OnKeyMiss::Allow => {
            plugin_log.push("key miss:allow");
            RateLimitKey::Allow
        }
        OnKeyMiss::Deny => {
            plugin_log.push("Deny key miss");
            RateLimitKey::Deny
        }
    }
}

/// Compose the rate-limit bucket key from the per-source resolution results.
///
/// `parts` holds one slot per configured `KeyGet`, in configuration order,
/// `None` for a source that did not resolve. No configured source at all
/// composes to the empty string — one global bucket for the plugin.
///
/// A single configured source is used verbatim, which keeps the common
/// one-dimension configuration (and the Redis keys built from it) readable.
///
/// With more than one source the encoding must be injective: the key IS the
/// bucket identity, and at least one source is typically client-controlled
/// (header / cookie / query), so two different value tuples rendering to the
/// same string would let a client merge its traffic into another client's
/// bucket. A plain `"_"` join is not injective — `("t1", "u_2")` and
/// `("t1_u", "2")` both render to `t1_u_2` — and skipping unresolved sources
/// shifts the remaining ones, so `(None, "x")` and `("x", None)` also collide.
/// Each present slot is therefore length-prefixed (`<byte-len>:<value>`) and
/// every configured slot is represented (`-` when unresolved); the length
/// prefix makes the byte span of each part unambiguous regardless of its
/// content.
fn compose_rate_limit_key(parts: &[Option<String>]) -> String {
    if let [Some(only)] = parts {
        return only.clone();
    }

    // Reserve up-front: each slot costs its value plus a short length prefix and
    // one separator, so the common case fills the buffer without a realloc.
    let value_bytes: usize = parts.iter().flatten().map(|v| v.len()).sum();
    let mut out = String::with_capacity(value_bytes + parts.len() * 6);
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push('_');
        }
        match part {
            // Writing into a String is infallible.
            Some(v) => {
                let _ = write!(out, "{}:{}", v.len(), v);
            }
            None => out.push('-'),
        }
    }
    out
}

/// Format a duration in human-readable form (e.g., "1.5s", "500ms").
///
/// Shared by the `RateLimit` and `RateLimitRedis` plugins for the
/// `retry_in` response header.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs >= 1.0 {
        format!("{:.1}s", secs)
    } else {
        format!("{}ms", d.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_sub_second_uses_ms() {
        assert_eq!(format_duration(Duration::from_millis(500)), "500ms");
        assert_eq!(format_duration(Duration::from_millis(50)), "50ms");
        assert_eq!(format_duration(Duration::from_millis(999)), "999ms");
    }

    #[test]
    fn format_duration_one_second_and_above_uses_seconds() {
        assert_eq!(format_duration(Duration::from_millis(1000)), "1.0s");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(format_duration(Duration::from_secs(12)), "12.0s");
    }

    use crate::plugins::runtime::traits::session::MockPluginSession;

    #[test]
    fn resolve_rate_limit_key_returns_resolved_when_key_present() {
        let mut session = MockPluginSession::new();
        session.expect_key_get().return_const(Some("user-1".to_string()));
        let mut plugin_log = PluginLog::new("RateLimit");
        let keys = vec![KeyGet::RemoteIp];

        let out = resolve_rate_limit_key(&mut session, &keys, None, &OnKeyMiss::Deny, &mut plugin_log);

        assert!(matches!(out, RateLimitKey::Resolved(k) if k == "user-1"));
    }

    #[test]
    fn resolve_rate_limit_key_falls_back_to_default_key() {
        let mut session = MockPluginSession::new();
        session.expect_key_get().return_const(None);
        let mut plugin_log = PluginLog::new("RateLimit");
        let keys = vec![KeyGet::RemoteIp];

        let out = resolve_rate_limit_key(&mut session, &keys, Some("dflt"), &OnKeyMiss::Deny, &mut plugin_log);

        assert!(matches!(out, RateLimitKey::Resolved(k) if k == "dflt"));
        assert!(plugin_log.contains("key miss:default"));
    }

    #[test]
    fn resolve_rate_limit_key_allows_on_missing_allow() {
        let mut session = MockPluginSession::new();
        session.expect_key_get().return_const(None);
        let mut plugin_log = PluginLog::new("RateLimit");
        let keys = vec![KeyGet::RemoteIp];

        let out = resolve_rate_limit_key(&mut session, &keys, None, &OnKeyMiss::Allow, &mut plugin_log);

        assert!(matches!(out, RateLimitKey::Allow));
        assert!(plugin_log.contains("key miss:allow"));
    }

    #[test]
    fn resolve_rate_limit_key_denies_with_canonical_tag_on_missing_deny() {
        let mut session = MockPluginSession::new();
        session.expect_key_get().return_const(None);
        let mut plugin_log = PluginLog::new("RateLimit");
        let keys = vec![KeyGet::RemoteIp];

        let out = resolve_rate_limit_key(&mut session, &keys, None, &OnKeyMiss::Deny, &mut plugin_log);

        assert!(matches!(out, RateLimitKey::Deny));
        // Canonical key-miss-denial tag, shared by both rate-limit plugins.
        assert!(plugin_log.contains("Deny key miss"));
    }

    /// Resolve a two-header key configuration, returning the composed bucket key.
    fn resolve_tenant_user_key(tenant: Option<&str>, user: Option<&str>) -> String {
        let (tenant, user) = (tenant.map(str::to_string), user.map(str::to_string));
        let mut session = MockPluginSession::new();
        session.expect_key_get().returning(move |k| match k {
            KeyGet::Header { name } if name == "X-Tenant" => tenant.clone(),
            KeyGet::Header { name } if name == "X-User" => user.clone(),
            _ => None,
        });
        let mut plugin_log = PluginLog::new("RateLimit");
        let keys = vec![
            KeyGet::Header {
                name: "X-Tenant".to_string(),
            },
            KeyGet::Header {
                name: "X-User".to_string(),
            },
        ];

        match resolve_rate_limit_key(&mut session, &keys, None, &OnKeyMiss::Deny, &mut plugin_log) {
            RateLimitKey::Resolved(k) => k,
            _ => panic!("expected a resolved key"),
        }
    }

    #[test]
    fn resolve_rate_limit_key_multi_source_separator_is_unambiguous() {
        // A plain "_" join would render both tuples as "t1_u_2", merging the two
        // clients into one bucket.
        let a = resolve_tenant_user_key(Some("t1"), Some("u_2"));
        let b = resolve_tenant_user_key(Some("t1_u"), Some("2"));
        assert_ne!(a, b);
    }

    #[test]
    fn resolve_rate_limit_key_multi_source_unresolved_slot_keeps_position() {
        // Skipping an unresolved source would shift the remaining one, so both
        // tuples would render as "x".
        let a = resolve_tenant_user_key(None, Some("x"));
        let b = resolve_tenant_user_key(Some("x"), None);
        assert_ne!(a, b);
    }

    #[test]
    fn compose_rate_limit_key_single_source_is_verbatim() {
        assert_eq!(
            compose_rate_limit_key(&[Some("192.168.1.1".to_string())]),
            "192.168.1.1"
        );
    }

    #[test]
    fn resolve_rate_limit_key_empty_key_list_still_rate_limits() {
        let mut session = MockPluginSession::new();
        session.expect_key_get().times(0);
        let mut plugin_log = PluginLog::new("RateLimit");

        // No configured source is not a key miss: OnKeyMiss::Allow must not
        // turn an empty `key` list into a silent bypass of the limiter.
        let out = resolve_rate_limit_key(&mut session, &[], None, &OnKeyMiss::Allow, &mut plugin_log);

        assert!(matches!(out, RateLimitKey::Resolved(k) if k.is_empty()));
        let rendered = serde_json::to_value(&plugin_log).expect("plugin log must serialize");
        assert!(rendered.get("log").is_none());
    }

    #[test]
    fn compose_rate_limit_key_multi_source_is_length_prefixed() {
        let key = compose_rate_limit_key(&[Some("192.168.1.1".to_string()), Some("apikey123".to_string())]);
        assert_eq!(key, "11:192.168.1.1_9:apikey123");
    }
}
