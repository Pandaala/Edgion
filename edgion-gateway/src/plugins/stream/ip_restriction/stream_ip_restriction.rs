//! Stream IP Restriction plugin (inline L4 config).

use async_trait::async_trait;
use smallvec::SmallVec;

use crate::plugins::runtime::edgion_config_data::{
    capture_global_edgion_config_data_snapshot_for_refs, resolve_ip_list_ref_keys,
};
use crate::plugins::stream::stream_plugin_trait::{
    access_decision_to_stream_result, StreamContext, StreamPlugin, StreamPluginResult,
};
use edgion_algo::matcher::radix_ip::IpRadixMatcher;
use edgion_resources::resources::edgion_stream_plugins::ConnectionIpRestrictionRules;

const PLUGIN_NAME: &str = "StreamIpRestriction";

/// Initialize a `ConnectionIpRestrictionRules` with fail-closed semantics.
/// Returns `(rules, init_failed)`.
pub(crate) fn init_ip_restriction(
    rules: &ConnectionIpRestrictionRules,
    label: &str,
) -> (ConnectionIpRestrictionRules, bool) {
    let mut rules = rules.clone();
    let init_failed = if let Err(e) = rules.validate_and_init() {
        tracing::error!(
            error = %e,
            label,
            "Failed to initialize ConnectionIpRestrictionRules; all connections will be denied",
        );
        true
    } else {
        false
    };
    // Loud signal: a skipped inline deny entry is a blacklist hole (fail-open direction).
    if let Some(deny) = rules.deny.as_ref() {
        if !edgion_resources::matcher::ip_cidr_helpers::collect_entry_errors(deny).is_empty() {
            crate::observe::metrics::record_ip_deny_entries_skipped("stream_l4");
        }
    }
    (rules, init_failed)
}

/// Stream IP Restriction plugin
pub struct StreamIpRestriction {
    rules: ConnectionIpRestrictionRules,
    init_failed: bool,
    allow_ref_keys: Vec<String>,
    deny_ref_keys: Vec<String>,
}

impl StreamIpRestriction {
    pub fn new(rules: &ConnectionIpRestrictionRules, parent_namespace: &str) -> Self {
        let (rules, init_failed) = init_ip_restriction(rules, "stream plugin");
        let allow_ref_keys = resolve_ip_list_ref_keys(rules.allow_refs.as_ref(), parent_namespace);
        let deny_ref_keys = resolve_ip_list_ref_keys(rules.deny_refs.as_ref(), parent_namespace);
        Self {
            rules,
            init_failed,
            allow_ref_keys,
            deny_ref_keys,
        }
    }
}

#[async_trait]
impl StreamPlugin for StreamIpRestriction {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        if self.init_failed {
            return StreamPluginResult::Deny("ip init fail".to_string());
        }
        let snapshot = capture_global_edgion_config_data_snapshot_for_refs(&self.deny_ref_keys, &self.allow_ref_keys);
        let mut deny_refs: SmallVec<[&IpRadixMatcher; 4]> = SmallVec::new();
        let mut allow_refs: SmallVec<[&IpRadixMatcher; 4]> = SmallVec::new();
        if let Some(snapshot) = snapshot.as_ref() {
            snapshot.append_ip_list_matchers(&self.deny_ref_keys, &mut deny_refs);
            snapshot.append_ip_list_matchers(&self.allow_ref_keys, &mut allow_refs);
        }
        let decision = self
            .rules
            .check_ip_with_refs(&ctx.direct_peer_ip, &deny_refs, &allow_refs);
        access_decision_to_stream_result(decision, || {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    use crate::plugins::stream::stream_plugin_trait::{StreamContext, StreamLayer};
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigData, EdgionConfigDataRef, EdgionConfigDataSpec, IpGroup, IpListData,
    };
    use std::sync::Arc;

    fn seed_iplist(ns: &str, name: &str, cidr: &str) {
        let spec = EdgionConfigDataSpec {
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::IpList(IpListData {
                items: vec![IpGroup {
                    name: "g".into(),
                    description: None,
                    cidrs: vec![cidr.into()],
                }],
            }),
        };
        let mut ed = EdgionConfigData::new(name, spec);
        ed.metadata.namespace = Some(ns.to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(format!("{ns}/{name}"), Arc::new(ed));
    }

    #[tokio::test]
    async fn deny_refs_block_referenced_ip() {
        let ns = "l4-ipr-denyref-unit";
        seed_iplist(ns, "blk", "9.9.9.9/32");
        let mut rules = ConnectionIpRestrictionRules::default();
        rules.deny_refs = Some(vec![EdgionConfigDataRef {
            name: "blk".into(),
            namespace: Some(ns.into()),
            ..Default::default()
        }]);
        let plugin = StreamIpRestriction::new(&rules, ns);
        let ctx_block = StreamContext::new("9.9.9.9".parse().unwrap(), 8080, StreamLayer::Stage1);
        assert!(matches!(
            plugin.on_connection(&ctx_block).await,
            StreamPluginResult::Deny(_)
        ));
        let ctx_ok = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
        assert!(matches!(plugin.on_connection(&ctx_ok).await, StreamPluginResult::Allow));
    }

    #[tokio::test]
    async fn l4_denied_allow_ref_contributes_no_key() {
        use edgion_resources::resources::edgion_config_data::EdgionConfigDataRef;
        let ns = "l4-ipr-ref-denied-unit";
        let mut rules = ConnectionIpRestrictionRules::default();
        rules.allow_refs = Some(vec![EdgionConfigDataRef {
            name: "corp".into(),
            namespace: Some(ns.into()),
            ref_denied: Some(edgion_resources::resources::common::RefDenied {
                target_namespace: ns.into(),
                target_name: "corp".into(),
                reason: None,
            }),
        }]);
        let plugin = StreamIpRestriction::new(&rules, ns);
        // Denied allow_ref → allow_ref_keys is empty → no matcher resolved.
        assert!(plugin.allow_ref_keys.is_empty(), "denied allow_ref must yield no key");
        // Evaluate path: whitelist mode + zero allow matchers → deny-all (fail-closed).
        // Empty key lists keep the snapshot absent, so on_connection reaches
        // check_ip_with_refs with empty ref slices — a whitelist miss → Deny.
        let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
        assert!(
            matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)),
            "denied allow_ref → empty allow matchers → non-matching IP must be DENIED (deny-all fail-closed)",
        );
    }
}
