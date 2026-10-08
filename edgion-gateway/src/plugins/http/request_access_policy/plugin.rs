//! RequestAccessPolicy L7 request filter.

use edgion_resources::refs::config_data_fields::{request_access_has_external_data, HttpConfigDataEntry};
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::IpAddr;

use async_trait::async_trait;
use bytes::Bytes;
use edgion_algo::matcher::radix_ip::IpRadixMatcher;
use edgion_algo::matcher::radix_path::{PathMatchKind, PathRadixBuilder, PathRadixTree};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    AccessPathMatchType, IpAccessMatch, RequestAccessPolicyConfig, RequestAccessProfile, RequestAccessRuleType,
    UrlAccessCandidate,
};
use edgion_resources::resources::edgion_plugins::BodyRequirement;
use regex::Regex;

use crate::ctx::NormalizedHostname;
use crate::plugins::runtime::conditions::{CompiledConditionSet, MatchState};
use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};

/// Fallback deny body used when the operator configured no `message`.
const DEFAULT_DENY_BODY: Bytes = Bytes::from_static(b"Access denied");

struct RuntimeIpSet(Option<IpRadixMatcher>);

impl RuntimeIpSet {
    fn new(config: IpAccessMatch) -> Self {
        Self(config.matcher)
    }

    fn matches(&self, ip: Option<&IpAddr>) -> bool {
        ip.is_some_and(|ip| self.0.as_ref().is_some_and(|matcher| matcher.contains(ip)))
    }
}

enum PolicyMatch {
    Matched,
    NoMatch,
    Terminal(PluginRunningResult),
}

async fn supplemental_matches(
    conditions: Option<&CompiledConditionSet>,
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
) -> PolicyMatch {
    let Some(conditions) = conditions else {
        return PolicyMatch::Matched;
    };
    match conditions
        .evaluate_async(session, log, conditions.atomic_count() > 1)
        .await
    {
        Ok(result) if result.state == MatchState::True => PolicyMatch::Matched,
        Ok(_) => PolicyMatch::NoMatch,
        Err(result) => PolicyMatch::Terminal(result),
    }
}

struct RuntimeUrlCandidate {
    hosts: HashSet<String>,
    conditions: Option<CompiledConditionSet>,
}

#[derive(Default)]
struct PathBucket {
    exact: Vec<usize>,
    prefix: Vec<usize>,
}

struct RuntimeUrlIndex {
    candidates: Vec<RuntimeUrlCandidate>,
    path_buckets: Vec<PathBucket>,
    path_tree: Option<PathRadixTree>,
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
                conditions: candidate
                    .conditions
                    .as_ref()
                    .map(|conditions| CompiledConditionSet::new(conditions, "conditions", body_requirement.clone())),
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

        let mut builder = PathRadixBuilder::new();
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
                if kind == PathMatchKind::FullyConsumed {
                    for candidate_index in &bucket.exact {
                        matched[*candidate_index] = true;
                    }
                }
                if matches!(kind, PathMatchKind::FullyConsumed | PathMatchKind::SegmentBoundary) {
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
        conditions: Option<CompiledConditionSet>,
    },
    RemoteIp {
        ips: RuntimeIpSet,
        conditions: Option<CompiledConditionSet>,
    },
    Url(Vec<usize>),
}

impl RuntimeRule {
    #[allow(clippy::too_many_arguments)]
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
            Self::DirectPeerIp { ips, conditions } => {
                if !ips.matches(direct_peer_ip) {
                    PolicyMatch::NoMatch
                } else {
                    supplemental_matches(conditions.as_ref(), session, log).await
                }
            }
            Self::RemoteIp { ips, conditions } => {
                if !ips.matches(remote_ip) {
                    PolicyMatch::NoMatch
                } else {
                    supplemental_matches(conditions.as_ref(), session, log).await
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
                    match supplemental_matches(candidate.conditions.as_ref(), session, log).await {
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
                        conditions: config.conditions.as_ref().map(|conditions| {
                            CompiledConditionSet::new(conditions, "conditions", body_requirement.clone())
                        }),
                    },
                    RequestAccessRuleType::RemoteIp(config) => RuntimeRule::RemoteIp {
                        ips: RuntimeIpSet::new(config.ip),
                        conditions: config.conditions.as_ref().map(|conditions| {
                            CompiledConditionSet::new(conditions, "conditions", body_requirement.clone())
                        }),
                    },
                    RequestAccessRuleType::Url(config) => {
                        let candidates = config.resolved_candidates.unwrap_or_else(|| {
                            vec![UrlAccessCandidate {
                                name: rule.name,
                                hosts: config.hosts,
                                paths: config.paths,
                                conditions: config.conditions,
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
            NormalizedHostname::new(session.header_str("host").unwrap_or_default())
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
    default_profile: String,
    profiles: HashMap<String, RuntimeProfile>,
    active_profile_ref_key: Option<String>,
    status: u16,
    /// Prepared once per generation; `None` falls back to
    /// [`DEFAULT_DENY_BODY`], which never allocates.
    message: Option<Bytes>,
    init_failed: bool,
}

impl RequestAccessPolicy {
    pub(crate) fn create_bound(
        entry: HttpConfigDataEntry<'_>,
        body_requirement: Option<BodyRequirement>,
    ) -> Option<Box<dyn RequestFilter>> {
        let EdgionPlugin::RequestAccessPolicy(config) = entry.plugin() else {
            return None;
        };
        if entry.has_references() {
            entry.validate_owner().ok()?;
        }
        let (key, invalid_selector) = match entry.field().map(|field| field.runtime_key()) {
            None => (None, false),
            Some(Ok(Some(key))) => (Some(key), false),
            Some(Ok(None) | Err(_)) => (None, true),
        };
        Some(Self::build(config, key, invalid_selector, body_requirement))
    }

    pub(crate) fn create_inline(
        config: &RequestAccessPolicyConfig,
        body_requirement: Option<BodyRequirement>,
    ) -> Option<Box<dyn RequestFilter>> {
        if request_access_has_external_data(config) {
            return None;
        }
        Some(Self::build(config, None, false, body_requirement))
    }

    fn build(
        config: &RequestAccessPolicyConfig,
        active_profile_ref_key: Option<String>,
        invalid_selector: bool,
        body_requirement: Option<BodyRequirement>,
    ) -> Box<dyn RequestFilter> {
        let mut config = config.clone();
        let mut init_failed = config.validate_and_init().is_err() || invalid_selector;
        let mut profiles = HashMap::new();
        if !init_failed {
            let prepared_profiles = config
                .resolved_profiles
                .take()
                .unwrap_or_else(|| std::mem::take(&mut config.profiles));
            for (name, profile) in prepared_profiles {
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
        Box::new(Self {
            default_profile: config.default_profile,
            profiles,
            active_profile_ref_key,
            status: config.status,
            message: config.message.map(Bytes::from),
            init_failed,
        })
    }

    fn active_profile<'a>(&'a self, log: &mut PluginLog) -> Option<&'a RuntimeProfile> {
        if let Some(key) = &self.active_profile_ref_key {
            let store = get_global_edgion_config_data_store();
            let data = store.get_raw_by_key(key);
            match data.as_deref() {
                Some(value) if value.spec.enable => match &value.spec.data {
                    ConfigEntry::Selector(selector) => {
                        let selected = selector.active.as_deref().unwrap_or(self.default_profile.as_str());
                        return self.profiles.get(selected);
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
        self.profiles.get(self.default_profile.as_str())
    }

    fn deny(&self) -> PluginRunningResult {
        PluginRunningResult::ErrResponse {
            status: self.status,
            body: Some(self.message.clone().unwrap_or(DEFAULT_DENY_BODY)),
        }
    }
}

#[async_trait]
impl RequestFilter for RequestAccessPolicy {
    fn name(&self) -> &str {
        "RequestAccessPolicy"
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
#[path = "../../../../tests/unit/plugins/http/request_access_policy/plugin_test.rs"]
mod tests;
