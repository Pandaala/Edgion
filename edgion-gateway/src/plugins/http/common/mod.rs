//! Common utilities shared across EdgionPlugins
//!
//! Provides shared infrastructure for plugins that need to make HTTP requests
//! to external services (e.g., ForwardAuth, OPA, Webhook), plus small helpers
//! reused across closely related plugins (e.g., the rate-limit family).

use std::time::Duration;

use crate::plugins::runtime::{PluginLog, PluginSession};
use edgion_resources::common::KeyGet;
use edgion_resources::resources::edgion_plugins::OnMissingKey;

pub mod auth_common;
pub mod http_client;

pub use auth_common::{resolve_claim_path, send_gateway_auth_error_response, set_claims_headers, Claims};
pub use http_client::{
    connection_listed_hop_by_hop, get_http_client, is_hop_by_hop, is_hop_by_hop_or_listed, HOP_BY_HOP_HEADERS,
};

/// Outcome of resolving a rate-limit key from the configured `key` sources.
///
/// The terminal `Allow` / `Deny` variants let each caller map them to its own
/// `PluginRunningResult`: `Allow` to `GoodNext`, `Deny` to the plugin-specific
/// `reject_request` (whose signature differs between `RateLimit` and
/// `RateLimitRedis`, so the rejection itself stays caller-side).
pub enum RateLimitKey {
    /// A non-empty key was resolved (joined parts, or the configured default).
    Resolved(String),
    /// No key resolved and `OnMissingKey::Allow` — let the request through.
    Allow,
    /// No key resolved and `OnMissingKey::Deny` — reject the request.
    Deny,
}

/// Resolve the rate-limit key from `keys` via `session.key_get`, joining the
/// resolved parts with `"_"`.
///
/// Shared by the `RateLimit` and `RateLimitRedis` plugins so the key-extraction
/// policy (separator, default-key fallback, `OnMissingKey` handling, and the
/// PluginLog tags) lives in exactly one place. The PluginLog tags distinguish the
/// three key-miss outcomes: `"key miss:default"` (rate-limited under the shared
/// default key), `"key miss:allow"` (full bypass via `OnMissingKey::Allow`), and
/// `"Deny key miss"` (the canonical key-miss-denial tag, distinct from a
/// quota-exceeded denial).
///
/// `plugin_log` entries stay short (≤40B) and never contain the resolved key,
/// which can be a user-identifying value (API key, user_id).
pub async fn resolve_rate_limit_key(
    session: &mut dyn PluginSession,
    keys: &[KeyGet],
    default_key: Option<&str>,
    on_missing: &OnMissingKey,
    plugin_log: &mut PluginLog,
) -> RateLimitKey {
    let mut parts: Vec<String> = Vec::new();
    for k in keys {
        if let Some(v) = session.key_get(k).await {
            parts.push(v);
        }
    }

    if !parts.is_empty() {
        return RateLimitKey::Resolved(parts.join("_"));
    }

    // No keys resolved - fall back to the configured default key if present.
    // Distinct tag from the OnMissingKey::Allow path below: here the request IS
    // rate-limited (counted against the shared default-key bucket), it is not a bypass.
    if let Some(default_key) = default_key {
        plugin_log.push("key miss:default");
        return RateLimitKey::Resolved(default_key.to_string());
    }

    // No default key - apply the onMissingKey policy.
    match on_missing {
        OnMissingKey::Allow => {
            plugin_log.push("key miss:allow");
            RateLimitKey::Allow
        }
        OnMissingKey::Deny => {
            plugin_log.push("Deny key miss");
            RateLimitKey::Deny
        }
    }
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

    #[tokio::test]
    async fn resolve_rate_limit_key_returns_resolved_when_key_present() {
        let mut session = MockPluginSession::new();
        session.expect_key_get().return_const(Some("user-1".to_string()));
        let mut plugin_log = PluginLog::new("RateLimit");
        let keys = vec![KeyGet::ClientIp];

        let out = resolve_rate_limit_key(&mut session, &keys, None, &OnMissingKey::Deny, &mut plugin_log).await;

        assert!(matches!(out, RateLimitKey::Resolved(k) if k == "user-1"));
    }

    #[tokio::test]
    async fn resolve_rate_limit_key_falls_back_to_default_key() {
        let mut session = MockPluginSession::new();
        session.expect_key_get().return_const(None);
        let mut plugin_log = PluginLog::new("RateLimit");
        let keys = vec![KeyGet::ClientIp];

        let out = resolve_rate_limit_key(&mut session, &keys, Some("dflt"), &OnMissingKey::Deny, &mut plugin_log).await;

        assert!(matches!(out, RateLimitKey::Resolved(k) if k == "dflt"));
        assert!(plugin_log.contains("key miss:default"));
    }

    #[tokio::test]
    async fn resolve_rate_limit_key_allows_on_missing_allow() {
        let mut session = MockPluginSession::new();
        session.expect_key_get().return_const(None);
        let mut plugin_log = PluginLog::new("RateLimit");
        let keys = vec![KeyGet::ClientIp];

        let out = resolve_rate_limit_key(&mut session, &keys, None, &OnMissingKey::Allow, &mut plugin_log).await;

        assert!(matches!(out, RateLimitKey::Allow));
        assert!(plugin_log.contains("key miss:allow"));
    }

    #[tokio::test]
    async fn resolve_rate_limit_key_denies_with_canonical_tag_on_missing_deny() {
        let mut session = MockPluginSession::new();
        session.expect_key_get().return_const(None);
        let mut plugin_log = PluginLog::new("RateLimit");
        let keys = vec![KeyGet::ClientIp];

        let out = resolve_rate_limit_key(&mut session, &keys, None, &OnMissingKey::Deny, &mut plugin_log).await;

        assert!(matches!(out, RateLimitKey::Deny));
        // Canonical key-miss-denial tag, shared by both rate-limit plugins.
        assert!(plugin_log.contains("Deny key miss"));
    }
}
