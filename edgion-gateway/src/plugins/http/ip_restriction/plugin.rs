//! HTTP ACL policy evaluated from reference-only central IpLists.
//! Configure ordered allowRefs/denyRefs and author their IpGroups in named IpLists.
use crate::plugins::common::ip_acl::CompiledIpAcl;
use crate::plugins::http::common::auth_common::send_gateway_plain_error_response;
use crate::plugins::runtime::conditions::ip_refs::IpConditionCompiler;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::refs::ip_list_fields::HttpIpListEntry;
use edgion_resources::resources::edgion_plugins::{AccessDecision, DefaultAction, IpSource, PluginValidation};
use std::net::IpAddr;
use std::sync::Arc;
const PLUGIN_NAME: &str = "IpRestriction";
const DEFAULT_DENY_BODY: &[u8] = br#"{"message":"Your IP address is not allowed to access this resource"}"#;
pub struct IpRestriction {
    ip_source: IpSource,
    status: u16,
    deny_body: Bytes,
    acl: Arc<CompiledIpAcl>,
}
impl IpRestriction {
    pub(crate) fn create_bound(
        entry: HttpIpListEntry<'_>,
        compiler: &IpConditionCompiler<'_>,
    ) -> Option<Box<dyn RequestFilter>> {
        let config = entry.config();
        if config.validation_error().is_some() {
            return None;
        }
        entry.references().validate_owner().ok()?;
        let acl = compiler.bind_acl(&config.refs)?;
        let deny_body = match config.message.as_deref() {
            Some(message) => Bytes::from(serde_json::json!({"message": message}).to_string()),
            None => Bytes::from_static(DEFAULT_DENY_BODY),
        };
        Some(Box::new(Self {
            ip_source: config.ip_source.clone(),
            status: config.status,
            deny_body,
            acl,
        }))
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
        session.ensure_config_data_view();
        let ip = self.get_source_ip(session);
        // The scalar outcome ends all immutable session borrows before response-write await.
        let decision = session.ip_acl_decision(&self.acl, ip);
        let should_deny = if ip.is_none() {
            let deny = !matches!(decision, AccessDecision::Default(DefaultAction::Allow));
            plugin_log.push(if deny {
                "deny:no-client-ip"
            } else {
                "allow:no-client-ip"
            });
            deny
        } else {
            match decision {
                AccessDecision::AllowByGroup(group) => {
                    plugin_log.push_fmt("allow:group={}", la![group.as_ref()]);
                    false
                }
                AccessDecision::DenyByGroup(group) => {
                    plugin_log.push_fmt("deny:group={}", la![group.as_ref()]);
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
        };
        if should_deny {
            if send_gateway_plain_error_response(session, self.status, "application/json", self.deny_body.clone())
                .await
                .is_err()
            {
                plugin_log.push_err("write response fail");
            }
            PluginRunningResult::ErrTerminateRequest
        } else {
            PluginRunningResult::GoodNext
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/ip_restriction/plugin_test.rs"]
mod tests;
