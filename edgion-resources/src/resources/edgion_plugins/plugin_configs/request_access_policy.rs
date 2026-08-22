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

use super::hmac_auth::HmacMatchConfig;
use crate::common::KeyGet;
use crate::matcher::ip_cidr_helpers::{build_grouped_matcher, build_grouped_matcher_partial};
use crate::matcher::ip_radix_tree::IpRadixMatcher;
use crate::path_normalization::{get_path_normalization_config, normalize_config_path};
use crate::resources::common::SecretObjectReference;
use crate::resources::edgion_config_data::{EdgionConfigDataRef, IpGroup};
use crate::resources::edgion_plugins::conditions::{KeyExistCondition, KeyMatchCondition};
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
    #[schemars(range(min = 100, max = 599))]
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all_of: Vec<AccessCondition>,
}

/// Inline and referenced IP ranges evaluated as one union.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpAccessMatch {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ips: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ip_refs: Vec<EdgionConfigDataRef>,

    /// Controller-expanded union of inline IPs and every usable IpList ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_ips: Option<Vec<String>>,

    /// Receiver-local matcher compiled from `resolvedIps` or inline `ips`.
    #[serde(skip)]
    #[schemars(skip)]
    pub matcher: Option<IpRadixMatcher>,
}

/// One URL typed rule. Its inline candidate and every referenced item are OR-ed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UrlAccessRuleConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<AccessPathMatch>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all_of: Vec<AccessCondition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_refs: Vec<RequestAccessUrlAllowListRef>,

    /// Controller-expanded URL candidates, including the inline candidate when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_candidates: Option<Vec<UrlAccessCandidate>>,
}

/// One named URL candidate stored in RequestAccessUrlAllowList ConfigData.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UrlAccessCandidate {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    pub paths: Vec<AccessPathMatch>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all_of: Vec<AccessCondition>,
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

/// Extensible secondary match conditions. Conditions in one `allOf` are AND-ed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AccessCondition {
    KeyExist(KeyExistCondition),
    KeyMatch(KeyMatchCondition),
    SecretMatch(SecretMatchCondition),
    DirectPeerIpMatch(IpAccessMatch),
    RemoteIpMatch(IpAccessMatch),
    HmacAuth(HmacMatchConfig),
}

/// Exact match against Controller-resolved Secret values.
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
                    RequestAccessRuleType::DirectPeerIp(config) | RequestAccessRuleType::RemoteIp(config) => {
                        conditions_validate_request_body(&config.all_of)
                    }
                    RequestAccessRuleType::Url(config) => {
                        conditions_validate_request_body(&config.all_of)
                            || config.resolved_candidates.as_ref().is_some_and(|items| {
                                items.iter().any(|item| conditions_validate_request_body(&item.all_of))
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
        if !(100..600).contains(&self.status) {
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
        validate_conditions(&mut self.all_of)
    }
}

impl IpAccessMatch {
    fn validate_and_init(&mut self) -> Result<(), String> {
        if self.ips.is_empty() && self.ip_refs.is_empty() {
            return Err("ips or ipRefs must be configured".to_string());
        }
        for (index, reference) in self.ip_refs.iter().enumerate() {
            validate_name(&reference.name, &format!("ipRefs[{index}].name"))?;
        }
        let effective = self.resolved_ips.as_ref().unwrap_or(&self.ips);
        if effective.is_empty() {
            self.matcher = None;
            return Ok(());
        }
        let groups = [IpGroup {
            name: "request-access-policy".to_string(),
            description: None,
            cidrs: effective.clone(),
        }];
        self.matcher = Some(if self.resolved_ips.is_some() {
            // ConfigData IpList resources are partial-good: retain all valid
            // siblings from the controller-expanded union while the source
            // resource reports its own invalid entries.
            build_grouped_matcher_partial(&groups).0
        } else {
            build_grouped_matcher(&groups)?
        });
        Ok(())
    }
}

impl UrlAccessRuleConfig {
    fn validate_and_init(&mut self) -> Result<(), String> {
        let has_inline = !self.hosts.is_empty() || !self.paths.is_empty();
        if has_inline {
            validate_url_parts(&self.hosts, &mut self.paths, false)?;
            validate_conditions(&mut self.all_of)?;
        } else if !self.all_of.is_empty() {
            return Err("allOf requires an inline hosts/paths candidate".to_string());
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
        validate_conditions(&mut self.all_of)
    }

    pub fn ip_rule_count(&self) -> usize {
        self.all_of
            .iter()
            .filter(|condition| {
                matches!(
                    condition,
                    AccessCondition::DirectPeerIpMatch(_) | AccessCondition::RemoteIpMatch(_)
                )
            })
            .count()
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

fn validate_conditions(conditions: &mut [AccessCondition]) -> Result<(), String> {
    for condition in conditions {
        validate_condition(condition)?;
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
            validate_name(&config.secret_ref.name, "secretMatch.secretRef.name")
        }
        AccessCondition::DirectPeerIpMatch(config) | AccessCondition::RemoteIpMatch(config) => {
            config.validate_and_init()
        }
        AccessCondition::HmacAuth(config) => {
            if !config.has_credentials_source() {
                return Err("hmacAuth condition requires at least one secret group".to_string());
            }
            config.detect_validation_error().map_or(Ok(()), Err)
        }
    }
}

fn conditions_validate_request_body(conditions: &[AccessCondition]) -> bool {
    conditions
        .iter()
        .any(|condition| matches!(condition, AccessCondition::HmacAuth(config) if config.validate_request_body))
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
mod tests {
    use super::*;

    fn ip_rule(name: &str, cidr: &str) -> RequestAccessRule {
        RequestAccessRule {
            name: name.to_string(),
            rule: RequestAccessRuleType::RemoteIp(IpAccessRuleConfig {
                ip: IpAccessMatch {
                    ips: vec![cidr.to_string()],
                    ..Default::default()
                },
                all_of: vec![],
            }),
        }
    }

    fn config() -> RequestAccessPolicyConfig {
        RequestAccessPolicyConfig {
            default_profile: "default".to_string(),
            profiles: HashMap::from([(
                "default".to_string(),
                RequestAccessProfile {
                    required_rule_groups: vec![RequiredRuleGroup {
                        name: "client".to_string(),
                        any_of_rules: vec![ip_rule("internal", "10.0.0.0/8")],
                    }],
                },
            )]),
            active_profile_ref: None,
            status: 403,
            message: None,
            description: None,
            resolved_profiles: None,
            resolution_warnings: vec![],
        }
    }

    #[test]
    fn validates_required_group_model() {
        assert!(config().validate_and_init().is_ok());
    }

    #[test]
    fn rejects_empty_any_of_rules() {
        let mut value = config();
        value.profiles.get_mut("default").unwrap().required_rule_groups[0]
            .any_of_rules
            .clear();
        assert!(value
            .validate_and_init()
            .unwrap_err()
            .contains("anyOfRules cannot be empty"));
    }

    #[test]
    fn rejects_duplicate_rule_names_within_group() {
        let mut value = config();
        value.profiles.get_mut("default").unwrap().required_rule_groups[0]
            .any_of_rules
            .push(ip_rule("internal", "192.0.2.0/24"));
        assert!(value
            .validate_and_init()
            .unwrap_err()
            .contains("duplicate any-of rule name"));
    }

    #[test]
    fn serde_uses_required_groups_and_typed_rules() {
        let yaml = serde_yaml::to_string(&config()).unwrap();
        assert!(yaml.contains("requiredRuleGroups:"), "{yaml}");
        assert!(yaml.contains("anyOfRules:"), "{yaml}");
        assert!(yaml.contains("type: remoteIp"), "{yaml}");
        assert!(!yaml.contains("sourceMatch:"), "{yaml}");
    }

    #[test]
    fn hmac_match_condition_rejects_standalone_only_fields() {
        for field in [
            "anonymous: guest",
            "realm: edge",
            "authFailureDelayMs: 1",
            "hideCredentials: true",
            "upstreamHeaderFields: [X-Consumer]",
        ] {
            let yaml = format!("type: hmacAuth\n{field}\n");
            assert!(
                serde_yaml::from_str::<AccessCondition>(&yaml).is_err(),
                "accepted: {field}"
            );
        }
    }

    #[test]
    fn hmac_match_condition_redacts_admin_output_and_round_trips_conf_sync() {
        let condition = AccessCondition::HmacAuth(HmacMatchConfig {
            resolved_credentials: Some(HashMap::from([(
                "alice".to_string(),
                crate::resources::edgion_plugins::plugin_configs::HmacCredential {
                    secret: b"policy-conf-sync-secret".to_vec(),
                    headers: HashMap::new(),
                    group: "default".to_string(),
                },
            )])),
            ..Default::default()
        });

        let admin_json = {
            let _guard = crate::redact::AdminRedactGuard::new();
            serde_json::to_string(&condition).unwrap()
        };
        assert!(!admin_json.contains("policy-conf-sync-secret"), "{admin_json}");
        assert!(admin_json.contains("[redacted]"), "{admin_json}");

        let wire = serde_json::to_vec(&condition).unwrap();
        let decoded: AccessCondition = serde_json::from_slice(&wire).unwrap();
        let AccessCondition::HmacAuth(config) = decoded else {
            panic!("unexpected condition type")
        };
        assert_eq!(
            config
                .resolved_credentials
                .as_ref()
                .and_then(|credentials| credentials.get("alice"))
                .map(|credential| credential.secret.as_slice()),
            Some(b"policy-conf-sync-secret".as_slice())
        );
    }

    #[test]
    fn expanded_ip_union_compiles_one_matcher_for_inline_and_refs() {
        let mut value = IpAccessMatch {
            ips: vec!["10.0.0.0/8".to_string()],
            ip_refs: vec![EdgionConfigDataRef {
                name: "shared-networks".to_string(),
                ..Default::default()
            }],
            resolved_ips: Some(vec!["10.0.0.0/8".to_string(), "203.0.113.0/24".to_string()]),
            matcher: None,
        };
        value.validate_and_init().unwrap();
        let matcher = value.matcher.as_ref().expect("one compiled union matcher");
        assert!(matcher.contains(&"10.1.2.3".parse().unwrap()));
        assert!(matcher.contains(&"203.0.113.8".parse().unwrap()));
    }

    #[test]
    fn resolved_secret_values_are_redacted_in_admin_mode() {
        let condition = SecretMatchCondition {
            key: KeyGet::Header {
                name: "X-Origin-Token".to_string(),
            },
            secret_ref: SecretObjectReference {
                name: "origin-token".to_string(),
                group: None,
                kind: None,
                namespace: None,
            },
            resolved_values: Some(vec!["do-not-expose".to_string()]),
        };
        let json = {
            let _guard = crate::redact::AdminRedactGuard::new();
            serde_json::to_string(&condition).unwrap()
        };
        assert!(!json.contains("do-not-expose"), "{json}");
        assert!(json.contains("[redacted]"), "{json}");
    }
}
