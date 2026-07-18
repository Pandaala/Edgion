//! Read-only "effective" resolution endpoints for the Center dashboard.
//!
//! These reuse the same merge/resolution rules as the gateway data plane
//! (`merge_region_route_override` and the Selector fail-to-base rule) so the
//! controller is the single authority for the effective view; Center polls
//! these instead of re-deriving the merge.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use edgion_resources::prelude_resources::EdgionStreamPlugins;
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigData, Selector};
use edgion_resources::resources::edgion_stream_plugins::{EdgionStreamPlugin, ProfileRules};
use edgion_resources::ResourceKind;
use serde::Serialize;

use super::types::AdminState;

/// One resolved GlobalConnectionIpRestriction plugin instance, after Selector resolution.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveGir {
    pub namespace: String,
    pub plugin_name: String,
    pub enable: bool,
    /// Effective active profile after Selector resolution (fail-to-base).
    pub active_profile: String,
    pub profiles: HashMap<String, ProfileRules>,
    pub active_profile_ref: Option<EffectiveConfigDataRef>,
    /// True if a Selector overlay successfully changed the active profile.
    pub selector_applied: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveConfigDataRef {
    pub namespace: String,
    pub name: String,
    pub permitted: bool,
}

/// Resolve effective GIR view from a set of EdgionStreamPlugins.
/// Selector rule: if `Selector.active = Some(name)` and `name ∈ profiles` ⇒ use name;
/// otherwise fall back to base `active_profile` (fail-to-base).
/// Entries with `enable == false` are skipped (not effective).
pub fn resolve_gir_effective(
    plugins: &[EdgionStreamPlugins],
    lookup: &dyn Fn(&str, &str) -> Option<Selector>,
) -> Vec<EffectiveGir> {
    let mut out = Vec::new();
    for esp in plugins {
        let ns = esp.metadata.namespace.as_deref().unwrap_or("default");
        let plugin_name = esp.metadata.name.clone().unwrap_or_default();
        for entry in esp.spec.plugins.iter().flatten() {
            if !entry.enable {
                continue;
            }
            if let EdgionStreamPlugin::GlobalConnectionIpRestriction(ref gir) = entry.plugin {
                let mut active = gir.active_profile.clone();
                let mut applied = false;
                let mut ref_name = None;
                if let Some(ref aref) = gir.active_profile_ref {
                    let aref_ns = aref.namespace.as_deref().unwrap_or(ns);
                    let permitted = aref.ref_denied.is_none();
                    ref_name = Some(EffectiveConfigDataRef {
                        namespace: aref_ns.to_string(),
                        name: aref.name.clone(),
                        permitted,
                    });
                    if permitted {
                        if let Some(sel) = lookup(aref_ns, &aref.name) {
                            if let Some(cand) = sel.active.as_ref() {
                                if gir.profiles.contains_key(cand) {
                                    active = cand.clone();
                                    applied = true;
                                }
                            }
                        }
                    }
                }
                out.push(EffectiveGir {
                    namespace: ns.to_string(),
                    plugin_name: plugin_name.clone(),
                    enable: gir.enable,
                    active_profile: active,
                    profiles: gir.profiles.clone(),
                    active_profile_ref: ref_name,
                    selector_applied: applied,
                });
            }
        }
    }
    out
}

/// GET /api/v1/global-ip-restrictions/effective
///
/// Returns the effective GlobalConnectionIpRestriction view by combining each
/// `EdgionStreamPlugins` resource's base config with its `Selector` EdgionConfigData
/// (fail-to-base on missing, unknown profile, or wrong-type overlay).
pub async fn global_ip_restrictions_effective(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let items = state.runtime_list_resources(ResourceKind::EdgionStreamPlugins)?;
    let plugins: Vec<EdgionStreamPlugins> = items
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();
    let rt = state.runtime();
    let lookup = |ns: &str, name: &str| -> Option<Selector> {
        rt.edgion_config_data_store
            .get(Some(ns), name)
            .and_then(|cd| enabled_selector(&cd))
    };
    let effective = resolve_gir_effective(&plugins, &lookup);
    Ok(Json(serde_json::json!({ "success": true, "data": effective })))
}

fn enabled_selector(config_data: &EdgionConfigData) -> Option<Selector> {
    if !config_data.spec.enable {
        return None;
    }
    match &config_data.spec.data {
        ConfigEntry::Selector(selector) => Some(selector.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use edgion_resources::resources::edgion_config_data::EdgionConfigDataSpec;
    use edgion_resources::resources::edgion_stream_plugins::{
        DefaultAction, EdgionStreamPluginsSpec, GlobalConnectionIpRestrictionConfig, ProfileRules as PR,
        StreamPluginEntry,
    };

    fn gir_esp(name: &str, aref: Option<&str>) -> EdgionStreamPlugins {
        let mut esp = EdgionStreamPlugins::new(
            name,
            EdgionStreamPluginsSpec {
                plugins: None,
                tls_route_plugins: None,
            },
        );
        esp.metadata.namespace = Some("default".to_string());
        let mut profiles = std::collections::HashMap::new();
        profiles.insert(
            "strict".to_string(),
            PR {
                allow: None,
                deny: None,
                default_action: DefaultAction::Deny,
                allow_refs: None,
                deny_refs: None,
                allow_matcher: None,
                deny_matcher: None,
            },
        );
        profiles.insert(
            "open".to_string(),
            PR {
                allow: None,
                deny: None,
                default_action: DefaultAction::Allow,
                allow_refs: None,
                deny_refs: None,
                allow_matcher: None,
                deny_matcher: None,
            },
        );
        let gir = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "strict".to_string(),
            profiles,
            description: None,
            active_profile_ref: aref.map(
                |n| edgion_resources::resources::edgion_config_data::EdgionConfigDataRef {
                    name: n.to_string(),
                    namespace: None,
                    ..Default::default()
                },
            ),
        };
        esp.spec.plugins = Some(vec![StreamPluginEntry {
            enable: true,
            policy_action: None,
            plugin: EdgionStreamPlugin::GlobalConnectionIpRestriction(gir),
        }]);
        esp
    }

    #[test]
    fn gir_selector_switches_profile() {
        let esps = vec![gir_esp("g", Some("sel"))];
        let result = resolve_gir_effective(&esps, &|_, _| {
            Some(Selector {
                active: Some("open".to_string()),
                description: None,
            })
        });
        assert_eq!(result[0].active_profile, "open");
        assert!(result[0].selector_applied);
        let reference = result[0]
            .active_profile_ref
            .as_ref()
            .expect("effective view keeps selector identity");
        assert_eq!(reference.namespace, "default");
        assert_eq!(reference.name, "sel");
        assert!(reference.permitted);
    }

    #[test]
    fn gir_unknown_profile_falls_back() {
        let esps = vec![gir_esp("g", Some("sel"))];
        let result = resolve_gir_effective(&esps, &|_, _| {
            Some(Selector {
                active: Some("ghost".to_string()),
                description: None,
            })
        });
        assert_eq!(result[0].active_profile, "strict");
        assert!(!result[0].selector_applied);
    }

    #[test]
    fn disabled_selector_is_ignored() {
        let mut selector = EdgionConfigData::new(
            "selector",
            EdgionConfigDataSpec {
                enable: false,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::Selector(Selector {
                    active: Some("open".to_string()),
                    description: None,
                }),
            },
        );
        assert!(enabled_selector(&selector).is_none());
        selector.spec.enable = true;
        assert_eq!(
            enabled_selector(&selector).and_then(|value| value.active),
            Some("open".to_string())
        );
    }
}
