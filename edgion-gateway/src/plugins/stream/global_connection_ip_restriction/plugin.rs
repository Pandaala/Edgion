//! GlobalConnectionIpRestriction stream plugin — L4 pre-TLS IP filter with an active-profile selector.
//!
//! # Model
//! The plugin holds an inline, pre-compiled `base` config (the flat
//! `GlobalConnectionIpRestrictionConfig` from the stream-plugin spec). It carries the
//! complete, git-owned IP rule data (`enable` + named `profiles`). The only dynamic input
//! is *which profile is active*: when `active_profile_ref` is set, the plugin looks up a
//! `Selector` EdgionConfigData at connection time and uses its `active` profile name —
//! but only to choose among the base's own profiles. The rule data is never overlaid.
//!
//! # Fail-to-base semantics (signals via Prometheus, never `tracing` on the data plane)
//! - **`overlay_missing`**: `activeProfileRef` set but EdgionConfigData not found → use base.
//! - **`overlay_type_mismatch`**: EdgionConfigData present but not a `Selector` variant → use base.
//! - **`overlay_disabled`**: EdgionConfigData envelope `spec.enable=false` → use base.
//! - **`overlay_invalid`**: Selector names a profile not in `profiles` → use base.
//! - Selector `active: null` / absent → **no-op**, use base (no counter).
//!
//! A selector can only switch among the git-owned `profiles`; it can never disable filtering
//! or inject rules. Every non-switch outcome falls back to `active_profile` (fail-to-base),
//! so the base MUST be authored as the conservative/safe value (e.g. `defaultAction: Deny`).

use edgion_resources::refs::ip_list_fields::GlobalIpListEntry;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use smallvec::SmallVec;

use crate::observe::metrics::record_conn_ip_restriction_event;
use crate::plugins::runtime::edgion_config_data::{
    capture_global_edgion_config_data_snapshot, capture_global_edgion_config_data_snapshot_for_refs,
    resolve_bound_ip_list_keys, EdgionConfigDataSnapshot,
};
use crate::plugins::stream::stream_plugin_trait::{
    access_decision_to_stream_result, StreamContext, StreamPlugin, StreamPluginResult,
};
use edgion_algo::matcher::radix_ip::IpRadixMatcher;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_stream_plugins::{GlobalConnectionIpRestrictionConfig, ProfileRules};

const PLUGIN_NAME: &str = "GlobalConnectionIpRestriction";

pub struct GlobalConnectionIpRestrictionPlugin {
    /// Pre-compiled base config. Matchers are compiled at construction time via
    /// `validate_and_init`. Used directly when no selector is set, and as the
    /// fail-to-base fallback on any selector error.
    base: Arc<GlobalConnectionIpRestrictionConfig>,
    /// Optional EdgionConfigData store key ("namespace/name") for a `Selector` that
    /// supplies the active profile name at connection time. `None` means no selector.
    active_profile_ref_key: Option<String>,
    /// Per-profile resolved IpList ref keys (deny_keys, allow_keys), keyed by profile name.
    /// Pre-resolved at construction because `parent_namespace` is not retained.
    profile_ref_keys: HashMap<String, (Vec<String>, Vec<String>)>,
}

impl GlobalConnectionIpRestrictionPlugin {
    /// Construct a plugin from a `GlobalConnectionIpRestrictionConfig`.
    ///
    /// Calls `validate_and_init` on the config to pre-compile IP matchers.
    /// Returns an error if the config is structurally invalid (this should not happen in
    /// practice if the controller validated before syncing).
    pub(crate) fn from_bound(entry: GlobalIpListEntry<'_>) -> Result<Self, String> {
        let config = entry.config();
        let selector = entry.selector();
        if let Some(field) = &selector {
            field.validate_owner().map_err(|error| error.to_string())?;
        }
        let active_profile_ref_key = selector.and_then(|field| field.runtime_key().ok().flatten());
        let profile_ref_keys = entry
            .profiles()
            .map(|(name, refs)| resolve_bound_ip_list_keys(refs).map(|(allow, deny)| (name.to_string(), (deny, allow))))
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(|error| error.to_string())?;
        let mut base = config.clone();
        base.validate_and_init()
            .map_err(|e| format!("GlobalConnectionIpRestriction config validation failed: {e}"))?;

        Ok(Self {
            base: Arc::new(base),
            active_profile_ref_key,
            profile_ref_keys,
        })
    }

    /// Resolve the effective active-profile **name** for this connection.
    ///
    /// A selector can only choose another git-owned profile, so this returns just the profile
    /// name — no rule-data clone on the hot path. Every error / `active: null` / disabled path
    /// returns the base default (`active_profile`), i.e. fail-to-base. The caller indexes
    /// `self.base.profiles` with the returned name (matchers already compiled at construction).
    fn resolve_active_profile_from_snapshot<'a>(
        &'a self,
        snapshot: &'a EdgionConfigDataSnapshot,
        selector_key: &str,
    ) -> &'a str {
        let raw_ed = snapshot.get_raw_by_key(selector_key);

        // Envelope kill-switch: spec.enable=false means the operator disabled this
        // EdgionConfigData entirely. Ignore the selector; use base.
        if raw_ed.map(|ed| !ed.spec.enable).unwrap_or(false) {
            record_conn_ip_restriction_event("overlay_disabled");
            return self.base.active_profile.as_str();
        }

        match raw_ed.map(|ed| &ed.spec.data) {
            None => {
                // activeProfileRef set but EdgionConfigData not found in the store.
                record_conn_ip_restriction_event("overlay_missing");
                self.base.active_profile.as_str()
            }
            Some(ConfigEntry::Selector(sel)) => match sel.active.as_deref() {
                // `active: null` / absent → deliberate no-op (use base).
                None => self.base.active_profile.as_str(),
                // Valid switch among the git-owned profiles.
                Some(name) if self.base.profiles.contains_key(name) => name,
                // Selector names a profile that does not exist in the git-owned profiles.
                Some(_) => {
                    record_conn_ip_restriction_event("overlay_invalid");
                    self.base.active_profile.as_str()
                }
            },
            Some(_) => {
                // EdgionConfigData found but it is not a Selector variant.
                record_conn_ip_restriction_event("overlay_type_mismatch");
                self.base.active_profile.as_str()
            }
        }
    }

    fn active_profile_ref_keys(&self, active: &str) -> (&[String], &[String]) {
        self.profile_ref_keys
            .get(active)
            .map(|(deny, allow)| (deny.as_slice(), allow.as_slice()))
            .unwrap_or_default()
    }

    fn evaluate_profile(
        &self,
        profile: &ProfileRules,
        deny_keys: &[String],
        allow_keys: &[String],
        snapshot: Option<&EdgionConfigDataSnapshot>,
        ctx: &StreamContext,
    ) -> StreamPluginResult {
        let mut deny_refs: SmallVec<[&IpRadixMatcher; 4]> = SmallVec::new();
        let mut allow_refs: SmallVec<[&IpRadixMatcher; 4]> = SmallVec::new();
        if let Some(snapshot) = snapshot {
            snapshot.append_ip_list_matchers(deny_keys, &mut deny_refs);
            snapshot.append_ip_list_matchers(allow_keys, &mut allow_refs);
        }
        let ip_decision = profile.check_ip_with_refs(&ctx.direct_peer_ip, &deny_refs, &allow_refs);
        access_decision_to_stream_result(ip_decision, || record_conn_ip_restriction_event("denied"))
    }
}

#[async_trait]
impl StreamPlugin for GlobalConnectionIpRestrictionPlugin {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        // Kill switch (git-owned; a selector cannot touch it) — allow all connections.
        if !self.base.enable {
            return StreamPluginResult::Allow;
        }

        if let Some(selector_key) = self.active_profile_ref_key.as_deref() {
            let snapshot = capture_global_edgion_config_data_snapshot();
            let active = self.resolve_active_profile_from_snapshot(&snapshot, selector_key);
            let Some(profile) = self.base.profiles.get(active) else {
                // Should not happen after validate_and_init + membership check, but guard defensively.
                record_conn_ip_restriction_event("fail_closed_profile_missing");
                return StreamPluginResult::Deny("ip:deny:profile-missing".to_string());
            };
            let (deny_keys, allow_keys) = self.active_profile_ref_keys(active);
            return self.evaluate_profile(profile, deny_keys, allow_keys, Some(&snapshot), ctx);
        }

        let active = self.base.active_profile.as_str();
        let Some(profile) = self.base.profiles.get(active) else {
            // Should not happen after validate_and_init, but guard defensively.
            record_conn_ip_restriction_event("fail_closed_profile_missing");
            return StreamPluginResult::Deny("ip:deny:profile-missing".to_string());
        };
        let (deny_keys, allow_keys) = self.active_profile_ref_keys(active);
        let snapshot = capture_global_edgion_config_data_snapshot_for_refs(deny_keys, allow_keys);
        self.evaluate_profile(profile, deny_keys, allow_keys, snapshot.as_ref(), ctx)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/stream/global_connection_ip_restriction/plugin_test.rs"]
mod tests;
