//! Request Restriction plugin implementation
//!
//! This plugin restricts access based on request attributes and identity,
//! evaluated through the shared bounded condition-set grammar.
//!
//! ## Model (DNF: OR-of-AND)
//! - `mode` is required: `Deny` (match => reject, unmatched passes) or `Allow`
//!   (match => pass, unmatched is rejected). It has no default, so a config that
//!   omits it is rejected instead of silently picking a polarity.
//! - `conditions` has exactly one root `allOf` or `anyOf`.
//! - A root `anyOf` contains direct conditions or one-level named `allOf`
//!   groups. Configuration order determines short-circuit order.
//!
//! ## Configuration Examples:
//!
//! ### Deny: block bots OR /admin paths:
//! ```yaml
//! requestRestriction:
//!   mode: Deny
//!   conditions:
//!     anyOf:
//!       - type: keyMatch
//!         key: { type: path }
//!         values: ["/admin"]
//!       - name: destructive-admin
//!         allOf:
//!           - type: keyMatch
//!             key: { type: method }
//!             values: ["DELETE"]
//!           - type: keyMatch
//!             key: { type: path }
//!             regex: ["^/admin(?:/|$)"]
//!   message: "Bot access denied"
//! ```

use async_trait::async_trait;
use bytes::Bytes;

use crate::plugins::http::common::auth_common::send_gateway_plain_error_response;
use crate::plugins::runtime::conditions::{condition_set_has_unresolved, CompiledConditionSet, MatchState};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{BodyRequirement, RequestRestrictionConfig};

const PLUGIN_NAME: &str = "RequestRestriction";
const DEFAULT_DENY_BODY: &[u8] = br#"{"message":"Access denied by restriction policy"}"#;
const INVALID_POLICY_BODY: &[u8] = br#"{"message":"Internal restriction policy error"}"#;

struct PreparedRestrictionPolicy {
    allow_mode: bool,
    status: u16,
    deny_body: Bytes,
}

/// Request Restriction plugin
pub struct RequestRestriction {
    conditions: CompiledConditionSet,
    /// `None` means validation failed or a deny policy has unresolved
    /// dependencies, so requests must take the generic fail-closed path.
    policy: Option<PreparedRestrictionPolicy>,
}

impl RequestRestriction {
    pub(crate) fn would_fail_construction(config: &RequestRestrictionConfig) -> bool {
        let mut validated = config.clone();
        validated.validate().is_err()
            || (!validated.allow_mode() && condition_set_has_unresolved(&validated.conditions))
    }

    /// Create a new RequestRestriction plugin from configuration
    pub fn create(config: &RequestRestrictionConfig) -> Box<dyn RequestFilter> {
        Self::create_with_body(config, None)
    }

    pub fn create_with_body(
        config: &RequestRestrictionConfig,
        body_requirement: Option<BodyRequirement>,
    ) -> Box<dyn RequestFilter> {
        // Clone and validate the config to compile regex patterns. The caller's
        // generation can then be released while the compiled condition set owns
        // the runtime material it needs.
        let mut validated_config = config.clone();
        let config_valid = validated_config.validate().is_ok();
        let conditions = CompiledConditionSet::new(&validated_config.conditions, "conditions", body_requirement);
        let policy = if config_valid {
            let allow_mode = validated_config.allow_mode();
            if !allow_mode && conditions.has_unresolved_refs() {
                None
            } else {
                let deny_body = match validated_config.message.as_deref() {
                    Some(message) => Bytes::from(serde_json::json!({"message": message}).to_string()),
                    None => Bytes::from_static(DEFAULT_DENY_BODY),
                };
                Some(PreparedRestrictionPolicy {
                    allow_mode,
                    status: validated_config.status,
                    deny_body,
                })
            }
        } else {
            None
        };

        let plugin = RequestRestriction { conditions, policy };

        Box::new(plugin)
    }
}

#[async_trait]
impl RequestFilter for RequestRestriction {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let Some(policy) = self.policy.as_ref() else {
            plugin_log.push_err("restriction config invalid");
            if let Err(_e) = send_gateway_plain_error_response(
                session,
                500,
                "application/json",
                Bytes::from_static(INVALID_POLICY_BODY),
            )
            .await
            {
                plugin_log.push_err("write response fail");
            }
            return PluginRunningResult::ErrTerminateRequest;
        };

        // Apply polarity only after the shared tri-state evaluator completes.
        let eval = match self
            .conditions
            .evaluate_async(session, plugin_log, self.conditions.atomic_count() > 1)
            .await
        {
            Ok(result) => result,
            Err(terminal) => return terminal,
        };
        let any_matched = eval.state == MatchState::True;
        let denied = any_matched != policy.allow_mode;

        if denied {
            if any_matched {
                plugin_log.push_fmt("Deny {}", crate::la![eval.cause.unwrap_or("conditions")]);
            } else {
                plugin_log.push("Deny (no allow alternative matched)");
            }

            // Invariant: resource validation accepts only final statuses in
            // 200..=599 before the plugin is built, so the response helper can
            // construct the configured status.
            if let Err(_e) =
                send_gateway_plain_error_response(session, policy.status, "application/json", policy.deny_body.clone())
                    .await
            {
                plugin_log.push_err("write response fail");
            }

            return PluginRunningResult::ErrTerminateRequest;
        }

        plugin_log.push("Allow");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/request_restriction/plugin_test.rs"]
mod tests;
