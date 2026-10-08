//! Stream IP Restriction plugin (inline L4 config).

use async_trait::async_trait;
use edgion_resources::refs::ip_list_fields::ConnectionIpListEntry;
use edgion_resources::refs::ReferenceError;
use smallvec::SmallVec;

use crate::plugins::runtime::edgion_config_data::{
    capture_global_edgion_config_data_snapshot_for_refs, resolve_bound_ip_list_keys,
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
    pub(crate) fn from_bound(entry: ConnectionIpListEntry<'_>) -> Result<Self, ReferenceError> {
        let (allow_ref_keys, deny_ref_keys) = resolve_bound_ip_list_keys(entry.references())?;
        let rules = entry.config();
        let (rules, init_failed) = init_ip_restriction(rules, "stream plugin");
        Ok(Self {
            rules,
            init_failed,
            allow_ref_keys,
            deny_ref_keys,
        })
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
#[path = "../../../../tests/unit/plugins/stream/ip_restriction/stream_ip_restriction_test.rs"]
mod tests;
