//! Stream IP Restriction plugin (inline L4 config).

use async_trait::async_trait;
use std::net::IpAddr;

use crate::core::gateway::plugins::stream::stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
use crate::types::resources::edgion_stream_plugins::ConnectionIpRestrictionRules;

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
    (rules, init_failed)
}

/// Check IP access with fail-closed guard.
pub(crate) fn check_ip_access(
    rules: &ConnectionIpRestrictionRules,
    init_failed: bool,
    ip: &IpAddr,
) -> StreamPluginResult {
    if init_failed {
        return StreamPluginResult::Deny("ip init fail".to_string());
    }
    // Priority: deny list > allow list > default action (mirrors ConnectionIpRestrictionRules::check_ip).
    if let Some(deny) = &rules.deny_matcher {
        if deny.match_ip(ip).is_some() {
            return StreamPluginResult::Deny("ip match:deny".to_string());
        }
    }
    if let Some(allow) = &rules.allow_matcher {
        return if allow.match_ip(ip).is_some() {
            StreamPluginResult::Allow // ip match:allow
        } else {
            StreamPluginResult::Deny("ip deny".to_string())
        };
    }
    // No allow list: default action decides.
    if matches!(rules.default_action, crate::types::resources::edgion_plugins::DefaultAction::Allow) {
        StreamPluginResult::Allow // ip allow
    } else {
        StreamPluginResult::Deny("ip deny".to_string())
    }
}

/// Stream IP Restriction plugin
pub struct StreamIpRestriction {
    rules: ConnectionIpRestrictionRules,
    init_failed: bool,
}

impl StreamIpRestriction {
    pub fn new(rules: &ConnectionIpRestrictionRules) -> Self {
        let (rules, init_failed) = init_ip_restriction(rules, "stream plugin");
        Self { rules, init_failed }
    }
}

#[async_trait]
impl StreamPlugin for StreamIpRestriction {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        check_ip_access(&self.rules, self.init_failed, &ctx.client_ip)
    }
}
