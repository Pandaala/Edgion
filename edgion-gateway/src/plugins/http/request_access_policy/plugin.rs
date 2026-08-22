//! RequestAccessPolicy L7 request filter.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;

use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::matcher::ip_radix_tree::IpRadixMatcher;
use edgion_resources::matcher::radix_tree::{MatchKind, RadixTree, RadixTreeBuilder};
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::conditions::{KeyExistCondition, KeyMatchCondition};
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    AccessCondition, AccessConditionGroup, AccessIpSet, AccessPathMatchType, IpFirstRule, RequestAccessPolicyConfig,
    RequestAccessProfile, SecretMatchCondition, SourceOperator, SourceRules,
};
use edgion_resources::resources::edgion_plugins::DefaultAction;
use regex::Regex;
use subtle::ConstantTimeEq;

use crate::ctx::NormalizedHostname;
use crate::plugins::http::hmac_auth::HmacAuth;
use crate::plugins::runtime::conditions::{KeyExistConditionExt, KeyMatchConditionExt};
use crate::plugins::runtime::edgion_config_data::{
    get_global_edgion_config_data_store, resolve_ip_list_matchers, resolve_ip_list_ref_keys,
};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};

struct RuntimeIpSet {
    inline: Option<IpRadixMatcher>,
    ref_keys: Vec<String>,
}

impl RuntimeIpSet {
    fn new(config: AccessIpSet, namespace: &str) -> Self {
        Self {
            inline: config.inline_matcher,
            ref_keys: resolve_ip_list_ref_keys(Some(&config.refs), namespace),
        }
    }

    fn matches(&self, ip: &IpAddr) -> bool {
        self.inline.as_ref().is_some_and(|matcher| matcher.contains(ip))
            || resolve_ip_list_matchers(&self.ref_keys)
                .iter()
                .any(|matcher| matcher.contains(ip))
    }
}

enum RuntimeCondition {
    KeyExist(KeyExistCondition),
    KeyMatch(KeyMatchCondition),
    SecretMatch(SecretMatchCondition),
    IpMatch(RuntimeIpSet),
    HmacAuth(HmacAuth),
}

type RuntimeAnyOf = Option<Vec<Vec<RuntimeCondition>>>;

struct RuntimeIpRule {
    name: Option<String>,
    ips: RuntimeIpSet,
    any_of: RuntimeAnyOf,
}

struct RuntimeUrlRule {
    name: Option<String>,
    any_of: RuntimeAnyOf,
}

struct RuntimeUrlEntry {
    rule_index: usize,
    hosts: HashSet<String>,
}

struct RuntimeSource {
    ip_rules: Vec<RuntimeIpRule>,
    url_rules: Vec<RuntimeUrlRule>,
    url_entries: Vec<RuntimeUrlEntry>,
    path_tree: Option<RadixTree>,
    regex_paths: Vec<(usize, Regex)>,
}

struct RuntimeProfile {
    default_action: DefaultAction,
    operator: SourceOperator,
    direct_peer_ip: Option<RuntimeSource>,
    remote_ip: Option<RuntimeSource>,
}

impl RuntimeCondition {
    fn new(config: AccessCondition, namespace: &str) -> Self {
        match config {
            AccessCondition::KeyExist(value) => Self::KeyExist(value),
            AccessCondition::KeyMatch(value) => Self::KeyMatch(value),
            AccessCondition::SecretMatch(value) => Self::SecretMatch(value),
            AccessCondition::IpMatch(value) => Self::IpMatch(RuntimeIpSet::new(value, namespace)),
            AccessCondition::HmacAuth(value) => Self::HmacAuth(HmacAuth::new(&value)),
        }
    }

    async fn matches(&self, session: &mut dyn PluginSession, ip: &IpAddr) -> bool {
        match self {
            Self::KeyExist(value) => value.evaluate_sync(session),
            Self::KeyMatch(value) => value.evaluate_sync_silent(session),
            Self::SecretMatch(value) => {
                let Some(actual) = session.key_get_local(&value.key) else {
                    return false;
                };
                value.resolved_values.as_ref().is_some_and(|expected| {
                    expected.iter().any(|candidate| {
                        candidate.as_bytes().len() == actual.as_bytes().len()
                            && candidate.as_bytes().ct_eq(actual.as_bytes()).into()
                    })
                })
            }
            Self::IpMatch(value) => value.matches(ip),
            Self::HmacAuth(value) => value.matches_policy(session).await,
        }
    }
}

fn compile_any_of(groups: Option<Vec<AccessConditionGroup>>, namespace: &str) -> RuntimeAnyOf {
    groups.map(|groups| {
        groups
            .into_iter()
            .map(|group| {
                group
                    .all_of
                    .into_iter()
                    .map(|condition| RuntimeCondition::new(condition, namespace))
                    .collect()
            })
            .collect()
    })
}

async fn any_of_matches(any_of: &RuntimeAnyOf, session: &mut dyn PluginSession, ip: &IpAddr) -> bool {
    let Some(groups) = any_of else { return true };
    for group in groups {
        let mut matched = true;
        for condition in group {
            if !condition.matches(session, ip).await {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

impl RuntimeSource {
    fn new(config: SourceRules, namespace: &str) -> Result<Self, String> {
        let ip_rules = config
            .ip_rules
            .into_iter()
            .map(|rule: IpFirstRule| RuntimeIpRule {
                name: rule.name,
                ips: RuntimeIpSet::new(rule.ips, namespace),
                any_of: compile_any_of(rule.any_of, namespace),
            })
            .collect();

        let mut path_builder = RadixTreeBuilder::new();
        let mut has_radix_paths = false;
        let mut regex_paths = Vec::new();
        let mut url_entries = Vec::new();
        let mut url_rules = Vec::with_capacity(config.url_rules.len());
        for (rule_index, rule) in config.url_rules.into_iter().enumerate() {
            for url in rule.urls {
                let entry_index = url_entries.len();
                url_entries.push(RuntimeUrlEntry {
                    rule_index,
                    hosts: url
                        .hosts
                        .into_iter()
                        .map(|host| NormalizedHostname::new(&host).as_str().to_string())
                        .collect(),
                });
                for path in url.paths {
                    match path.match_type {
                        AccessPathMatchType::Exact => {
                            has_radix_paths = true;
                            path_builder
                                .insert(&path.value, entry_index * 2)
                                .map_err(|error| format!("cannot compile exact path '{}': {error}", path.value))?;
                        }
                        AccessPathMatchType::PathPrefix => {
                            has_radix_paths = true;
                            path_builder
                                .insert(&path.value, entry_index * 2 + 1)
                                .map_err(|error| format!("cannot compile prefix path '{}': {error}", path.value))?;
                        }
                        AccessPathMatchType::RegularExpression => regex_paths.push((
                            entry_index,
                            Regex::new(&path.value)
                                .map_err(|error| format!("cannot compile path regex '{}': {error}", path.value))?,
                        )),
                    }
                }
            }
            url_rules.push(RuntimeUrlRule {
                name: rule.name,
                any_of: compile_any_of(rule.any_of, namespace),
            });
        }
        let path_tree = has_radix_paths
            .then(|| {
                path_builder
                    .freeze()
                    .map_err(|error| format!("cannot freeze path matcher: {error}"))
            })
            .transpose()?;
        Ok(Self {
            ip_rules,
            url_rules,
            url_entries,
            path_tree,
            regex_paths,
        })
    }

    async fn matches(&self, session: &mut dyn PluginSession, ip: Option<&IpAddr>) -> Option<String> {
        let ip = ip?;
        for rule in &self.ip_rules {
            if rule.ips.matches(ip) && any_of_matches(&rule.any_of, session, ip).await {
                return Some(rule.name.clone().unwrap_or_else(|| "ipRule".to_string()));
            }
        }

        if self.url_rules.is_empty() {
            return None;
        }

        let host = NormalizedHostname::new(session.header_value("host").as_deref().unwrap_or_default());
        let path = session.get_path();
        let mut entries = HashSet::new();
        if let Some(tree) = &self.path_tree {
            for (encoded, kind) in tree.match_all_ext(path) {
                let exact = encoded % 2 == 0;
                if (exact && kind == MatchKind::FullyConsumed)
                    || (!exact && matches!(kind, MatchKind::FullyConsumed | MatchKind::SegmentBoundary))
                {
                    entries.insert((encoded as usize) / 2);
                }
            }
        }
        for (entry_index, regex) in &self.regex_paths {
            if regex.is_match(path) {
                entries.insert(*entry_index);
            }
        }
        for entry_index in entries {
            let entry = &self.url_entries[entry_index];
            if entry.hosts.contains(host.as_str()) {
                let rule = &self.url_rules[entry.rule_index];
                if any_of_matches(&rule.any_of, session, ip).await {
                    return Some(rule.name.clone().unwrap_or_else(|| "urlRule".to_string()));
                }
            }
        }
        None
    }
}

impl RuntimeProfile {
    fn new(config: RequestAccessProfile, namespace: &str) -> Result<Self, String> {
        Ok(Self {
            default_action: config.default_action,
            operator: config.source_match.operator,
            direct_peer_ip: config
                .source_match
                .direct_peer_ip
                .map(|source| RuntimeSource::new(source, namespace))
                .transpose()?,
            remote_ip: config
                .source_match
                .remote_ip
                .map(|source| RuntimeSource::new(source, namespace))
                .transpose()?,
        })
    }

    async fn allows(&self, session: &mut dyn PluginSession) -> Option<String> {
        let direct_ip = session.client_addr().parse::<IpAddr>().ok();
        let remote_ip = session.remote_addr().parse::<IpAddr>().ok();
        match self.operator {
            SourceOperator::Any => {
                if let Some(source) = &self.direct_peer_ip {
                    if let Some(rule) = source.matches(session, direct_ip.as_ref()).await {
                        return Some(format!("directPeerIp:{rule}"));
                    }
                }
                if let Some(source) = &self.remote_ip {
                    if let Some(rule) = source.matches(session, remote_ip.as_ref()).await {
                        return Some(format!("remoteIp:{rule}"));
                    }
                }
                (self.default_action == DefaultAction::Allow).then(|| "default".to_string())
            }
            SourceOperator::All => {
                let direct = if let Some(source) = &self.direct_peer_ip {
                    source.matches(session, direct_ip.as_ref()).await
                } else {
                    None
                };
                let remote = if let Some(source) = &self.remote_ip {
                    source.matches(session, remote_ip.as_ref()).await
                } else {
                    None
                };
                match (direct, remote) {
                    (Some(direct), Some(remote)) => Some(format!("directPeerIp:{direct},remoteIp:{remote}")),
                    _ if self.default_action == DefaultAction::Allow => Some("default".to_string()),
                    _ => None,
                }
            }
        }
    }
}

pub struct RequestAccessPolicy {
    name: String,
    default_profile: String,
    profiles: HashMap<String, RuntimeProfile>,
    active_profile_ref_key: Option<String>,
    status: u16,
    message: Option<String>,
    init_failed: bool,
}

impl RequestAccessPolicy {
    pub fn create(config: &RequestAccessPolicyConfig, namespace: &str) -> Box<dyn RequestFilter> {
        let mut config = config.clone();
        let mut init_failed = config.validate_and_init().is_err();
        if config
            .active_profile_ref
            .as_ref()
            .is_some_and(|reference| reference.ref_denied.is_some())
        {
            init_failed = true;
        }
        let mut profiles = HashMap::new();
        if !init_failed {
            for (name, profile) in config.profiles.clone() {
                match RuntimeProfile::new(profile, namespace) {
                    Ok(profile) => {
                        profiles.insert(name, profile);
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "Failed to initialize RequestAccessPolicy; requests will be denied");
                        init_failed = true;
                        profiles.clear();
                        break;
                    }
                }
            }
        }
        if init_failed {
            tracing::error!("Invalid RequestAccessPolicy configuration; requests will be denied");
        }
        let active_profile_ref_key = config
            .active_profile_ref
            .as_ref()
            .filter(|reference| reference.ref_denied.is_none())
            .map(|reference| {
                format!(
                    "{}/{}",
                    reference.namespace.as_deref().unwrap_or(namespace),
                    reference.name
                )
            });
        Box::new(Self {
            name: "RequestAccessPolicy".to_string(),
            default_profile: config.default_profile,
            profiles,
            active_profile_ref_key,
            status: config.status,
            message: config.message,
            init_failed,
        })
    }

    fn active_profile<'a>(&'a self, log: &mut PluginLog) -> Option<&'a RuntimeProfile> {
        let mut selected = self.default_profile.clone();
        if let Some(key) = &self.active_profile_ref_key {
            let store = get_global_edgion_config_data_store();
            let data = store.get_raw_by_key(key);
            match data.as_deref() {
                Some(value) if value.spec.enable => {
                    if let ConfigEntry::Selector(selector) = &value.spec.data {
                        if let Some(active) = selector.active.as_deref() {
                            selected = active.to_string();
                        }
                    } else {
                        log.push_err("selector type mismatch");
                        return None;
                    }
                }
                _ => {
                    log.push_err("selector unavailable");
                    return None;
                }
            }
        }
        self.profiles.get(&selected)
    }

    fn deny(&self) -> PluginRunningResult {
        PluginRunningResult::ErrResponse {
            status: self.status,
            body: self.message.clone().or_else(|| Some("Access denied".to_string())),
        }
    }
}

#[async_trait]
impl RequestFilter for RequestAccessPolicy {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if self.init_failed {
            log.push_err("deny:init-failed");
            return self.deny();
        }
        let Some(profile) = self.active_profile(log) else {
            log.push_err("deny:profile-unavailable");
            return self.deny();
        };
        match profile.allows(session).await {
            Some(rule) => {
                log.push(&format!("allow:{rule}"));
                PluginRunningResult::GoodNext
            }
            None => {
                log.push("deny:no-match");
                self.deny()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    fn parse_config(yaml: &str) -> RequestAccessPolicyConfig {
        serde_yaml::from_str(yaml).expect("valid policy fixture")
    }

    #[tokio::test]
    async fn direct_peer_ip_and_key_match_allow() {
        let config = parse_config(
            r#"
defaultProfile: lockdown
profiles:
  lockdown:
    defaultAction: deny
    sourceMatch:
      directPeerIp:
        ipRules:
          - name: cdn
            ips:
              inline:
                - name: edge
                  cidrs: ["192.0.2.0/24"]
            anyOf:
              - allOf:
                  - type: keyMatch
                    key: { type: header, name: X-Origin-Token }
                    value: expected
"#,
        );
        let plugin = RequestAccessPolicy::create(&config, "default");
        let mut session = MockPluginSession::new();
        session.expect_client_addr().return_const("192.0.2.10".to_string());
        session.expect_remote_addr().return_const("198.51.100.10".to_string());
        session
            .expect_key_get_local()
            .returning(|_| Some("expected".to_string()));
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert!(log.contains("allow:directPeerIp:cdn"));
    }

    #[tokio::test]
    async fn url_parameter_rule_requires_exact_host_and_ip_condition() {
        let config = parse_config(
            r#"
defaultProfile: lockdown
profiles:
  lockdown:
    defaultAction: deny
    sourceMatch:
      directPeerIp:
        urlRules:
          - name: vendor
            urls:
              - hosts: ["callbacks.example.com"]
                paths:
                  - type: PathPrefix
                    value: /callbacks/:vendor
            anyOf:
              - allOf:
                  - type: ipMatch
                    inline:
                      - name: vendor
                        cidrs: ["203.0.113.0/24"]
"#,
        );
        let plugin = RequestAccessPolicy::create(&config, "default");
        let mut session = MockPluginSession::new();
        session.expect_client_addr().return_const("203.0.113.8".to_string());
        session.expect_remote_addr().return_const("198.51.100.10".to_string());
        session
            .expect_get_path()
            .return_const("/callbacks/acme/events".to_string());
        session.expect_header_value().returning(|name| {
            name.eq_ignore_ascii_case("host")
                .then(|| "Callbacks.Example.Com:443".to_string())
        });
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert!(log.contains("allow:directPeerIp:vendor"));
    }

    #[tokio::test]
    async fn all_sources_fail_closed_when_only_one_matches() {
        let config = parse_config(
            r#"
defaultProfile: lockdown
profiles:
  lockdown:
    defaultAction: deny
    sourceMatch:
      operator: all
      directPeerIp:
        ipRules:
          - ips:
              inline:
                - name: peer
                  cidrs: ["192.0.2.0/24"]
      remoteIp:
        ipRules:
          - ips:
              inline:
                - name: client
                  cidrs: ["203.0.113.0/24"]
"#,
        );
        let plugin = RequestAccessPolicy::create(&config, "default");
        let mut session = MockPluginSession::new();
        session.expect_client_addr().return_const("192.0.2.10".to_string());
        session.expect_remote_addr().return_const("198.51.100.10".to_string());
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: Some("Access denied".to_string()),
            }
        );
        assert!(log.contains("deny:no-match"));
    }
}
