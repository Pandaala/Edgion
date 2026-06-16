//! GlobalConnectionIpRestriction stream plugin — L4 pre-TLS IP filter with base + overlay support.
//!
//! # Fail-to-base semantics
//! The plugin holds an inline pre-compiled `base` config (from `GlobalConnectionIpRestrictionEntry`).
//! When `override_key` is set, the plugin looks up a `GlobalConnIpRestrictionOverride` EdgionData
//! at connection time and merges it on top of the base. On any overlay error:
//! - **`overlay_missing`**: EdgionData not found in the store → use base.
//! - **`overlay_type_mismatch`**: EdgionData present but wrong variant type → use base.
//! - **`overlay_invalid`**: overlay applied but merged effective config fails validation → use base.
//!
//! In all error cases the base is used (fail-to-base), NOT a deny. The base MUST therefore
//! be authored as the conservative/safe value (e.g. `defaultAction: Deny` + explicit allowlist).
//!
//! # No `tracing::warn!/error!` in this module
//! This module is on the per-connection data plane. All observability goes through
//! `record_conn_ip_restriction_event` (Prometheus counter).

use std::sync::Arc;

use async_trait::async_trait;

use crate::observe::metrics::record_conn_ip_restriction_event;
use crate::plugins::runtime::data::get_global_edgion_data_store;
use crate::plugins::stream::stream_plugin_trait::{
    access_decision_to_stream_result, StreamContext, StreamPlugin, StreamPluginResult,
};
use edgion_resources::resources::data::{
    apply_override, merge_gcir_override, DataEntry, GlobalConnectionIpRestrictionData, OverlaySignal,
};
use edgion_resources::resources::edgion_stream_plugins::GlobalConnectionIpRestrictionEntry;

const PLUGIN_NAME: &str = "GlobalConnectionIpRestriction";

pub struct GlobalConnectionIpRestrictionPlugin {
    /// Pre-compiled base config. Matchers are compiled at construction time via
    /// `validate_and_init`. Used directly when no overlay is set, and as the
    /// fail-to-base fallback on any overlay error.
    base: Arc<GlobalConnectionIpRestrictionData>,
    /// Optional EdgionData store key ("namespace/name") for a
    /// `GlobalConnIpRestrictionOverride` to apply on top of `base` at connection
    /// time. `None` means no overlay is configured.
    override_key: Option<String>,
}

impl GlobalConnectionIpRestrictionPlugin {
    /// Construct a plugin from a `GlobalConnectionIpRestrictionEntry`.
    ///
    /// Calls `validate_and_init` on the base to pre-compile IP matchers.
    /// Returns an error if the base config is structurally invalid (this should
    /// not happen in practice if the controller validated before syncing).
    pub fn new(entry: &GlobalConnectionIpRestrictionEntry, parent_namespace: &str) -> Result<Self, String> {
        let mut base = entry.base.clone();
        base.validate_and_init()
            .map_err(|e| format!("GlobalConnectionIpRestriction base validation failed: {e}"))?;

        let override_key = entry.override_ref.as_ref().map(|r| {
            let ns = r.namespace.as_deref().unwrap_or(parent_namespace);
            format!("{}/{}", ns, r.name)
        });

        Ok(Self {
            base: Arc::new(base),
            override_key,
        })
    }

    /// Compute the effective config for this connection.
    ///
    /// Fast path (no overlay): clones and returns base with pre-compiled matchers.
    /// Slow path (overlay configured): looks up the EdgionData store, applies the
    /// typed overlay via `apply_override`, then calls `validate_and_init` on the
    /// merged result to compile matchers for any overlay-supplied profiles.
    ///
    /// On any overlay error, emits a metric signal and falls back to the base.
    fn compute_effective(&self) -> GlobalConnectionIpRestrictionData {
        let Some(ref key) = self.override_key else {
            // No overlay configured — use base directly (already has compiled matchers).
            return (*self.base).clone();
        };

        let store = get_global_edgion_data_store();
        let raw_ed = store.get_raw_by_key(key);
        // The overlay entry is the `DataEntry` inside the raw EdgionData (if found).
        let overlay_data_entry = raw_ed.as_deref().map(|ed| &ed.spec.data);

        let base_clone = (*self.base).clone();
        let (mut merged, signal) = apply_override(
            base_clone,
            overlay_data_entry,
            // Type guard (D6): only accept `GlobalConnIpRestrictionOverride` variant.
            |entry: &DataEntry| match entry {
                DataEntry::GlobalConnIpRestrictionOverride(o) => Some(o),
                _ => None,
            },
            // Merge function: field-level replace; collections whole-replace.
            merge_gcir_override,
        );

        match signal {
            OverlaySignal::Absent => {
                // overrideRef set but EdgionData not found in store.
                record_conn_ip_restriction_event("overlay_missing");
                merged // apply_override returns base unchanged; has compiled matchers
            }
            OverlaySignal::TypeMismatch => {
                // EdgionData found but variant is not GlobalConnIpRestrictionOverride.
                record_conn_ip_restriction_event("overlay_type_mismatch");
                merged // apply_override returns base unchanged; has compiled matchers
            }
            OverlaySignal::Applied => {
                // Overlay merged into base. Must compile matchers for any overlay-supplied
                // profiles (deserialized ProfileRules have no compiled matchers).
                match merged.validate_and_init() {
                    Ok(()) => merged,
                    Err(_) => {
                        // Merged effective config is invalid (e.g. activeProfile not in profiles).
                        record_conn_ip_restriction_event("overlay_invalid");
                        (*self.base).clone() // fresh clone with pre-compiled matchers
                    }
                }
            }
        }
    }
}

#[async_trait]
impl StreamPlugin for GlobalConnectionIpRestrictionPlugin {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        let effective = self.compute_effective();

        if !effective.enable {
            // Kill switch: plugin disabled — allow all connections.
            return StreamPluginResult::Allow;
        }

        let Some(profile) = effective.profiles.get(&effective.active_profile) else {
            // Should not happen after validate_and_init, but guard defensively.
            record_conn_ip_restriction_event("fail_closed_profile_missing");
            return StreamPluginResult::Deny("ip:deny:profile-missing".to_string());
        };

        access_decision_to_stream_result(profile.check_ip(&ctx.client_ip), || {
            record_conn_ip_restriction_event("denied")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::data::{IpGroup, ProfileRules};
    use edgion_resources::resources::edgion_plugins::DefaultAction;
    use edgion_resources::resources::edgion_stream_plugins::GlobalConnectionIpRestrictionEntry;
    use std::collections::HashMap;

    /// Build a minimal valid entry with a single profile.
    ///
    /// `default_action`: the profile's default action (Deny = restrictive, Allow = permissive).
    /// When `default_action` is Deny, only the given `allow_cidr` is permitted.
    fn make_entry(
        active_profile: &str,
        default_action: DefaultAction,
        allow_cidr: Option<&str>,
        deny_cidr: Option<&str>,
    ) -> GlobalConnectionIpRestrictionEntry {
        let profile = ProfileRules {
            allow: allow_cidr.map(|cidr| {
                vec![IpGroup {
                    name: "test-allow".to_string(),
                    description: None,
                    cidrs: vec![cidr.to_string()],
                }]
            }),
            deny: deny_cidr.map(|cidr| {
                vec![IpGroup {
                    name: "test-deny".to_string(),
                    description: None,
                    cidrs: vec![cidr.to_string()],
                }]
            }),
            default_action,
            allow_matcher: None,
            deny_matcher: None,
        };
        let base = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: active_profile.to_string(),
            profiles: HashMap::from([(active_profile.to_string(), profile)]),
            description: None,
        };
        GlobalConnectionIpRestrictionEntry {
            base,
            override_ref: None,
        }
    }

    /// Construct a plugin with a restrictive base (deny all except loopback).
    fn plugin_deny_nonloopback() -> GlobalConnectionIpRestrictionPlugin {
        let entry = make_entry("main", DefaultAction::Deny, Some("127.0.0.0/8"), None);
        GlobalConnectionIpRestrictionPlugin::new(&entry, "default").expect("valid base")
    }

    #[tokio::test]
    async fn no_overlay_base_allow_loopback() {
        let plugin = plugin_deny_nonloopback();
        let ctx = StreamContext::new("127.0.0.1".parse().unwrap(), 8080);
        assert!(
            matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow),
            "loopback must be allowed by the restrictive-base allow-list"
        );
    }

    #[tokio::test]
    async fn no_overlay_base_deny_non_loopback() {
        let plugin = plugin_deny_nonloopback();
        let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080);
        assert!(
            matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)),
            "non-loopback must be denied by the restrictive base"
        );
    }

    #[tokio::test]
    async fn absent_overlay_falls_back_to_base() {
        // Build a plugin that points at a non-existent override EdgionData.
        let mut entry = make_entry("main", DefaultAction::Deny, Some("127.0.0.0/8"), None);
        // Point at an override that does NOT exist in the store.
        entry.override_ref = Some(edgion_resources::resources::data::DataRef {
            name: "gcir-unit-test-absent-overlay-do-not-register".to_string(),
            namespace: Some("default".to_string()),
        });
        let plugin = GlobalConnectionIpRestrictionPlugin::new(&entry, "default").expect("valid base");

        // Connection from loopback — base allows it, overlay is absent → fall to base → Allow.
        let ctx = StreamContext::new("127.0.0.1".parse().unwrap(), 8080);
        assert!(
            matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow),
            "absent overlay must fall back to base (which allows loopback)"
        );

        // Non-loopback — base denies it → still denied even though overlay is absent.
        let ctx2 = StreamContext::new("8.8.8.8".parse().unwrap(), 8080);
        assert!(
            matches!(plugin.on_connection(&ctx2).await, StreamPluginResult::Deny(_)),
            "absent overlay falls to base; non-loopback must be denied"
        );
    }

    #[tokio::test]
    async fn enable_false_allows_all_connections() {
        let mut entry = make_entry("main", DefaultAction::Deny, Some("127.0.0.0/8"), None);
        entry.base.enable = false;
        let plugin = GlobalConnectionIpRestrictionPlugin::new(&entry, "default").expect("valid base");

        let ctx = StreamContext::new("1.2.3.4".parse().unwrap(), 8080);
        assert!(
            matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow),
            "enable=false must allow all connections regardless of profile rules"
        );
    }

    #[test]
    fn invalid_base_returns_error() {
        // A base with an active_profile that does not exist in profiles is invalid.
        let profile = ProfileRules {
            allow: Some(vec![IpGroup {
                name: "g".to_string(),
                description: None,
                cidrs: vec!["10.0.0.0/8".to_string()],
            }]),
            deny: None,
            default_action: DefaultAction::Deny,
            allow_matcher: None,
            deny_matcher: None,
        };
        let base = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "nonexistent".to_string(),
            profiles: HashMap::from([("p1".to_string(), profile)]),
            description: None,
        };
        let entry = GlobalConnectionIpRestrictionEntry {
            base,
            override_ref: None,
        };
        assert!(
            GlobalConnectionIpRestrictionPlugin::new(&entry, "default").is_err(),
            "invalid base (active_profile not in profiles) must return an error"
        );
    }
}
