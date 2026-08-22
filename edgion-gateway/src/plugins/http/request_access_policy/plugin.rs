//! RequestAccessPolicy L7 request filter.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::IpAddr;

use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::matcher::ip_radix_tree::IpRadixMatcher;
use edgion_resources::matcher::radix_tree::{MatchKind, RadixTree, RadixTreeBuilder};
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::conditions::{KeyExistCondition, KeyMatchCondition};
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    AccessCondition, AccessPathMatchType, IpAccessMatch, RequestAccessPolicyConfig, RequestAccessProfile,
    RequestAccessRuleType, SecretMatchCondition, UrlAccessCandidate,
};
use edgion_resources::resources::edgion_plugins::BodyRequirement;
use regex::Regex;
use subtle::ConstantTimeEq;

use crate::ctx::NormalizedHostname;
use crate::plugins::http::hmac_auth::{HmacVerifier, HmacVerifyOutcome};
use crate::plugins::runtime::body_gate::body_read_failure_result;
use crate::plugins::runtime::conditions::{KeyExistConditionExt, KeyMatchConditionExt};
use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};

struct RuntimeIpSet(Option<IpRadixMatcher>);

impl RuntimeIpSet {
    fn new(config: IpAccessMatch) -> Self {
        Self(config.matcher)
    }

    fn matches(&self, ip: Option<&IpAddr>) -> bool {
        ip.is_some_and(|ip| self.0.as_ref().is_some_and(|matcher| matcher.contains(ip)))
    }
}

enum RuntimeCondition {
    KeyExist(KeyExistCondition),
    KeyMatch(KeyMatchCondition),
    SecretMatch(SecretMatchCondition),
    DirectPeerIpMatch(RuntimeIpSet),
    RemoteIpMatch(RuntimeIpSet),
    HmacAuth(HmacVerifier),
}

enum PolicyMatch {
    Matched,
    NoMatch,
    Terminal(PluginRunningResult),
}

impl RuntimeCondition {
    fn new(config: AccessCondition, body_requirement: Option<BodyRequirement>) -> Self {
        match config {
            AccessCondition::KeyExist(value) => Self::KeyExist(value),
            AccessCondition::KeyMatch(value) => Self::KeyMatch(value),
            AccessCondition::SecretMatch(value) => Self::SecretMatch(value),
            AccessCondition::DirectPeerIpMatch(value) => Self::DirectPeerIpMatch(RuntimeIpSet::new(value)),
            AccessCondition::RemoteIpMatch(value) => Self::RemoteIpMatch(RuntimeIpSet::new(value)),
            AccessCondition::HmacAuth(value) => Self::HmacAuth(HmacVerifier::new(&value, body_requirement)),
        }
    }

    async fn matches(
        &self,
        session: &mut dyn PluginSession,
        direct_peer_ip: Option<&IpAddr>,
        remote_ip: Option<&IpAddr>,
        log: &mut PluginLog,
    ) -> PolicyMatch {
        let matched = match self {
            Self::KeyExist(value) => value.evaluate_sync(session),
            Self::KeyMatch(value) => value.evaluate_sync_silent(session),
            Self::SecretMatch(value) => {
                let Some(actual) = session.key_get_local(&value.key) else {
                    return PolicyMatch::NoMatch;
                };
                value.resolved_values.as_ref().is_some_and(|expected| {
                    expected.iter().any(|candidate| {
                        candidate.len() == actual.len() && candidate.as_bytes().ct_eq(actual.as_bytes()).into()
                    })
                })
            }
            Self::DirectPeerIpMatch(value) => value.matches(direct_peer_ip),
            Self::RemoteIpMatch(value) => value.matches(remote_ip),
            Self::HmacAuth(value) => match value.verify(session).await {
                HmacVerifyOutcome::Verified { .. } => true,
                HmacVerifyOutcome::NoMatch(_) | HmacVerifyOutcome::LogicalBodyLimit(_) => false,
                HmacVerifyOutcome::CaptureFailure(error) | HmacVerifyOutcome::DigestReadFailure(error) => {
                    return PolicyMatch::Terminal(body_read_failure_result(
                        session,
                        value.body_requirement(),
                        log,
                        &error,
                    ));
                }
            },
        };
        if matched {
            PolicyMatch::Matched
        } else {
            PolicyMatch::NoMatch
        }
    }
}

async fn all_match(
    conditions: &[RuntimeCondition],
    session: &mut dyn PluginSession,
    direct_peer_ip: Option<&IpAddr>,
    remote_ip: Option<&IpAddr>,
    log: &mut PluginLog,
) -> PolicyMatch {
    for condition in conditions {
        match condition.matches(session, direct_peer_ip, remote_ip, log).await {
            PolicyMatch::Matched => {}
            other => return other,
        }
    }
    PolicyMatch::Matched
}

struct RuntimeUrlCandidate {
    hosts: HashSet<String>,
    all_of: Vec<RuntimeCondition>,
}

#[derive(Default)]
struct PathBucket {
    exact: Vec<usize>,
    prefix: Vec<usize>,
}

struct RuntimeUrlIndex {
    candidates: Vec<RuntimeUrlCandidate>,
    path_buckets: Vec<PathBucket>,
    path_tree: Option<RadixTree>,
    regex_paths: Vec<(usize, Regex)>,
}

fn canonical_radix_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with(':') && !segment.starts_with("::") {
                ":_"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

impl RuntimeUrlIndex {
    fn compile(candidates: Vec<UrlAccessCandidate>, body_requirement: Option<BodyRequirement>) -> Result<Self, String> {
        let mut runtime_candidates = Vec::with_capacity(candidates.len());
        let mut paths: BTreeMap<String, PathBucket> = BTreeMap::new();
        let mut regex_paths = Vec::new();
        for candidate in candidates {
            let candidate_index = runtime_candidates.len();
            runtime_candidates.push(RuntimeUrlCandidate {
                hosts: candidate
                    .hosts
                    .into_iter()
                    .map(|host| NormalizedHostname::new(&host).as_str().to_string())
                    .collect(),
                all_of: candidate
                    .all_of
                    .into_iter()
                    .map(|condition| RuntimeCondition::new(condition, body_requirement.clone()))
                    .collect(),
            });
            for path in candidate.paths {
                match path.match_type {
                    AccessPathMatchType::Exact => paths
                        .entry(canonical_radix_path(&path.value))
                        .or_default()
                        .exact
                        .push(candidate_index),
                    AccessPathMatchType::PathPrefix => paths
                        .entry(canonical_radix_path(&path.value))
                        .or_default()
                        .prefix
                        .push(candidate_index),
                    AccessPathMatchType::RegularExpression => regex_paths.push((
                        candidate_index,
                        Regex::new(&path.value)
                            .map_err(|error| format!("cannot compile path regex '{}': {error}", path.value))?,
                    )),
                }
            }
        }

        let mut builder = RadixTreeBuilder::new();
        let mut path_buckets = Vec::with_capacity(paths.len());
        for (path, bucket) in paths {
            let bucket_index = path_buckets.len();
            builder
                .insert(&path, bucket_index)
                .map_err(|error| format!("cannot compile path '{path}': {error}"))?;
            path_buckets.push(bucket);
        }
        let path_tree = (!path_buckets.is_empty())
            .then(|| {
                builder
                    .freeze()
                    .map_err(|error| format!("cannot freeze path matcher: {error}"))
            })
            .transpose()?;
        Ok(Self {
            candidates: runtime_candidates,
            path_buckets,
            path_tree,
            regex_paths,
        })
    }

    fn lookup(&self, path: &str) -> Vec<bool> {
        let mut matched = vec![false; self.candidates.len()];
        if let Some(tree) = &self.path_tree {
            for (bucket_index, kind) in tree.match_all_ext(path) {
                let bucket = &self.path_buckets[bucket_index as usize];
                if kind == MatchKind::FullyConsumed {
                    for candidate_index in &bucket.exact {
                        matched[*candidate_index] = true;
                    }
                }
                if matches!(kind, MatchKind::FullyConsumed | MatchKind::SegmentBoundary) {
                    for candidate_index in &bucket.prefix {
                        matched[*candidate_index] = true;
                    }
                }
            }
        }
        for (candidate_index, regex) in &self.regex_paths {
            if regex.is_match(path) {
                matched[*candidate_index] = true;
            }
        }
        matched
    }
}

enum RuntimeRule {
    DirectPeerIp {
        ips: RuntimeIpSet,
        all_of: Vec<RuntimeCondition>,
    },
    RemoteIp {
        ips: RuntimeIpSet,
        all_of: Vec<RuntimeCondition>,
    },
    Url(Vec<usize>),
}

impl RuntimeRule {
    async fn matches(
        &self,
        session: &mut dyn PluginSession,
        direct_peer_ip: Option<&IpAddr>,
        remote_ip: Option<&IpAddr>,
        matched_urls: &[bool],
        url_index: &RuntimeUrlIndex,
        host: &NormalizedHostname,
        log: &mut PluginLog,
    ) -> PolicyMatch {
        match self {
            Self::DirectPeerIp { ips, all_of } => {
                if !ips.matches(direct_peer_ip) {
                    PolicyMatch::NoMatch
                } else {
                    all_match(all_of, session, direct_peer_ip, remote_ip, log).await
                }
            }
            Self::RemoteIp { ips, all_of } => {
                if !ips.matches(remote_ip) {
                    PolicyMatch::NoMatch
                } else {
                    all_match(all_of, session, direct_peer_ip, remote_ip, log).await
                }
            }
            Self::Url(indices) => {
                for index in indices {
                    if !matched_urls.get(*index).copied().unwrap_or(false) {
                        continue;
                    }
                    let candidate = &url_index.candidates[*index];
                    if !candidate.hosts.is_empty() && !candidate.hosts.contains(host.as_str()) {
                        continue;
                    }
                    match all_match(&candidate.all_of, session, direct_peer_ip, remote_ip, log).await {
                        PolicyMatch::Matched => return PolicyMatch::Matched,
                        PolicyMatch::NoMatch => {}
                        terminal @ PolicyMatch::Terminal(_) => return terminal,
                    }
                }
                PolicyMatch::NoMatch
            }
        }
    }
}

struct RuntimeRuleGroup(Vec<RuntimeRule>);

struct RuntimeProfile {
    groups: Vec<RuntimeRuleGroup>,
    url_index: RuntimeUrlIndex,
}

impl RuntimeProfile {
    fn new(config: RequestAccessProfile, body_requirement: Option<BodyRequirement>) -> Result<Self, String> {
        let mut url_candidates = Vec::new();
        let mut groups = Vec::with_capacity(config.required_rule_groups.len());
        for group in config.required_rule_groups {
            let mut rules = Vec::with_capacity(group.any_of_rules.len());
            for rule in group.any_of_rules {
                rules.push(match rule.rule {
                    RequestAccessRuleType::DirectPeerIp(config) => RuntimeRule::DirectPeerIp {
                        ips: RuntimeIpSet::new(config.ip),
                        all_of: config
                            .all_of
                            .into_iter()
                            .map(|condition| RuntimeCondition::new(condition, body_requirement.clone()))
                            .collect(),
                    },
                    RequestAccessRuleType::RemoteIp(config) => RuntimeRule::RemoteIp {
                        ips: RuntimeIpSet::new(config.ip),
                        all_of: config
                            .all_of
                            .into_iter()
                            .map(|condition| RuntimeCondition::new(condition, body_requirement.clone()))
                            .collect(),
                    },
                    RequestAccessRuleType::Url(config) => {
                        let candidates = config.resolved_candidates.unwrap_or_else(|| {
                            vec![UrlAccessCandidate {
                                name: rule.name,
                                hosts: config.hosts,
                                paths: config.paths,
                                all_of: config.all_of,
                            }]
                        });
                        let start = url_candidates.len();
                        url_candidates.extend(candidates);
                        RuntimeRule::Url((start..url_candidates.len()).collect())
                    }
                });
            }
            groups.push(RuntimeRuleGroup(rules));
        }
        Ok(Self {
            groups,
            url_index: RuntimeUrlIndex::compile(url_candidates, body_requirement)?,
        })
    }

    async fn allows(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PolicyMatch {
        let direct_peer_ip = session.direct_peer_ip().parse::<IpAddr>().ok();
        let remote_ip = session.remote_ip().parse::<IpAddr>().ok();
        // One profile-wide path index is queried before host or secondary-rule checks.
        let matched_urls = if self.url_index.candidates.is_empty() {
            Vec::new()
        } else {
            self.url_index.lookup(session.get_path())
        };
        let host = if self.url_index.candidates.is_empty() {
            NormalizedHostname::new("")
        } else {
            NormalizedHostname::new(session.header_value("host").as_deref().unwrap_or_default())
        };
        for group in &self.groups {
            let mut group_matched = false;
            for rule in &group.0 {
                match rule
                    .matches(
                        session,
                        direct_peer_ip.as_ref(),
                        remote_ip.as_ref(),
                        &matched_urls,
                        &self.url_index,
                        &host,
                        log,
                    )
                    .await
                {
                    PolicyMatch::Matched => {
                        group_matched = true;
                        break;
                    }
                    PolicyMatch::NoMatch => {}
                    terminal @ PolicyMatch::Terminal(_) => return terminal,
                }
            }
            if !group_matched {
                return PolicyMatch::NoMatch;
            }
        }
        PolicyMatch::Matched
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
    pub fn create(
        config: &RequestAccessPolicyConfig,
        namespace: &str,
        body_requirement: Option<BodyRequirement>,
    ) -> Box<dyn RequestFilter> {
        let mut config = config.clone();
        let mut init_failed = config.validate_and_init().is_err()
            || config
                .active_profile_ref
                .as_ref()
                .is_some_and(|reference| reference.ref_denied.is_some());
        let mut profiles = HashMap::new();
        if !init_failed {
            for (name, profile) in config.runtime_profiles().clone() {
                match RuntimeProfile::new(profile, body_requirement.clone()) {
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
                Some(value) if value.spec.enable => match &value.spec.data {
                    ConfigEntry::Selector(selector) => {
                        if let Some(active) = selector.active.as_deref() {
                            selected = active.to_string();
                        }
                    }
                    _ => {
                        log.push_err("selector type mismatch");
                        return None;
                    }
                },
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
        match profile.allows(session, log).await {
            PolicyMatch::Matched => {
                log.push("allow:matched");
                PluginRunningResult::GoodNext
            }
            PolicyMatch::NoMatch => {
                log.push("deny:no-match");
                self.deny()
            }
            PolicyMatch::Terminal(result) => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use edgion_resources::EdgionStatus;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    const HMAC_SECRET: &[u8] = b"alice-super-secret-key";

    fn parse_config(yaml: &str) -> RequestAccessPolicyConfig {
        serde_yaml::from_str(yaml).expect("valid policy fixture")
    }

    fn hmac_fallback_config(validate_request_body: bool) -> RequestAccessPolicyConfig {
        let mut config = parse_config(&format!(
            r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: authenticated-or-internal
        anyOfRules:
          - name: signed
            type: directPeerIp
            config:
              ips: [192.0.2.0/24]
              allOf:
                - type: hmacAuth
                  secretGroups:
                    - name: default
                      secretRefs: [{{ name: hmac-credentials }}]
                  validateRequestBody: {validate_request_body}
          - name: internal-fallback
            type: directPeerIp
            config: {{ ips: [192.0.2.0/24] }}
"#
        ));
        let rule = &mut config.profiles.get_mut("lockdown").unwrap().required_rule_groups[0].any_of_rules[0];
        let RequestAccessRuleType::DirectPeerIp(rule) = &mut rule.rule else {
            panic!("unexpected rule type")
        };
        let AccessCondition::HmacAuth(hmac) = &mut rule.all_of[0] else {
            panic!("unexpected condition type")
        };
        hmac.resolved_credentials = Some(HashMap::from([(
            "alice".to_string(),
            edgion_resources::resources::edgion_plugins::HmacCredential {
                secret: HMAC_SECRET.to_vec(),
                headers: HashMap::new(),
                group: "default".to_string(),
            },
        )]));
        config
    }

    fn signed_authorization(date: &str, digest: Option<&str>) -> String {
        let (headers, signing_string) = match digest {
            Some(digest) => ("date digest", format!("date: {date}\ndigest: {digest}")),
            None => ("date", format!("date: {date}")),
        };
        let mut mac = Hmac::<Sha256>::new_from_slice(HMAC_SECRET).unwrap();
        mac.update(signing_string.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        format!(r#"hmac username="alice", algorithm="hmac-sha256", headers="{headers}", signature="{signature}""#)
    }

    #[tokio::test]
    async fn required_groups_are_and_and_inner_rules_are_or() {
        let config = parse_config(
            r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: peer
        anyOfRules:
          - name: internal-peer
            type: directPeerIp
            config: { ips: [192.0.2.0/24] }
      - name: client
        anyOfRules:
          - name: internal-client
            type: remoteIp
            config: { ips: [10.0.0.0/8] }
          - name: vendor
            type: url
            config:
              hosts: [callbacks.example.com]
              paths: [{ type: PathPrefix, value: /callbacks }]
              allOf:
                - type: remoteIpMatch
                  ips: [203.0.113.0/24]
"#,
        );
        let plugin = RequestAccessPolicy::create(&config, "default", None);
        let mut session = MockPluginSession::new();
        session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
        session.expect_remote_ip().return_const("203.0.113.8".to_string());
        session.expect_get_path().return_const("/callbacks/acme".to_string());
        session.expect_header_value().returning(|name| {
            name.eq_ignore_ascii_case("host")
                .then(|| "Callbacks.Example.Com:443".to_string())
        });
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn one_failed_required_group_denies() {
        let config = parse_config(
            r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: peer
        anyOfRules:
          - name: internal-peer
            type: directPeerIp
            config: { ips: [192.0.2.0/24] }
      - name: client
        anyOfRules:
          - name: internal-client
            type: remoteIp
            config: { ips: [10.0.0.0/8] }
"#,
        );
        let plugin = RequestAccessPolicy::create(&config, "default", None);
        let mut session = MockPluginSession::new();
        session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
        session.expect_remote_ip().return_const("203.0.113.8".to_string());
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert!(matches!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse { status: 403, .. }
        ));
    }

    #[tokio::test]
    async fn parameterized_prefix_is_matched_by_the_profile_radix_tree() {
        let config = parse_config(
            r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: callback
        anyOfRules:
          - name: vendor
            type: url
            config:
              hosts: [callbacks.example.com]
              paths: [{ type: PathPrefix, value: /callbacks/:vendor }]
"#,
        );
        let plugin = RequestAccessPolicy::create(&config, "default", None);
        let mut session = MockPluginSession::new();
        session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
        session.expect_remote_ip().return_const("203.0.113.8".to_string());
        session
            .expect_get_path()
            .return_const("/callbacks/acme/events".to_string());
        session
            .expect_header_value()
            .return_const(Some("callbacks.example.com".to_string()));
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn bad_hmac_is_a_non_match_and_allows_sibling_fallback() {
        let config = hmac_fallback_config(false);
        let plugin = RequestAccessPolicy::create(&config, "default", None);
        let date = chrono::Utc::now().to_rfc2822();
        let mut session = MockPluginSession::new();
        session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
        session.expect_remote_ip().return_const("198.51.100.8".to_string());
        session.expect_header_value().returning(move |name| match name {
            "authorization" => {
                Some(r#"hmac username="alice", algorithm="hmac-sha256", headers="date", signature="AAAA""#.to_string())
            }
            "date" => Some(date.clone()),
            _ => None,
        });
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn hmac_body_capture_failure_is_terminal_for_the_policy() {
        let config = hmac_fallback_config(true);
        let plugin = RequestAccessPolicy::create(&config, "default", None);
        let date = chrono::Utc::now().to_rfc2822();
        let digest = "SHA-256=unused".to_string();
        let authorization = signed_authorization(&date, Some(&digest));
        let mut session = MockPluginSession::new();
        session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
        session.expect_remote_ip().return_const("198.51.100.8".to_string());
        session.expect_header_value().returning(move |name| match name {
            "authorization" => Some(authorization.clone()),
            "date" => Some(date.clone()),
            "digest" => Some(digest.clone()),
            _ => None,
        });
        session
            .expect_ensure_request_body()
            .returning(|_| Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
        session.expect_add_error().returning(|_| ());
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 413,
                body: None
            }
        );
    }

    #[tokio::test]
    async fn hmac_logical_body_limit_is_a_non_match_and_allows_sibling_fallback() {
        let config = hmac_fallback_config(true);
        let requirement = BodyRequirement {
            max_body_size: Some("4".to_string()),
            ..BodyRequirement::fail_close()
        };
        let plugin = RequestAccessPolicy::create(&config, "default", Some(requirement));
        let date = chrono::Utc::now().to_rfc2822();
        let digest = "SHA-256=unused".to_string();
        let authorization = signed_authorization(&date, Some(&digest));
        let mut session = MockPluginSession::new();
        session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
        session.expect_remote_ip().return_const("198.51.100.8".to_string());
        session.expect_header_value().returning(move |name| match name {
            "authorization" => Some(authorization.clone()),
            "date" => Some(date.clone()),
            "digest" => Some(digest.clone()),
            _ => None,
        });
        session.expect_ensure_request_body().returning(|_| Ok(5));
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn hmac_digest_read_failure_is_terminal_for_the_policy() {
        let config = hmac_fallback_config(true);
        let plugin = RequestAccessPolicy::create(&config, "default", None);
        let date = chrono::Utc::now().to_rfc2822();
        let digest = "SHA-256=unused".to_string();
        let authorization = signed_authorization(&date, Some(&digest));
        let mut session = MockPluginSession::new();
        session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
        session.expect_remote_ip().return_const("198.51.100.8".to_string());
        session.expect_header_value().returning(move |name| match name {
            "authorization" => Some(authorization.clone()),
            "date" => Some(date.clone()),
            "digest" => Some(digest.clone()),
            _ => None,
        });
        session.expect_ensure_request_body().returning(|_| Ok(1));
        session
            .expect_request_body_sha256()
            .returning(|| Err(BodyReadError::Timeout));
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::BodyReadTimeout)
            .times(1)
            .returning(|_| ());
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 408,
                body: None
            }
        );
    }
}
