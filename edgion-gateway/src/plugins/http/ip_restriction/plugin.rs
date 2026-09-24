//! IP Restriction plugin implementation
//!
//! This plugin controls access based on client IP addresses using allow/deny lists.
//!
//! ## Access Control Rules (nginx-compatible):
//!
//! 1. **Deny has highest priority**: If IP matches deny list, access is denied immediately
//! 2. **Allow checked next**: If IP matches allow list, access is granted
//! 3. **Default action applies**: If no rules match, use the configured default action
//!
//! ## Configuration Examples:
//!
//! ### Whitelist only specific network:
//! ```yaml
//! ipRestriction:
//!   allow:
//!     - name: local-network
//!       description: Local network clients
//!       cidrs: ["192.168.1.0/24"]
//! ```
//!
//! ### Blacklist specific IPs:
//! ```yaml
//! ipRestriction:
//!   deny:
//!     - name: blocked-hosts
//!       cidrs: ["192.168.1.100", "10.0.0.50"]
//! ```
//!
//! ### Combined: Allow subnet but deny specific IP:
//! ```yaml
//! ipRestriction:
//!   allow:
//!     - name: private-network
//!       cidrs: ["10.0.0.0/8"]
//!   deny:
//!     - name: blocked-hosts
//!       cidrs: ["10.0.0.100"]
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use smallvec::SmallVec;
use std::net::IpAddr;

use crate::plugins::http::common::auth_common::send_gateway_plain_error_response;
use crate::plugins::runtime::edgion_config_data::{
    capture_global_edgion_config_data_snapshot_for_refs, resolve_ip_list_ref_keys,
};
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_algo::matcher::radix_ip::IpRadixMatcher;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::matcher::ip_cidr_helpers::evaluate_ip_access_multi;
use edgion_resources::resources::edgion_plugins::{DefaultAction, IpRestrictionConfig, IpSource};

const PLUGIN_NAME: &str = "IpRestriction";
const INIT_FAILURE_BODY: &[u8] = br#"{"message":"Access denied"}"#;
const DEFAULT_DENY_BODY: &[u8] = br#"{"message":"Your IP address is not allowed to access this resource"}"#;

/// IP Restriction plugin
pub struct IpRestriction {
    ip_source: IpSource,
    status: u16,
    default_action: DefaultAction,
    /// Frozen configured intent: an inline allow group or allow ref enables
    /// whitelist mode even if no referenced matcher currently resolves.
    allow_mode: bool,
    allow_matcher: Option<IpRadixMatcher>,
    deny_matcher: Option<IpRadixMatcher>,
    deny_body: Bytes,
    /// True when `validate_and_init` failed at construction; requests are then
    /// denied (fail-closed) instead of evaluated against uncompiled matchers.
    init_failed: bool,
    /// Resolved "namespace/name" store keys for allow/deny IpList refs (built once at
    /// construction; re-looked-up live each request for hot reload).
    allow_ref_keys: Vec<String>,
    deny_ref_keys: Vec<String>,
}

impl IpRestriction {
    /// Create a new IpRestriction plugin from configuration
    pub fn create(config: &IpRestrictionConfig, namespace: &str) -> Box<dyn RequestFilter> {
        // `allow_matcher` / `deny_matcher` are `#[serde(skip)]`, so they are `None`
        // after gRPC/YAML deserialization. Compile them here via `validate_and_init`;
        // without this the evaluator falls through to `Default(Allow)` and the ACL is
        // silently fail-open. Mirror the stream-layer fail-closed pattern on failure.
        let mut config = config.clone();
        let init_failed = if let Err(e) = config.validate_and_init() {
            tracing::error!(
                error = %e,
                label = "HTTP plugin",
                "Failed to initialize IpRestrictionConfig; all requests will be denied",
            );
            true
        } else {
            false
        };

        // Loud signal: a skipped inline deny entry is a blacklist hole (fail-open direction).
        if let Some(deny) = config.deny.as_ref() {
            if !edgion_resources::matcher::ip_cidr_helpers::collect_entry_errors(deny).is_empty() {
                crate::observe::metrics::record_ip_deny_entries_skipped("http");
            }
        }

        let allow_ref_keys = resolve_ip_list_ref_keys(config.allow_refs.as_ref(), namespace);
        let deny_ref_keys = resolve_ip_list_ref_keys(config.deny_refs.as_ref(), namespace);
        let allow_mode = config.allow_mode();
        let deny_body = match config.message.as_deref() {
            Some(message) => Bytes::from(serde_json::json!({"message": message}).to_string()),
            None => Bytes::from_static(DEFAULT_DENY_BODY),
        };

        let ip_restriction = IpRestriction {
            ip_source: config.ip_source,
            status: config.status,
            default_action: config.default_action,
            allow_mode,
            allow_matcher: config.allow_matcher.take(),
            deny_matcher: config.deny_matcher.take(),
            deny_body,
            init_failed,
            allow_ref_keys,
            deny_ref_keys,
        };

        Box::new(ip_restriction)
    }

    /// Extract client IP from session based on configured source
    fn get_source_ip(&self, session: &mut dyn PluginSession) -> Option<IpAddr> {
        let ip_str = match self.ip_source {
            IpSource::RemoteIp => session.remote_ip(),
            IpSource::DirectPeerIp => session.direct_peer_ip(),
        };

        // Return None if empty string (not yet populated)
        if ip_str.is_empty() {
            return None;
        }

        ip_str.parse::<IpAddr>().ok()
    }
}

#[async_trait]
impl RequestFilter for IpRestriction {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        use edgion_resources::resources::edgion_plugins::AccessDecision;

        // Fail-closed: if matcher compilation failed at construction, deny every
        // request rather than fall through to the (uncompiled) default action.
        if self.init_failed {
            plugin_log.push_err("ip init fail");
            if let Err(_e) = send_gateway_plain_error_response(
                session,
                403,
                "application/json",
                Bytes::from_static(INIT_FAILURE_BODY),
            )
            .await
            {
                plugin_log.push_err("write response fail");
            }
            return PluginRunningResult::ErrTerminateRequest;
        }

        let source_ip = self.get_source_ip(session);

        let should_deny = match source_ip {
            Some(ip) => {
                let res = {
                    // Keep the optional snapshot and every matcher borrow inside this
                    // lexical scope, which ends before any response-write await below.
                    // Declare the owner first so it outlives the matcher containers.
                    let snapshot =
                        capture_global_edgion_config_data_snapshot_for_refs(&self.deny_ref_keys, &self.allow_ref_keys);
                    let mut deny_matchers: SmallVec<[&IpRadixMatcher; 4]> = self.deny_matcher.iter().collect();
                    let mut allow_matchers: SmallVec<[&IpRadixMatcher; 4]> = self.allow_matcher.iter().collect();
                    if let Some(snapshot) = snapshot.as_ref() {
                        snapshot.append_ip_list_matchers(&self.deny_ref_keys, &mut deny_matchers);
                        snapshot.append_ip_list_matchers(&self.allow_ref_keys, &mut allow_matchers);
                    }
                    evaluate_ip_access_multi(
                        &deny_matchers,
                        &allow_matchers,
                        self.allow_mode,
                        &self.default_action,
                        &ip,
                    )
                };
                match res {
                    AccessDecision::AllowByGroup(g) => {
                        plugin_log.push_fmt("allow:group={}", la![g.as_ref()]);
                        false
                    }
                    AccessDecision::DenyByGroup(g) => {
                        plugin_log.push_fmt("deny:group={}", la![g.as_ref()]);
                        true
                    }
                    AccessDecision::DenyByWhitelistMiss => {
                        plugin_log.push("deny:not-in-allow");
                        true
                    }
                    AccessDecision::Default(DefaultAction::Allow) => {
                        plugin_log.push("allow:default");
                        false
                    }
                    AccessDecision::Default(DefaultAction::Deny) => {
                        plugin_log.push("deny:default");
                        true
                    }
                }
            }
            None => {
                // No client IP. Whitelist mode is decided by configured intent (inline allow
                // OR allow refs), NOT by resolved-matcher presence — so a refs-only allowlist
                // still denies here (fail-closed). Deny refs are irrelevant with no IP to test.
                if self.allow_mode {
                    plugin_log.push("deny:no-client-ip");
                    true
                } else {
                    let deny = matches!(self.default_action, DefaultAction::Deny);
                    plugin_log.push(if deny {
                        "deny:no-client-ip"
                    } else {
                        "allow:no-client-ip"
                    });
                    deny
                }
            }
        };

        if should_deny {
            // `self.status` is validated as a final 200..=599 response by IpRestrictionConfig::validate_and_init,
            // so the helper's ResponseHeader::build never fails here in practice.
            if let Err(_e) =
                send_gateway_plain_error_response(session, self.status, "application/json", self.deny_body.clone())
                    .await
            {
                plugin_log.push_err("write response fail");
            }

            return PluginRunningResult::ErrTerminateRequest;
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/ip_restriction/plugin_test.rs"]
mod tests;
