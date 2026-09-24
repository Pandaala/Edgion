//! L7 request access policy configuration.
//!
//! A profile is an AND of named required rule groups. Each group is satisfied
//! by any complete typed rule in `anyOfRules`. Typed rules may match the direct
//! TCP peer, the RealIp-resolved remote client, or URL candidates. IP-list and
//! URL-list references are expanded by the Controller into `resolvedProfiles`
//! before the policy reaches the Gateway.

use std::collections::{HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::path_normalization::{get_path_normalization_config, normalize_config_path};
use crate::resources::edgion_config_data::EdgionConfigDataRef;
use crate::resources::edgion_plugins::conditions::{ConditionSet, ConditionValidationProfile, IpAccessMatch};
use crate::resources::edgion_plugins::PluginValidation;

pub const DEFAULT_MAX_REFERENCED_URL_RULES: usize = 100;
pub const DEFAULT_MAX_REFERENCED_IP_RULES: usize = 100;
pub const MAX_REFERENCED_URL_RULES: usize = 10_000;
pub const MAX_REFERENCED_IP_RULES: usize = 10_000;

fn default_status() -> u16 {
    403
}

fn default_max_url_rules() -> usize {
    DEFAULT_MAX_REFERENCED_URL_RULES
}

fn default_max_ip_rules() -> usize {
    DEFAULT_MAX_REFERENCED_IP_RULES
}

/// L7 access policy with named, hot-selectable profiles.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestAccessPolicyConfig {
    /// Profile used when no selector is configured or selected.
    pub default_profile: String,

    /// Operator-authored policy profiles.
    pub profiles: HashMap<String, RequestAccessProfile>,

    /// Optional Selector ConfigData whose value names the active profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_ref: Option<EdgionConfigDataRef>,

    /// HTTP status returned when access is denied.
    #[serde(default = "default_status")]
    #[schemars(range(min = 200, max = 599))]
    pub status: u16,

    /// Optional denial body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Controller-expanded, self-contained runtime profiles. The Gateway uses
    /// these instead of resolving IP/URL ConfigData on the request path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_profiles: Option<HashMap<String, RequestAccessProfile>>,

    /// Controller-only warning details used to build `PartiallyInvalid`.
    #[serde(skip)]
    #[schemars(skip)]
    pub resolution_warnings: Vec<String>,
}

/// One complete access policy. Every required group must match.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestAccessProfile {
    pub required_rule_groups: Vec<RequiredRuleGroup>,
}

/// A mandatory gate. Any complete child rule may satisfy this group.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequiredRuleGroup {
    pub name: String,
    pub any_of_rules: Vec<RequestAccessRule>,
}

/// One named, complete alternative inside a required group.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestAccessRule {
    pub name: String,
    #[serde(flatten)]
    pub rule: RequestAccessRuleType,
}

/// The three canonical RequestAccessPolicy rule kinds.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config", rename_all = "camelCase")]
pub enum RequestAccessRuleType {
    DirectPeerIp(IpAccessRuleConfig),
    RemoteIp(IpAccessRuleConfig),
    Url(UrlAccessRuleConfig),
}

/// Primary IP match plus secondary conditions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IpAccessRuleConfig {
    #[serde(flatten)]
    pub ip: IpAccessMatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<ConditionSet>,
}

/// One URL typed rule. Its inline candidate and every referenced item are OR-ed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UrlAccessRuleConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<AccessPathMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<ConditionSet>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_refs: Vec<RequestAccessUrlAllowListRef>,

    /// Controller-expanded URL candidates, including the inline candidate when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_candidates: Option<Vec<UrlAccessCandidate>>,
}

/// One named URL candidate stored in RequestAccessUrlAllowList ConfigData.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UrlAccessCandidate {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    pub paths: Vec<AccessPathMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<ConditionSet>,
}

/// Reference to a typed RequestAccessUrlAllowList and its consumer-side limits.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestAccessUrlAllowListRef {
    #[serde(flatten)]
    pub reference: EdgionConfigDataRef,
    #[serde(default)]
    pub validate: UrlAllowListRefValidation,
}

/// Consumer-side guardrails for dynamically supplied URL allow data.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UrlAllowListRefValidation {
    #[serde(default)]
    pub allow_no_hosts: bool,
    #[serde(default = "default_max_url_rules")]
    #[schemars(range(min = 1, max = 10000))]
    pub max_url_rules: usize,
    #[serde(default = "default_max_ip_rules")]
    #[schemars(range(min = 1, max = 10000))]
    pub max_ip_rules: usize,
}

impl Default for UrlAllowListRefValidation {
    fn default() -> Self {
        Self {
            allow_no_hosts: false,
            max_url_rules: default_max_url_rules(),
            max_ip_rules: default_max_ip_rules(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccessPathMatch {
    #[serde(rename = "type")]
    pub match_type: AccessPathMatchType,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AccessPathMatchType {
    Exact,
    PathPrefix,
    RegularExpression,
}

impl RequestAccessPolicyConfig {
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        self.validate_base()?;
        validate_profiles(&mut self.profiles)?;
        if let Some(profiles) = &mut self.resolved_profiles {
            validate_profiles(profiles).map_err(|error| format!("resolvedProfiles: {error}"))?;
        }
        Ok(())
    }

    pub fn check_fields(&self) -> Option<String> {
        self.clone().validate_and_init().err()
    }

    pub fn runtime_profiles(&self) -> &HashMap<String, RequestAccessProfile> {
        self.resolved_profiles.as_ref().unwrap_or(&self.profiles)
    }

    pub fn validates_request_body(&self) -> bool {
        self.runtime_profiles().values().any(|profile| {
            profile.required_rule_groups.iter().any(|group| {
                group.any_of_rules.iter().any(|rule| match &rule.rule {
                    RequestAccessRuleType::DirectPeerIp(config) | RequestAccessRuleType::RemoteIp(config) => config
                        .conditions
                        .as_ref()
                        .is_some_and(ConditionSet::validates_request_body),
                    RequestAccessRuleType::Url(config) => {
                        config
                            .conditions
                            .as_ref()
                            .is_some_and(ConditionSet::validates_request_body)
                            || config.resolved_candidates.as_ref().is_some_and(|items| {
                                items.iter().any(|item| {
                                    item.conditions
                                        .as_ref()
                                        .is_some_and(ConditionSet::validates_request_body)
                                })
                            })
                    }
                })
            })
        })
    }

    fn validate_base(&self) -> Result<(), String> {
        if self.profiles.is_empty() {
            return Err("requestAccessPolicy: at least one profile is required".to_string());
        }
        if !self.profiles.contains_key(&self.default_profile) {
            return Err(format!(
                "requestAccessPolicy: defaultProfile '{}' not found in profiles",
                self.default_profile
            ));
        }
        if !crate::http_status::is_valid_final_http_status(self.status) {
            return Err(format!("requestAccessPolicy: invalid status code: {}", self.status));
        }
        Ok(())
    }
}

fn validate_profiles(profiles: &mut HashMap<String, RequestAccessProfile>) -> Result<(), String> {
    for (name, profile) in profiles {
        profile
            .validate_and_init()
            .map_err(|error| format!("profile '{name}': {error}"))?;
    }
    Ok(())
}

impl RequestAccessProfile {
    fn validate_and_init(&mut self) -> Result<(), String> {
        if self.required_rule_groups.is_empty() {
            return Err("requiredRuleGroups cannot be empty".to_string());
        }
        let mut names = HashSet::new();
        for (index, group) in self.required_rule_groups.iter_mut().enumerate() {
            validate_name(&group.name, &format!("requiredRuleGroups[{index}].name"))?;
            if !names.insert(group.name.clone()) {
                return Err(format!("duplicate required rule group name '{}'", group.name));
            }
            group
                .validate_and_init()
                .map_err(|error| format!("requiredRuleGroups[{index}]: {error}"))?;
        }
        Ok(())
    }
}

impl RequiredRuleGroup {
    fn validate_and_init(&mut self) -> Result<(), String> {
        if self.any_of_rules.is_empty() {
            return Err("anyOfRules cannot be empty".to_string());
        }
        let mut names = HashSet::new();
        for (index, rule) in self.any_of_rules.iter_mut().enumerate() {
            validate_name(&rule.name, &format!("anyOfRules[{index}].name"))?;
            if !names.insert(rule.name.clone()) {
                return Err(format!("duplicate any-of rule name '{}'", rule.name));
            }
            rule.validate_and_init()
                .map_err(|error| format!("anyOfRules[{index}]: {error}"))?;
        }
        Ok(())
    }
}

impl RequestAccessRule {
    fn validate_and_init(&mut self) -> Result<(), String> {
        match &mut self.rule {
            RequestAccessRuleType::DirectPeerIp(config) | RequestAccessRuleType::RemoteIp(config) => {
                config.validate_and_init()
            }
            RequestAccessRuleType::Url(config) => config.validate_and_init(),
        }
    }
}

impl IpAccessRuleConfig {
    fn validate_and_init(&mut self) -> Result<(), String> {
        self.ip.validate_and_init()?;
        validate_optional_conditions(&mut self.conditions)
    }
}

impl UrlAccessRuleConfig {
    fn validate_and_init(&mut self) -> Result<(), String> {
        let has_inline = !self.hosts.is_empty() || !self.paths.is_empty();
        if has_inline {
            validate_url_parts(&self.hosts, &mut self.paths, false)?;
            validate_optional_conditions(&mut self.conditions)?;
        } else if self.conditions.is_some() {
            return Err("conditions requires an inline hosts/paths candidate".to_string());
        }
        if !has_inline && self.config_refs.is_empty() {
            return Err("an inline URL candidate or configRefs must be configured".to_string());
        }
        for (index, reference) in self.config_refs.iter().enumerate() {
            validate_name(&reference.reference.name, &format!("configRefs[{index}].name"))?;
            reference.validate.validate()?;
        }
        if let Some(candidates) = &mut self.resolved_candidates {
            for (index, candidate) in candidates.iter_mut().enumerate() {
                candidate
                    .validate_and_init(true)
                    .map_err(|error| format!("resolvedCandidates[{index}]: {error}"))?;
            }
        }
        Ok(())
    }
}

impl UrlAccessCandidate {
    pub fn validate_and_init(&mut self, allow_no_hosts: bool) -> Result<(), String> {
        validate_name(&self.name, "name")?;
        validate_url_parts(&self.hosts, &mut self.paths, allow_no_hosts)?;
        validate_optional_conditions(&mut self.conditions)
    }

    pub fn ip_rule_count(&self) -> usize {
        self.conditions.as_ref().map_or(0, ConditionSet::ip_match_count)
    }
}

impl UrlAllowListRefValidation {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_url_rules == 0 || self.max_url_rules > MAX_REFERENCED_URL_RULES {
            return Err(format!("maxUrlRules must be between 1 and {MAX_REFERENCED_URL_RULES}"));
        }
        if self.max_ip_rules == 0 || self.max_ip_rules > MAX_REFERENCED_IP_RULES {
            return Err(format!("maxIpRules must be between 1 and {MAX_REFERENCED_IP_RULES}"));
        }
        Ok(())
    }
}

fn validate_url_parts(hosts: &[String], paths: &mut [AccessPathMatch], allow_no_hosts: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Err("paths cannot be empty".to_string());
    }
    if hosts.is_empty() && !allow_no_hosts {
        return Err("hosts cannot be empty".to_string());
    }
    for host in hosts {
        let host = host.trim();
        if host.is_empty() || host.contains('*') || host.contains('/') || host.chars().any(char::is_control) {
            return Err(format!("host '{host}' must be an exact hostname"));
        }
    }
    for path in paths {
        match path.match_type {
            AccessPathMatchType::Exact | AccessPathMatchType::PathPrefix => {
                if !path.value.starts_with('/') || path.value.contains('?') || path.value.contains('#') {
                    return Err(format!(
                        "path '{}' must start with '/' and exclude query/fragment",
                        path.value
                    ));
                }
                path.value = normalize_config_path(&path.value, get_path_normalization_config());
            }
            AccessPathMatchType::RegularExpression => {
                regex::Regex::new(&path.value).map_err(|error| format!("invalid path regex: {error}"))?;
            }
        }
    }
    Ok(())
}

fn validate_optional_conditions(conditions: &mut Option<ConditionSet>) -> Result<(), String> {
    let Some(conditions) = conditions else {
        return Ok(());
    };
    conditions
        .compile()
        .map_err(|errors| format!("invalid condition regex: {}", errors[0]))?;
    conditions
        .validate(ConditionValidationProfile::RequestAsync)
        .map_err(|errors| errors.join("; "))
}

fn validate_name(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} cannot be empty"))
    } else {
        Ok(())
    }
}

impl PluginValidation for RequestAccessPolicyConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/request_access_policy_test.rs"]
mod tests;
