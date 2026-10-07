//! Canary runtime plugin (L7 request stage).
//!
//! Resolves the active profile via the Selector EdgionConfigData store
//! (fail-to-base, mirrors RequestAccessPolicy::resolve_active_profile_name
//! exactly, including the ref_denied filter), then jumps 100% of the
//! request's traffic to that profile's backendRef. Unlike
//! RequestAccessPolicy (which fails OPEN on an unresolvable profile), an
//! invalid *resolved backend* (not present in the route's backendRefs)
//! follows the configured `on_invalid` behavior (Reject default / Fallback).

use async_trait::async_trait;

use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::routes::http::routes_mgr::RouteRules;
use crate::{InternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigDataRef};
use edgion_resources::resources::edgion_plugins::plugin_configs::{CanaryConfig, DynUpstreamTarget, UpstreamFallback};

/// Resolve the "namespace/name" Selector store key for a (possibly denied)
/// `activeProfileRef`. Returns `None` when no ref is configured OR the ref
/// was denied by the controller (cross-namespace, no ReferenceGrant) — both
/// cases must fall back to the base `active_profile`, so they collapse to
/// the same `None` (mirrors `RequestAccessPolicy::create` exactly). Extracted
/// as a free function so this security-relevant gate (a denied ref must
/// never resolve to a live store key — cross-namespace authorization bypass
/// otherwise) is directly unit-testable without downcasting the
/// `Box<dyn RequestFilter>` that `Canary::create()` returns.
fn resolve_active_profile_ref_key(active_profile_ref: &Option<EdgionConfigDataRef>, namespace: &str) -> Option<String> {
    active_profile_ref.as_ref().filter(|r| r.ref_denied.is_none()).map(|r| {
        let ns = r.namespace.as_deref().unwrap_or(namespace);
        format!("{}/{}", ns, r.name)
    })
}

/// Canary L7 traffic-switch plugin.
pub struct Canary {
    name: String,
    config: CanaryConfig,
    /// Resolved "namespace/name" key for the activeProfileRef Selector
    /// (built once at construction; re-looked-up live each request for hot
    /// reload). `None` when no selector is configured OR the ref was
    /// denied by the controller (cross-namespace, no ReferenceGrant) —
    /// both cases must use the base `active_profile`, so they collapse to
    /// the same `None` (mirrors `RequestAccessPolicy::create` exactly).
    active_profile_ref_key: Option<String>,
}

impl Canary {
    pub fn create(config: &CanaryConfig, namespace: &str) -> Box<dyn RequestFilter> {
        let config = config.clone();

        let active_profile_ref_key = resolve_active_profile_ref_key(&config.active_profile_ref, namespace);

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

        if RouteRules::find_backend_by_name(route_unit, &target.name, target.namespace.as_deref()).is_err() {
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
