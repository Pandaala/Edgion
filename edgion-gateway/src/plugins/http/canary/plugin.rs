//! Canary runtime plugin (L7 request stage).
//!
//! Resolves the active profile via the Selector EdgionConfigData store
//! (fail-to-base, mirrors GlobalAccessControl::resolve_active_profile_name
//! exactly, including the ref_denied filter), then jumps 100% of the
//! request's traffic to that profile's backendRef. Unlike
//! GlobalAccessControl (which fails OPEN on an unresolvable profile), an
//! invalid *resolved backend* (not present in the route's backendRefs)
//! follows the configured `on_invalid` behavior (Reject default / Fallback).

use async_trait::async_trait;

use crate::plugins::runtime::dye_headers::{apply_dye_headers, strip_request_dye_headers};
use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::{InternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigDataRef};
use edgion_resources::resources::edgion_plugins::plugin_configs::{CanaryConfig, DynUpstreamOnInvalid};

/// Resolve the "namespace/name" Selector store key for a (possibly denied)
/// `activeProfileRef`. Returns `None` when no ref is configured OR the ref
/// was denied by the controller (cross-namespace, no ReferenceGrant) — both
/// cases must fall back to the base `active_profile`, so they collapse to
/// the same `None` (mirrors `GlobalAccessControl::create` exactly). Extracted
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
    /// the same `None` (mirrors `GlobalAccessControl::create` exactly).
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

    /// Resolve the active profile name via the Selector store, with
    /// fail-to-base to `config.active_profile` on any error/absence.
    ///
    /// Each fail-to-base branch emits a distinct `selector miss: <reason>` log
    /// tag so operators debugging a canary switch that didn't take can grep
    /// the access log for the exact failure mode (per project convention:
    /// data-plane observability signals live in PluginLog / access log, not
    /// in Prometheus counters — see skills/03-coding/observability/01-metrics.md).
    fn resolve_active_profile_name(&self, plugin_log: &mut PluginLog) -> String {
        let Some(ref key) = self.active_profile_ref_key else {
            return self.config.active_profile.clone();
        };
        let store = get_global_edgion_config_data_store();
        let raw_ed = store.get_raw_by_key(key);

        // Envelope kill-switch: spec.enable=false => ignore the selector, use base.
        if raw_ed.as_ref().map(|ed| !ed.spec.enable).unwrap_or(false) {
            plugin_log.push("selector miss: disabled");
            return self.config.active_profile.clone();
        }
        match raw_ed.as_deref().map(|ed| &ed.spec.data) {
            None => {
                plugin_log.push("selector miss: absent");
                self.config.active_profile.clone()
            }
            Some(ConfigEntry::Selector(sel)) => match sel.active.as_deref() {
                // Deliberate no-op (use base) — not a miss.
                None => self.config.active_profile.clone(),
                Some(name) if self.config.profiles.contains_key(name) => name.to_string(),
                Some(_) => {
                    plugin_log.push("selector miss: unknown_profile");
                    self.config.active_profile.clone()
                }
            },
            Some(_) => {
                plugin_log.push("selector miss: type_mismatch");
                self.config.active_profile.clone()
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

        // Anti-spoofing: a client-sent header with a configured dye name must
        // never pass through on any path (fallback/reject included).
        if let Some(dye) = &self.config.dye_headers {
            strip_request_dye_headers(session, dye);
        }

        let profile_name = self.resolve_active_profile_name(log);
        let Some(target) = self.config.profiles.get(&profile_name) else {
            // Unreachable post-validation: resolve_active_profile_name only
            // ever returns a name present in `profiles` (checked inline
            // above) or the parse-time-validated `active_profile`. Fail
            // open if this invariant ever breaks, mirroring
            // GlobalAccessControl's "profile missing: fail-open" arm.
            log.push_err("profile missing: fail-open");
            return PluginRunningResult::GoodNext;
        };

        // Pre-validate that the resolved target exists in the route's backend_refs
        // (same check DynamicInternalUpstream::run_request performs).
        let ctx = session.ctx();
        let route_unit = match ctx.route_unit.as_ref() {
            Some(unit) => unit,
            None => {
                log.push("no route");
                return PluginRunningResult::GoodNext;
            }
        };
        let route_ns = &route_unit.matched_info.rsc.ns;
        let backend_refs = match route_unit.rule.backend_refs.as_ref() {
            Some(refs) if !refs.is_empty() => refs,
            _ => {
                log.push("no backend refs");
                return PluginRunningResult::GoodNext;
            }
        };

        let found = backend_refs.iter().any(|br| {
            if br.name != target.name {
                return false;
            }
            match target.namespace.as_deref() {
                Some(ns) => br.namespace.as_deref().unwrap_or(route_ns) == ns,
                None => true,
            }
        });

        if !found {
            log.push_err("backend ref not found");
            return match self.config.on_invalid {
                DynUpstreamOnInvalid::Reject => PluginRunningResult::ErrResponse {
                    status: 403,
                    body: Some("Target backend not allowed".to_string()),
                },
                DynUpstreamOnInvalid::Fallback => PluginRunningResult::GoodNext,
            };
        }

        let info = InternalJumpPreset {
            backend_ref_name: target.name.clone(),
            backend_ref_namespace: target.namespace.clone(),
        };
        match session.set_upstream_jump(UpstreamJumpTarget::Internal(info)) {
            SetJumpOutcome::Set => {
                log.push("upstream selected");
            }
            SetJumpOutcome::Rejected => {
                log.push("jump locked, rejected");
                return PluginRunningResult::GoodNext;
            }
        }

        if let Some(dye) = &self.config.dye_headers {
            apply_dye_headers(session, dye, &profile_name, log);
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigData, EdgionConfigDataSpec, Selector,
    };
    use edgion_resources::resources::edgion_plugins::plugin_configs::DynUpstreamTarget;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn target(name: &str) -> DynUpstreamTarget {
        DynUpstreamTarget {
            name: name.to_string(),
            namespace: None,
        }
    }

    fn base_config() -> CanaryConfig {
        CanaryConfig {
            enable: true,
            active_profile: "stable".to_string(),
            profiles: HashMap::from([
                ("stable".to_string(), target("app-v1")),
                ("canary".to_string(), target("app-v2")),
            ]),
            active_profile_ref: None,
            on_invalid: DynUpstreamOnInvalid::default(),
            dye_headers: None,
        }
    }

    fn canary_with_key(config: CanaryConfig, key: Option<&str>) -> Canary {
        Canary {
            name: "Canary".to_string(),
            config,
            active_profile_ref_key: key.map(|k| k.to_string()),
        }
    }

    #[test]
    fn no_selector_uses_active_profile() {
        let plugin = canary_with_key(base_config(), None);
        let mut log = PluginLog::new("Canary");
        assert_eq!(plugin.resolve_active_profile_name(&mut log), "stable");
    }

    #[test]
    fn selector_missing_falls_back_to_base() {
        let plugin = canary_with_key(base_config(), Some("unit-test-ns/canary-selector-absent"));
        let mut log = PluginLog::new("Canary");
        assert_eq!(plugin.resolve_active_profile_name(&mut log), "stable");
        assert!(
            log.contains("selector miss: absent"),
            "absent-store-entry branch must emit the distinct `absent` log tag for access-log grep"
        );
    }

    #[test]
    fn selector_active_overrides_base() {
        let test_key = "unit-test-ns/canary-selector-override";
        let mut ed = EdgionConfigData::new(
            "canary-selector-override",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::Selector(Selector {
                    active: Some("canary".to_string()),
                    description: None,
                }),
            },
        );
        ed.metadata.namespace = Some("unit-test-ns".to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

        let plugin = canary_with_key(base_config(), Some(test_key));
        let mut log = PluginLog::new("Canary");
        assert_eq!(plugin.resolve_active_profile_name(&mut log), "canary");
    }

    #[test]
    fn selector_kill_switch_falls_back_to_base() {
        let test_key = "unit-test-ns/canary-selector-killswitch";
        let mut ed = EdgionConfigData::new(
            "canary-selector-killswitch",
            EdgionConfigDataSpec {
                enable: false, // envelope kill-switch
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::Selector(Selector {
                    active: Some("canary".to_string()),
                    description: None,
                }),
            },
        );
        ed.metadata.namespace = Some("unit-test-ns".to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

        let plugin = canary_with_key(base_config(), Some(test_key));
        let mut log = PluginLog::new("Canary");
        assert_eq!(plugin.resolve_active_profile_name(&mut log), "stable");
        assert!(
            log.contains("selector miss: disabled"),
            "envelope-kill-switch branch must emit the distinct `disabled` log tag for access-log grep"
        );
    }

    #[test]
    fn selector_unknown_name_falls_back_to_base() {
        let test_key = "unit-test-ns/canary-selector-unknown";
        let mut ed = EdgionConfigData::new(
            "canary-selector-unknown",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::Selector(Selector {
                    active: Some("ghost".to_string()),
                    description: None,
                }),
            },
        );
        ed.metadata.namespace = Some("unit-test-ns".to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

        let plugin = canary_with_key(base_config(), Some(test_key));
        let mut log = PluginLog::new("Canary");
        assert_eq!(plugin.resolve_active_profile_name(&mut log), "stable");
        assert!(
            log.contains("selector miss: unknown_profile"),
            "unknown-active-name branch must emit the distinct `unknown_profile` log tag for access-log grep"
        );
    }

    #[test]
    fn selector_none_active_is_noop_uses_base() {
        let test_key = "unit-test-ns/canary-selector-none-active";
        let mut ed = EdgionConfigData::new(
            "canary-selector-none-active",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::Selector(Selector {
                    active: None,
                    description: None,
                }),
            },
        );
        ed.metadata.namespace = Some("unit-test-ns".to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

        let plugin = canary_with_key(base_config(), Some(test_key));
        let mut log = PluginLog::new("Canary");
        assert_eq!(plugin.resolve_active_profile_name(&mut log), "stable");
        assert!(
            !log.contains("selector miss"),
            "`active: None` is a deliberate no-op and must NOT emit any selector-miss tag"
        );
    }

    /// Regression: an `EdgionConfigData` present at the referenced key but carrying a
    /// non-`Selector` variant (e.g. a `KeyList` misconfigured under the Canary ref) must
    /// fail-to-base AND emit the distinct `type_mismatch` tag so operators can grep
    /// this specific misconfiguration in the access log.
    #[test]
    fn selector_wrong_type_falls_back_and_logs_type_mismatch() {
        use edgion_resources::resources::edgion_config_data::{KeyListData, KeyMatchMode};

        let test_key = "unit-test-ns/canary-selector-wrongtype";
        let mut ed = EdgionConfigData::new(
            "canary-selector-wrongtype",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::KeyList(KeyListData {
                    match_mode: KeyMatchMode::Exact,
                    // Wrong variant is the whole point — items list can be empty.
                    items: vec![],
                }),
            },
        );
        ed.metadata.namespace = Some("unit-test-ns".to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

        let plugin = canary_with_key(base_config(), Some(test_key));
        let mut log = PluginLog::new("Canary");
        assert_eq!(plugin.resolve_active_profile_name(&mut log), "stable");
        assert!(
            log.contains("selector miss: type_mismatch"),
            "wrong-ConfigEntry-variant branch must emit the distinct `type_mismatch` log tag for access-log grep"
        );
    }

    /// A denied (`ref_denied: Some`) `active_profile_ref` must not be resolved
    /// as a store key — `resolve_active_profile_ref_key` (the exact function
    /// `Canary::create()` calls to build `active_profile_ref_key`) must apply
    /// the `.filter(|r| r.ref_denied.is_none())` gate `GlobalAccessControl::create`
    /// applies too, otherwise a cross-namespace ref the controller explicitly
    /// denied would still take effect at runtime. Unlike a test that hand-builds
    /// a separate `Canary` struct, this calls the very function `create()` uses,
    /// so deleting the `ref_denied` filter from `create()`/this function would
    /// fail this test.
    #[test]
    fn denied_active_profile_ref_resolves_to_no_store_key() {
        use edgion_resources::resources::common::RefDenied;

        let denied_ref = Some(EdgionConfigDataRef {
            name: "canary-selector-denied".to_string(),
            namespace: Some("other-ns".to_string()),
            ref_denied: Some(RefDenied {
                target_namespace: "other-ns".to_string(),
                target_name: "canary-selector-denied".to_string(),
                reason: Some("NoMatchingReferenceGrant".to_string()),
            }),
        });

        assert_eq!(
            resolve_active_profile_ref_key(&denied_ref, "unit-test-ns"),
            None,
            "a denied activeProfileRef must never resolve to a live store key"
        );
    }

    /// Sanity counterpart to the test above: a non-denied ref DOES resolve to a
    /// store key, proving the gate depends specifically on `ref_denied` (and
    /// isn't just an always-`None` stub that would make the denial test above
    /// vacuously pass).
    #[test]
    fn allowed_active_profile_ref_resolves_to_store_key() {
        let allowed_ref = Some(EdgionConfigDataRef {
            name: "canary-selector-allowed".to_string(),
            namespace: Some("other-ns".to_string()),
            ref_denied: None,
        });

        assert_eq!(
            resolve_active_profile_ref_key(&allowed_ref, "unit-test-ns"),
            Some("other-ns/canary-selector-allowed".to_string())
        );
    }
}
