//! Preparse stamping for the controller plugin policy: clear any inbound
//! `policy_action` (anti-spoofing on non-K8s ingest paths), re-stamp from the
//! compiled policy, and build the PluginPolicyRestricted condition.

use edgion_resources::resources::plugin_policy::{PluginFamily, PluginPolicyAction};
use edgion_resources::resources::{EdgionPlugins, EdgionStreamPlugins};

use super::status_utils::{condition_reasons, condition_true, condition_types, update_condition};
use crate::config::plugin_policy::{compiled_plugin_policy, CompiledPluginPolicy};
use edgion_resources::resources::common::Condition;

fn describe(action: &PluginPolicyAction) -> String {
    match action {
        PluginPolicyAction::Block { status } => format!("block({status})"),
        PluginPolicyAction::Bypass => "bypass".to_string(),
    }
}

/// Clear + stamp one stage's entries. Generic over the entry type via a
/// macro because the six entry structs share field names but no trait.
macro_rules! stamp_stage {
    ($entries:expr, $family:expr, $label:expr, $policy:expr, $hits:expr) => {
        if let Some(entries) = $entries {
            for (i, entry) in entries.iter_mut().enumerate() {
                entry.policy_action = None; // unconditional anti-spoofing clear
                if !entry.enable {
                    continue; // already off: no stamp, no status noise
                }
                if let Some(pol) = $policy {
                    if let Some(action) = pol.action_for($family, entry.type_name()) {
                        $hits.push(format!(
                            "{}[{}] {}/{} -> {}",
                            $label,
                            i,
                            $family.as_str(),
                            entry.type_name(),
                            describe(&action)
                        ));
                        entry.policy_action = Some(action);
                    }
                }
            }
        }
    };
}

/// Testable core: policy passed explicitly.
pub(crate) fn stamp_http_entries_with(ep: &mut EdgionPlugins, policy: Option<&CompiledPluginPolicy>) -> Vec<String> {
    let mut hits = Vec::new();
    stamp_stage!(
        ep.spec.request_plugins.as_mut(),
        PluginFamily::Http,
        "requestPlugins",
        policy,
        hits
    );
    stamp_stage!(
        ep.spec.upstream_response_filter_plugins.as_mut(),
        PluginFamily::Http,
        "upstreamResponseFilterPlugins",
        policy,
        hits
    );
    stamp_stage!(
        ep.spec.upstream_response_body_filter_plugins.as_mut(),
        PluginFamily::Http,
        "upstreamResponseBodyFilterPlugins",
        policy,
        hits
    );
    stamp_stage!(
        ep.spec.upstream_response_plugins.as_mut(),
        PluginFamily::Http,
        "upstreamResponsePlugins",
        policy,
        hits
    );
    hits
}

/// Production entry point: reads the global compiled policy.
pub(crate) fn stamp_http_entries(ep: &mut EdgionPlugins) -> Vec<String> {
    stamp_http_entries_with(ep, compiled_plugin_policy())
}

pub(crate) fn stamp_stream_entries_with(
    esp: &mut EdgionStreamPlugins,
    policy: Option<&CompiledPluginPolicy>,
) -> Vec<String> {
    let mut hits = Vec::new();
    stamp_stage!(esp.spec.plugins.as_mut(), PluginFamily::Stream, "plugins", policy, hits);
    stamp_stage!(
        esp.spec.tls_route_plugins.as_mut(),
        PluginFamily::TlsRoute,
        "tlsRoutePlugins",
        policy,
        hits
    );
    hits
}

pub(crate) fn stamp_stream_entries(esp: &mut EdgionStreamPlugins) -> Vec<String> {
    stamp_stream_entries_with(esp, compiled_plugin_policy())
}

/// `Some(condition)` when hits exist; callers MUST remove the condition
/// themselves when `None` (upsert-only pipelines wedge otherwise — see spec).
pub(crate) fn policy_condition(hits: &[String], generation: Option<i64>) -> Option<Condition> {
    if hits.is_empty() {
        return None;
    }
    Some(condition_true(
        condition_types::PLUGIN_POLICY_RESTRICTED,
        condition_reasons::RESTRICTED_BY_POLICY,
        hits.join("; "),
        generation,
    ))
}

/// Apply the condition (upsert on hits, retain-remove on zero hits).
pub(crate) fn apply_policy_condition(conditions: &mut Vec<Condition>, hits: &[String], generation: Option<i64>) {
    match policy_condition(hits, generation) {
        Some(c) => update_condition(conditions, c),
        None => conditions.retain(|c| c.type_ != condition_types::PLUGIN_POLICY_RESTRICTED),
    }
}

/// Re-derive hit descriptions from stamped entries (used by update_status,
/// which does not receive preparse-local state).
pub(crate) fn collect_policy_hits_http(ep: &EdgionPlugins) -> Vec<String> {
    let mut hits = Vec::new();
    macro_rules! scan {
        ($entries:expr, $label:expr) => {
            if let Some(entries) = $entries {
                for (i, entry) in entries.iter().enumerate() {
                    if let Some(action) = &entry.policy_action {
                        hits.push(format!(
                            "{}[{}] http/{} -> {}",
                            $label,
                            i,
                            entry.type_name(),
                            describe(action)
                        ));
                    }
                }
            }
        };
    }
    scan!(ep.spec.request_plugins.as_ref(), "requestPlugins");
    scan!(
        ep.spec.upstream_response_filter_plugins.as_ref(),
        "upstreamResponseFilterPlugins"
    );
    scan!(
        ep.spec.upstream_response_body_filter_plugins.as_ref(),
        "upstreamResponseBodyFilterPlugins"
    );
    scan!(ep.spec.upstream_response_plugins.as_ref(), "upstreamResponsePlugins");
    hits
}

/// Re-derive hit descriptions from stamped entries (used by update_status,
/// which does not receive preparse-local state). Mirrors `collect_policy_hits_http`
/// with the two stream stages and their per-stage family.
pub(crate) fn collect_policy_hits_stream(esp: &EdgionStreamPlugins) -> Vec<String> {
    let mut hits = Vec::new();
    macro_rules! scan {
        ($entries:expr, $label:expr, $family:expr) => {
            if let Some(entries) = $entries {
                for (i, entry) in entries.iter().enumerate() {
                    if let Some(action) = &entry.policy_action {
                        hits.push(format!(
                            "{}[{}] {}/{} -> {}",
                            $label,
                            i,
                            $family.as_str(),
                            entry.type_name(),
                            describe(action)
                        ));
                    }
                }
            }
        };
    }
    scan!(esp.spec.plugins.as_ref(), "plugins", PluginFamily::Stream);
    scan!(
        esp.spec.tls_route_plugins.as_ref(),
        "tlsRoutePlugins",
        PluginFamily::TlsRoute
    );
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::plugin_policy::{compile, PluginPolicyConfig};

    fn policy(yaml: &str) -> CompiledPluginPolicy {
        compile(&serde_yaml::from_str::<PluginPolicyConfig>(yaml).unwrap()).unwrap()
    }

    fn ep_with_wasm_and_cors() -> EdgionPlugins {
        serde_yaml::from_str(
            r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata: { name: t, namespace: default }
spec:
  requestPlugins:
    - type: Cors
      config: {}
    - type: Wasm
      config: { source: { inline: "AGFzbQEAAAA=" } }
    - enable: false
      type: Wasm
      config: { source: { inline: "AGFzbQEAAAA=" } }
"#,
        )
        .unwrap()
    }

    #[test]
    fn stamps_hit_skips_disabled_and_clears_spoofed() {
        let mut ep = ep_with_wasm_and_cors();
        // Spoof: pretend a user hand-wrote a bypass on the Cors entry.
        ep.spec.request_plugins.as_mut().unwrap()[0].policy_action = Some(PluginPolicyAction::Bypass);

        let pol = policy("mode: denylist\nplugins:\n  - name: http/Wasm\n");
        let hits = stamp_http_entries_with(&mut ep, Some(&pol));

        let entries = ep.spec.request_plugins.as_ref().unwrap();
        assert_eq!(entries[0].policy_action, None, "spoofed stamp must be cleared");
        assert_eq!(
            entries[1].policy_action,
            Some(PluginPolicyAction::Block { status: 503 })
        );
        assert_eq!(entries[2].policy_action, None, "enable:false entries are not stamped");
        assert_eq!(hits, vec!["requestPlugins[1] http/Wasm -> block(503)".to_string()]);
    }

    #[test]
    fn no_policy_clears_everything_and_reports_no_hits() {
        let mut ep = ep_with_wasm_and_cors();
        ep.spec.request_plugins.as_mut().unwrap()[1].policy_action = Some(PluginPolicyAction::Bypass);
        let hits = stamp_http_entries_with(&mut ep, None);
        assert!(hits.is_empty());
        assert!(ep
            .spec
            .request_plugins
            .as_ref()
            .unwrap()
            .iter()
            .all(|e| e.policy_action.is_none()));
    }

    #[test]
    fn policy_condition_present_only_on_hits() {
        assert!(policy_condition(&[], Some(1)).is_none());
        let c = policy_condition(&["requestPlugins[1] http/Wasm -> block(503)".to_string()], Some(1)).unwrap();
        assert_eq!(c.type_, "PluginPolicyRestricted");
        assert_eq!(c.status, "True");
        assert!(c.message.contains("http/Wasm"));
    }

    #[test]
    fn stream_stamping_covers_both_stages_and_families() {
        // Config content is irrelevant to stamping; shapes below mirror the
        // valid configs used by edgion_stream_plugins.rs's own preparse tests
        // (ConnectionRateLimit needs no required fields; IpRestriction needs
        // `allow`/`deny` as `Vec<IpGroup>`, not bare CIDR strings).
        let mut esp: EdgionStreamPlugins = serde_yaml::from_str(
            r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata: { name: t, namespace: default }
spec:
  plugins:
    - type: ConnectionRateLimit
      config: {}
  tlsRoutePlugins:
    - type: IpRestriction
      config:
        allow:
          - name: default
            cidrs: ["10.0.0.0/8"]
        defaultAction: deny
"#,
        )
        .unwrap();
        let pol = policy(
            "mode: denylist\nplugins:\n  - name: stream/ConnectionRateLimit\n    action: bypass\n  - name: tls-route/IpRestriction\n",
        );
        let hits = stamp_stream_entries_with(&mut esp, Some(&pol));
        assert_eq!(hits.len(), 2);
        assert!(hits[0].starts_with("plugins[0] stream/ConnectionRateLimit -> bypass"));
        assert!(hits[1].starts_with("tlsRoutePlugins[0] tls-route/IpRestriction -> block(503)"));
        assert_eq!(hits, collect_policy_hits_stream(&esp));
    }
}
