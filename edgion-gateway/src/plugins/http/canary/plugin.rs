//! Canary runtime plugin (L7 request stage).
//!
//! A live Selector switches among authored profiles; unavailable selectors use the
//! base profile. RAP deliberately differs: its Selector failures deny requests.
//! Invalid selected backends follow Canary's configured on_invalid policy.

use async_trait::async_trait;

use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::routes::http::routes_mgr::RouteRules;
use crate::{InternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::refs::config_data_fields::HttpConfigDataEntry;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::plugin_configs::{CanaryConfig, DynUpstreamTarget, UpstreamFallback};
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

/// Canary L7 traffic-switch plugin.
pub struct Canary {
    name: String,
    config: CanaryConfig,
    /// Resolved "namespace/name" key for the activeProfileRef Selector
    /// (built once at construction; re-looked-up live each request for hot
    /// reload). `None` when no selector is configured OR the ref was
    /// denied by the controller (cross-namespace, no ReferenceGrant) —
    /// both cases must use the base `active_profile`, so they collapse to
    /// the same `None`; RAP instead denies on Selector failure.
    active_profile_ref_key: Option<String>,
}

impl Canary {
    pub(crate) fn create_bound(entry: HttpConfigDataEntry<'_>) -> Option<Box<dyn RequestFilter>> {
        let EdgionPlugin::Canary(config) = entry.plugin() else {
            return None;
        };
        if entry.has_references() {
            entry.validate_owner().ok()?;
        }
        let key = entry.field().and_then(|field| field.runtime_key().ok().flatten());
        Some(Self::build(config, key))
    }

    pub(crate) fn create_inline(config: &CanaryConfig) -> Option<Box<dyn RequestFilter>> {
        if config.active_profile_ref.is_some() {
            return None;
        }
        Some(Self::build(config, None))
    }

    fn build(config: &CanaryConfig, active_profile_ref_key: Option<String>) -> Box<dyn RequestFilter> {
        let config = config.clone();
        Box::new(Canary {
            name: "Canary".to_string(),
            config,
            active_profile_ref_key,
        })
    }

    /// Resolve the canonical active profile entry via the Selector store, with
    /// fail-to-base to `config.active_profile` on any error/absence.
    ///
    /// Each fail-to-base branch emits a distinct `selector miss: <reason>` log
    /// tag so operators debugging a canary switch that didn't take can grep
    /// the access log for the exact failure mode (per project convention:
    /// data-plane observability signals live in PluginLog / access log, not
    /// in Prometheus counters — see skills/03-coding/observability/01-metrics.md).
    fn resolve_active_profile<'a>(&'a self, plugin_log: &mut PluginLog) -> Option<(&'a str, &'a DynUpstreamTarget)> {
        let base_profile = || {
            self.config
                .profiles
                .get_key_value(&self.config.active_profile)
                .map(|(name, target)| (name.as_str(), target))
        };
        let Some(ref key) = self.active_profile_ref_key else {
            return base_profile();
        };
        let store = get_global_edgion_config_data_store();
        let raw_ed = store.get_raw_by_key(key);

        // Envelope kill-switch: spec.enable=false => ignore the selector, use base.
        if raw_ed.as_ref().map(|ed| !ed.spec.enable).unwrap_or(false) {
            plugin_log.push("selector miss: disabled");
            return base_profile();
        }
        match raw_ed.as_deref().map(|ed| &ed.spec.data) {
            None => {
                plugin_log.push("selector miss: absent");
                base_profile()
            }
            Some(ConfigEntry::Selector(sel)) => match sel.active.as_deref() {
                // Deliberate no-op (use base) — not a miss.
                None => base_profile(),
                Some(name) => match self.config.profiles.get_key_value(name) {
                    Some((name, target)) => Some((name.as_str(), target)),
                    None => {
                        plugin_log.push("selector miss: unknown_profile");
                        base_profile()
                    }
                },
            },
            Some(_) => {
                plugin_log.push("selector miss: type_mismatch");
                base_profile()
            }
        }
    }
}

#[async_trait]
impl RequestFilter for Canary {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Kill switch.
        if !self.config.enable {
            log.push("disabled:pass");
            return PluginRunningResult::GoodNext;
        }

        let Some((profile_name, target)) = self.resolve_active_profile(log) else {
            // Unreachable post-validation: active_profile is required to name
            // an entry in profiles. Fail open if this invariant ever breaks.
            log.push_err("profile missing: fail-open");
            return PluginRunningResult::GoodNext;
        };

        // Pre-validate the resolved target against the route's canonical
        // compiled backend view before publishing the jump.
        let ctx = session.ctx();
        let Some(route_unit) = ctx.route_unit.as_ref() else {
            log.push("no route");
            return PluginRunningResult::GoodNext;
        };
        match route_unit.rule.backend_refs.as_ref() {
            Some(refs) if !refs.is_empty() => {}
            _ => {
                log.push("no backend refs");
                return PluginRunningResult::GoodNext;
            }
        };

        if RouteRules::validate_backend_by_name(route_unit, &target.name, target.namespace.as_deref()).is_err() {
            log.push_err("backend ref not found");
            return match self.config.on_invalid {
                UpstreamFallback::Reject => {
                    session.record_outcome(DyeOutcome::Reject, "on-invalid");
                    PluginRunningResult::ErrResponse {
                        status: 403,
                        body: Some(Bytes::from_static(b"Target backend not allowed")),
                    }
                }
                UpstreamFallback::Fallback => {
                    session.record_outcome(DyeOutcome::Fallback, "on-invalid");
                    PluginRunningResult::GoodNext
                }
            };
        }

        let info = InternalJumpPreset {
            backend_ref_name: target.name.clone(),
            backend_ref_namespace: target.namespace.clone(),
        };
        match session.set_upstream_jump(UpstreamJumpTarget::Internal(info)) {
            SetJumpOutcome::Set => {
                session.record_outcome(DyeOutcome::Success, profile_name);
                log.push("upstream selected");
            }
            SetJumpOutcome::Rejected => {
                log.push("jump locked, rejected");
                return PluginRunningResult::GoodNext;
            }
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/canary/plugin_test.rs"]
mod tests;
