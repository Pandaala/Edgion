//! Named GCIR policy selection with reference-only canonical IpList collections.
//! Genuine Selector errors preserve the established fail-to-base diagnostic policy.
use crate::observe::metrics::record_conn_ip_restriction_event;
use crate::plugins::common::ip_acl::CompiledIpAcl;
use crate::plugins::runtime::edgion_config_data::{
    capture_global_edgion_config_data_snapshot, EdgionConfigDataSnapshot,
};
use crate::plugins::stream::stream_plugin_trait::{
    access_decision_to_stream_result, StreamContext, StreamPlugin, StreamPluginResult,
};
use async_trait::async_trait;
use edgion_resources::refs::ip_list_fields::GlobalIpListEntry;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::PluginValidation;
use std::collections::HashMap;
use std::sync::Arc;
const PLUGIN_NAME: &str = "GlobalConnectionIpRestriction";
pub struct GlobalConnectionIpRestrictionPlugin {
    enable: bool,
    active_profile: String,
    profiles: HashMap<String, Arc<CompiledIpAcl>>,
    active_profile_ref_key: Option<String>,
}
impl GlobalConnectionIpRestrictionPlugin {
    pub(crate) fn from_bound(entry: GlobalIpListEntry<'_>) -> Result<Self, String> {
        let config = entry.config();
        if let Some(error) = config.validation_error() {
            return Err(error);
        }
        let selector = entry.selector();
        if let Some(field) = &selector {
            field.validate_owner().map_err(|error| error.to_string())?;
        }
        let active_profile_ref_key = selector.and_then(|field| field.runtime_key().ok().flatten());
        let profiles = if config.enable {
            entry
                .profiles()
                .map(|(name, refs)| {
                    CompiledIpAcl::from_bound(refs, &config.profiles[name].default_action)
                        .map(|plan| (name.to_owned(), plan))
                })
                .collect::<Result<HashMap<_, _>, _>>()
                .map_err(|error| error.to_string())?
        } else {
            HashMap::new()
        };
        Ok(Self {
            enable: config.enable,
            active_profile: config.active_profile.clone(),
            profiles,
            active_profile_ref_key,
        })
    }
    /// Resolve the effective active-profile **name** for this connection.
    ///
    /// A selector can only choose another git-owned profile, so this returns just the profile
    /// name — no rule-data clone on the hot path. Every error / `active: null` / disabled path
    /// returns the base default (`active_profile`), i.e. fail-to-base. The caller indexes
    /// `self.profiles` with the returned name (matchers already compiled at construction).
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
            return self.active_profile.as_str();
        }

        match raw_ed.map(|ed| &ed.spec.data) {
            None => {
                // activeProfileRef set but EdgionConfigData not found in the store.
                record_conn_ip_restriction_event("overlay_missing");
                self.active_profile.as_str()
            }
            Some(ConfigEntry::Selector(sel)) => match sel.active.as_deref() {
                // `active: null` / absent → deliberate no-op (use base).
                None => self.active_profile.as_str(),
                // Valid switch among the git-owned profiles.
                Some(name) if self.profiles.contains_key(name) => name,
                // Selector names a profile that does not exist in the git-owned profiles.
                Some(_) => {
                    record_conn_ip_restriction_event("overlay_invalid");
                    self.active_profile.as_str()
                }
            },
            Some(_) => {
                // EdgionConfigData found but it is not a Selector variant.
                record_conn_ip_restriction_event("overlay_type_mismatch");
                self.active_profile.as_str()
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
        if !self.enable {
            return StreamPluginResult::Allow;
        }
        // Profile selection and all list reads use this one owned generation.
        let view = capture_global_edgion_config_data_snapshot();
        let active = self
            .active_profile_ref_key
            .as_deref()
            .map(|key| self.resolve_active_profile_from_snapshot(&view, key))
            .unwrap_or(self.active_profile.as_str());
        let Some(plan) = self.profiles.get(active) else {
            record_conn_ip_restriction_event("fail_closed_profile_missing");
            return StreamPluginResult::Deny("ip:deny:profile-missing".to_string());
        };
        access_decision_to_stream_result(plan.evaluate_in(Some(&view), Some(ctx.direct_peer_ip)), || {
            record_conn_ip_restriction_event("denied")
        })
    }
}
#[cfg(test)]
#[path = "../../../../tests/unit/plugins/stream/global_connection_ip_restriction/plugin_test.rs"]
mod tests;
