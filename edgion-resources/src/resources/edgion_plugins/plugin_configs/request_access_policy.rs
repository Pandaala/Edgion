//! L7 request access policy configuration.
//!
//! A profile evaluates the immutable direct peer and the RealIp-resolved remote
//! client independently, then combines those results with `any` or `all`.
//! Within one source, IP-first and URL-first rules are OR-ed. A primary match
//! may optionally require one `anyOf` group, where every group's `allOf`
//! conditions must all match.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::hmac_auth::HmacAuthConfig;
use super::ip_restriction::DefaultAction;
use crate::common::KeyGet;
use crate::matcher::ip_cidr_helpers::{build_grouped_matcher, validate_groups_structure};
use crate::matcher::ip_radix_tree::IpRadixMatcher;
use crate::path_normalization::{get_path_normalization_config, normalize_config_path};
use crate::resources::common::SecretObjectReference;
use crate::resources::edgion_config_data::{EdgionConfigDataRef, IpGroup};
use crate::resources::edgion_plugins::conditions::{KeyExistCondition, KeyMatchCondition};
use crate::resources::edgion_plugins::PluginValidation;

fn default_status() -> u16 {
    403
}

fn default_deny() -> DefaultAction {
    DefaultAction::Deny
}

/// L7 access policy with named, hot-selectable profiles.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestAccessPolicyConfig {
    /// Profile used when no selector is configured or selected.
    pub default_profile: String,

    /// Independently configured policy profiles.
    pub profiles: HashMap<String, RequestAccessProfile>,

    /// Optional Selector ConfigData whose value names the active profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_ref: Option<EdgionConfigDataRef>,

    /// HTTP status returned when access is denied.
    #[serde(default = "default_status")]
    #[schemars(range(min = 100, max = 599))]
    pub status: u16,

    /// Optional denial body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One complete access policy.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestAccessProfile {
    /// Result when no configured rule matches. Defaults to deny.
    #[serde(default = "default_deny")]
    pub default_action: DefaultAction,

    pub source_match: IpSourceMatch,
}

/// How DirectPeerIp and RemoteIp branches are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum SourceOperator {
    #[default]
    Any,
    All,
}

/// Rules for the two canonical L7 IP identities.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IpSourceMatch {
    #[serde(default)]
    pub operator: SourceOperator,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_peer_ip: Option<SourceRules>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_ip: Option<SourceRules>,
}

/// IP-first and URL-first rules for one IP identity.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRules {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ip_rules: Vec<IpFirstRule>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub url_rules: Vec<UrlFirstRule>,
}

/// Inline and referenced IP ranges, evaluated as one union.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessIpSet {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inline: Vec<IpGroup>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<EdgionConfigDataRef>,

    #[serde(skip)]
    #[schemars(skip)]
    pub inline_matcher: Option<IpRadixMatcher>,
}

/// IP-primary rule. `anyOf` omitted means the IP match is sufficient.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IpFirstRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub ips: AccessIpSet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<AccessConditionGroup>>,
}

/// URL-primary rule. `anyOf` omitted means the URL match is sufficient.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UrlFirstRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub urls: Vec<AccessUrlMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<AccessConditionGroup>>,
}

/// Exact host plus one or more path matches.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessUrlMatch {
    pub hosts: Vec<String>,
    pub paths: Vec<AccessPathMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

/// One OR branch. All conditions in `allOf` must match.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessConditionGroup {
    pub all_of: Vec<AccessCondition>,
}

/// Extensible secondary match conditions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AccessCondition {
    KeyExist(KeyExistCondition),
    KeyMatch(KeyMatchCondition),
    SecretMatch(SecretMatchCondition),
    IpMatch(AccessIpSet),
    HmacAuth(HmacAuthConfig),
}

/// Exact match against controller-resolved Secret values.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretMatchCondition {
    pub key: KeyGet,
    pub secret_ref: SecretObjectReference,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_values: Option<Vec<String>>,
}

impl RequestAccessPolicyConfig {
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        self.validate_base()?;
        for (name, profile) in &mut self.profiles {
            profile
                .validate_and_init()
                .map_err(|error| format!("profile '{name}': {error}"))?;
        }
        Ok(())
    }

    pub fn check_fields(&self) -> Option<String> {
        self.clone().validate_and_init().err()
    }

    pub fn resolve_profile(&self, selected: Option<&str>) -> Option<&RequestAccessProfile> {
        self.profiles.get(selected.unwrap_or(self.default_profile.as_str()))
    }

    pub fn validates_request_body(&self) -> bool {
        self.profiles.values().any(|profile| {
            [
                profile.source_match.direct_peer_ip.as_ref(),
                profile.source_match.remote_ip.as_ref(),
            ]
            .into_iter()
            .flatten()
            .any(|source| {
                source
                    .ip_rules
                    .iter()
                    .filter_map(|rule| rule.any_of.as_ref())
                    .chain(source.url_rules.iter().filter_map(|rule| rule.any_of.as_ref()))
                    .flatten()
                    .flat_map(|group| &group.all_of)
                    .any(|condition| {
                        matches!(condition, AccessCondition::HmacAuth(config) if config.validate_request_body)
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
        if !(100..600).contains(&self.status) {
            return Err(format!("requestAccessPolicy: invalid status code: {}", self.status));
        }
        Ok(())
    }
}

impl RequestAccessProfile {
    fn validate_and_init(&mut self) -> Result<(), String> {
        let sources = usize::from(self.source_match.direct_peer_ip.is_some())
            + usize::from(self.source_match.remote_ip.is_some());
        if sources == 0 {
            return Err("sourceMatch must configure directPeerIp or remoteIp".to_string());
        }
        if self.source_match.operator == SourceOperator::All && sources != 2 {
            return Err("sourceMatch.operator 'all' requires both directPeerIp and remoteIp".to_string());
        }
        if let Some(rules) = &mut self.source_match.direct_peer_ip {
            rules.validate_and_init("directPeerIp")?;
        }
        if let Some(rules) = &mut self.source_match.remote_ip {
            rules.validate_and_init("remoteIp")?;
        }
        Ok(())
    }
}

impl SourceRules {
    fn validate_and_init(&mut self, source: &str) -> Result<(), String> {
        if self.ip_rules.is_empty() && self.url_rules.is_empty() {
            return Err(format!("{source} must contain an ipRule or urlRule"));
        }
        for (index, rule) in self.ip_rules.iter_mut().enumerate() {
            rule.ips
                .validate_and_init()
                .map_err(|error| format!("{source}.ipRules[{index}].ips: {error}"))?;
            validate_any_of(rule.any_of.as_mut(), source, "ipRules", index)?;
        }
        for (index, rule) in self.url_rules.iter_mut().enumerate() {
            if rule.urls.is_empty() {
                return Err(format!("{source}.urlRules[{index}].urls cannot be empty"));
            }
            for url in &mut rule.urls {
                url.validate_and_init()
                    .map_err(|error| format!("{source}.urlRules[{index}]: {error}"))?;
            }
            validate_any_of(rule.any_of.as_mut(), source, "urlRules", index)?;
        }
        Ok(())
    }
}

impl AccessIpSet {
    fn validate_and_init(&mut self) -> Result<(), String> {
        if self.inline.is_empty() && self.refs.is_empty() {
            return Err("inline or refs must be configured".to_string());
        }
        if !self.inline.is_empty() {
            validate_groups_structure(&self.inline)?;
            self.inline_matcher = Some(build_grouped_matcher(&self.inline)?);
        }
        for reference in &self.refs {
            if reference.name.trim().is_empty() {
                return Err("refs[].name cannot be empty".to_string());
            }
        }
        Ok(())
    }
}

impl AccessUrlMatch {
    fn validate_and_init(&mut self) -> Result<(), String> {
        if self.hosts.is_empty() || self.paths.is_empty() {
            return Err("hosts and paths must both be non-empty".to_string());
        }
        for host in &self.hosts {
            let host = host.trim();
            if host.is_empty() || host.contains('*') || host.contains('/') || host.chars().any(char::is_control) {
                return Err(format!("host '{host}' must be an exact hostname"));
            }
        }
        for path in &mut self.paths {
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
}

fn validate_any_of(
    groups: Option<&mut Vec<AccessConditionGroup>>,
    source: &str,
    kind: &str,
    index: usize,
) -> Result<(), String> {
    let Some(groups) = groups else { return Ok(()) };
    if groups.is_empty() {
        return Err(format!("{source}.{kind}[{index}].anyOf cannot be empty"));
    }
    for (group_index, group) in groups.iter_mut().enumerate() {
        if group.all_of.is_empty() {
            return Err(format!(
                "{source}.{kind}[{index}].anyOf[{group_index}].allOf cannot be empty"
            ));
        }
        for condition in &mut group.all_of {
            validate_condition(condition)?;
        }
    }
    Ok(())
}

fn validate_condition(condition: &mut AccessCondition) -> Result<(), String> {
    fn local_key(key: &KeyGet) -> Result<(), String> {
        if matches!(key, KeyGet::Webhook { .. }) {
            Err("request access policy conditions cannot use webhook keys".to_string())
        } else {
            Ok(())
        }
    }

    match condition {
        AccessCondition::KeyExist(config) => local_key(&config.key),
        AccessCondition::KeyMatch(config) => {
            local_key(&config.key)?;
            let has_value = config.value.is_some()
                || config.values.as_ref().is_some_and(|values| !values.is_empty())
                || config.regex.as_ref().is_some_and(|patterns| !patterns.is_empty());
            if !has_value {
                return Err("keyMatch requires value, values, or regex".to_string());
            }
            config
                .compile()
                .map_err(|error| format!("invalid keyMatch regex: {error}"))
        }
        AccessCondition::SecretMatch(config) => {
            local_key(&config.key)?;
            if config.secret_ref.name.trim().is_empty() {
                return Err("secretMatch.secretRef.name cannot be empty".to_string());
            }
            Ok(())
        }
        AccessCondition::IpMatch(config) => config.validate_and_init(),
        AccessCondition::HmacAuth(config) => {
            if config.anonymous.is_some() {
                return Err("hmacAuth condition cannot allow anonymous requests".to_string());
            }
            if config.secret_groups.as_ref().is_none_or(Vec::is_empty) {
                return Err("hmacAuth condition requires at least one secret group".to_string());
            }
            if config.hide_credentials || !config.upstream_header_fields.is_empty() {
                return Err("hmacAuth condition cannot mutate request headers".to_string());
            }
            if config.auth_failure_delay_ms != 0 {
                return Err("hmacAuth condition cannot configure an authentication failure delay".to_string());
            }
            config.detect_validation_error().map_or(Ok(()), Err)
        }
    }
}

impl PluginValidation for RequestAccessPolicyConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_rules() -> SourceRules {
        SourceRules {
            ip_rules: vec![IpFirstRule {
                name: None,
                ips: AccessIpSet {
                    inline: vec![IpGroup {
                        name: "internal".to_string(),
                        description: None,
                        cidrs: vec!["10.0.0.0/8".to_string()],
                    }],
                    ..Default::default()
                },
                any_of: None,
            }],
            ..Default::default()
        }
    }

    fn config() -> RequestAccessPolicyConfig {
        RequestAccessPolicyConfig {
            default_profile: "default".to_string(),
            profiles: HashMap::from([(
                "default".to_string(),
                RequestAccessProfile {
                    default_action: DefaultAction::Deny,
                    source_match: IpSourceMatch {
                        operator: SourceOperator::Any,
                        direct_peer_ip: Some(source_rules()),
                        remote_ip: None,
                    },
                },
            )]),
            active_profile_ref: None,
            status: 403,
            message: None,
            description: None,
        }
    }

    #[test]
    fn compiles_valid_policy() {
        assert!(config().validate_and_init().is_ok());
    }

    #[test]
    fn rejects_missing_default_profile() {
        let mut value = config();
        value.default_profile = "missing".to_string();
        assert!(value.validate_and_init().is_err());
    }

    #[test]
    fn all_requires_both_sources() {
        let mut value = config();
        value.profiles.get_mut("default").unwrap().source_match.operator = SourceOperator::All;
        assert!(value.validate_and_init().is_err());
    }

    #[test]
    fn rejects_empty_any_of() {
        let mut value = config();
        value
            .profiles
            .get_mut("default")
            .unwrap()
            .source_match
            .direct_peer_ip
            .as_mut()
            .unwrap()
            .ip_rules[0]
            .any_of = Some(vec![]);
        assert!(value.validate_and_init().is_err());
    }
}
