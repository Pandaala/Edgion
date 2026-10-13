//! Actual-owner cold ACL plan over authority-filtered borrowed canonical IpList matchers.
use crate::plugins::runtime::conditions::ip_refs::{compile_keys, IpListReadKey};
use crate::plugins::runtime::edgion_config_data::EdgionConfigDataSnapshot;
use edgion_resources::matcher::ip_cidr_helpers::evaluate_ip_access_multi;
use edgion_resources::refs::condition_fields::IpExecutionRole;
use edgion_resources::refs::ip_list_fields::{BoundIpListRefs, IpListSide};
use edgion_resources::refs::ReferenceError;
use edgion_resources::resources::edgion_plugins::{AccessDecision, DefaultAction};
use std::net::IpAddr;
use std::sync::Arc;

/// Scalar session capability input. Construction and keys remain crate-private.
#[derive(Debug)]
pub struct CompiledIpAcl {
    deny: Vec<IpListReadKey>,
    allow: Vec<IpListReadKey>,
    allow_mode: bool,
    default_action: DefaultAction,
}
impl CompiledIpAcl {
    pub(crate) fn from_bound(
        refs: BoundIpListRefs<'_>,
        default_action: &DefaultAction,
    ) -> Result<Arc<Self>, ReferenceError> {
        refs.validate_owner()?;
        if refs.config().validation_error().is_some() {
            return Err(ReferenceError::UnsupportedTarget);
        }
        let mut invalid = false;
        let allow_scopes = refs.read_scopes(IpListSide::Allow)?;
        let deny_scopes = refs.read_scopes(IpListSide::Deny)?;
        let allow = compile_keys(
            allow_scopes.as_deref(),
            IpExecutionRole::Partial,
            refs.references(IpListSide::Allow),
            &mut invalid,
        )?;
        let deny = compile_keys(
            deny_scopes.as_deref(),
            IpExecutionRole::Partial,
            refs.references(IpListSide::Deny),
            &mut invalid,
        )?;
        Ok(Arc::new(Self {
            deny,
            allow,
            allow_mode: refs.config().allow_mode(),
            default_action: default_action.clone(),
        }))
    }
    pub(crate) fn evaluate_in(&self, view: Option<&EdgionConfigDataSnapshot>, ip: Option<IpAddr>) -> AccessDecision {
        let Some(ip) = ip else {
            return if self.allow_mode {
                AccessDecision::DenyByWhitelistMiss
            } else {
                AccessDecision::Default(self.default_action.clone())
            };
        };
        let deny = view.into_iter().flat_map(|view| view.ip_list_matchers(&self.deny));
        let allow = view.into_iter().flat_map(|view| view.ip_list_matchers(&self.allow));
        evaluate_ip_access_multi(deny, allow, self.allow_mode, &self.default_action, &ip)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/common/ip_acl_test.rs"]
mod tests;
