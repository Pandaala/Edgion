//! RequestAccessPolicy L7 request filter.

use edgion_resources::refs::config_data_fields::{request_access_has_external_data, HttpConfigDataEntry};
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::IpAddr;

use crate::plugins::runtime::conditions::ip_refs::{BoundIpPredicate, IpConditionCompiler, ProfileSelection};
use async_trait::async_trait;
use bytes::Bytes;
use edgion_algo::matcher::radix_path::{PathMatchKind, PathRadixBuilder, PathRadixTree};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    AccessPathMatch, AccessPathMatchType, RequestAccessPolicyConfig, RequestAccessProfile, RequestAccessRuleType,
};
use edgion_resources::resources::edgion_plugins::BodyRequirement;
use regex::Regex;
use std::sync::Arc;

use crate::ctx::NormalizedHostname;
use crate::plugins::runtime::conditions::{CompiledConditionSet, MatchState};
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};

/// Fallback deny body used when the operator configured no `message`.
const DEFAULT_DENY_BODY: Bytes = Bytes::from_static(b"Access denied");

struct RuntimeIpSet(Option<Arc<BoundIpPredicate>>);
impl RuntimeIpSet {
    fn matches(&self, session: &dyn PluginSession, ip: Option<&IpAddr>) -> bool {
        self.0
            .as_ref()
            .is_some_and(|refs| refs.evaluate(session, ip.copied(), false) == MatchState::True)
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
    needs_host: bool,
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
    fn compile(
        candidates: Vec<(
            &[String],
            &[AccessPathMatch],
            Option<&edgion_resources::resources::edgion_plugins::conditions::ConditionSet>,
        )>,
        body_requirement: Option<BodyRequirement>,
        ip: Option<&IpConditionCompiler<'_>>,
    ) -> Result<Self, String> {
        let mut runtime_candidates = Vec::with_capacity(candidates.len());
        let mut paths: BTreeMap<String, PathBucket> = BTreeMap::new();
        let mut regex_paths = Vec::new();
        for (hosts, candidate_paths, conditions) in candidates {
            let candidate_index = runtime_candidates.len();
            runtime_candidates.push(RuntimeUrlCandidate {
                hosts: hosts
                    .iter()
                    .map(|host| NormalizedHostname::new(host).as_str().to_string())
                    .collect(),
                conditions: conditions.map(|conditions| {
                    CompiledConditionSet::with_ip(conditions, "conditions", body_requirement.clone(), ip)
                }),
            });
            for path in candidate_paths {
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
        let needs_host = runtime_candidates.iter().any(|candidate| !candidate.hosts.is_empty());
        Ok(Self {
            candidates: runtime_candidates,
            needs_host,
            path_buckets,
            path_tree,
            regex_paths,
        })
    }

    fn lookup(&self, path: &str) -> Vec<bool> {
        let mut matched = vec![false; self.candidates.len()];
        if let Some(tree) = &self.path_tree {
            tree.for_each_match_ext(path, |bucket_index, kind| {
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
            });
        }
        for (candidate_index, regex) in &self.regex_paths {
            if !matched[*candidate_index] && regex.is_match(path) {
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
    Url(std::ops::Range<usize>),
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
                if !ips.matches(session, direct_peer_ip) {
                    PolicyMatch::NoMatch
                } else {
                    supplemental_matches(conditions.as_ref(), session, log).await
                }
            }
            Self::RemoteIp { ips, conditions } => {
                if !ips.matches(session, remote_ip) {
                    PolicyMatch::NoMatch
                } else {
                    supplemental_matches(conditions.as_ref(), session, log).await
                }
            }
            Self::Url(indices) => {
                for index in indices.clone() {
                    if !matched_urls.get(index).copied().unwrap_or(false) {
                        continue;
                    }
                    let candidate = &url_index.candidates[index];
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
    needs_direct_peer_ip: bool,
    needs_remote_ip: bool,
}

impl RuntimeProfile {
    fn new(
        config: &RequestAccessProfile,
        body_requirement: Option<BodyRequirement>,
        ip: Option<&IpConditionCompiler<'_>>,
    ) -> Result<Self, String> {
        let mut url_candidates = Vec::new();
        let mut groups = Vec::with_capacity(config.required_rule_groups.len());
        for group in &config.required_rule_groups {
            let mut rules = Vec::with_capacity(group.any_of_rules.len());
            for rule in &group.any_of_rules {
                rules.push(match &rule.rule {
                    RequestAccessRuleType::DirectPeerIp(config) => RuntimeRule::DirectPeerIp {
                        ips: RuntimeIpSet(ip.and_then(|ip| ip.bind(&config.refs))),
                        conditions: config.conditions.as_ref().map(|conditions| {
                            CompiledConditionSet::with_ip(conditions, "conditions", body_requirement.clone(), ip)
                        }),
                    },
                    RequestAccessRuleType::RemoteIp(config) => RuntimeRule::RemoteIp {
                        ips: RuntimeIpSet(ip.and_then(|ip| ip.bind(&config.refs))),
                        conditions: config.conditions.as_ref().map(|conditions| {
                            CompiledConditionSet::with_ip(conditions, "conditions", body_requirement.clone(), ip)
                        }),
                    },
                    RequestAccessRuleType::Url(config) => {
                        let start = url_candidates.len();
                        if let Some(candidates) = &config.resolved_candidates {
                            url_candidates.extend(candidates.iter().map(|candidate| {
                                (
                                    candidate.hosts.as_slice(),
                                    candidate.paths.as_slice(),
                                    candidate.conditions.as_ref(),
                                )
                            }));
                        } else {
                            url_candidates.push((
                                config.hosts.as_slice(),
                                config.paths.as_slice(),
                                config.conditions.as_ref(),
                            ));
                        }
                        RuntimeRule::Url(start..url_candidates.len())
                    }
                });
            }
            groups.push(RuntimeRuleGroup(rules));
        }
        let needs_direct_peer_ip = groups
            .iter()
            .flat_map(|group| &group.0)
            .any(|rule| matches!(rule, RuntimeRule::DirectPeerIp { .. }));
        let needs_remote_ip = groups
            .iter()
            .flat_map(|group| &group.0)
            .any(|rule| matches!(rule, RuntimeRule::RemoteIp { .. }));
        Ok(Self {
            groups,
            url_index: RuntimeUrlIndex::compile(url_candidates, body_requirement, ip)?,
            needs_direct_peer_ip,
            needs_remote_ip,
        })
    }

    async fn allows(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PolicyMatch {
        let direct_peer_ip = self
            .needs_direct_peer_ip
            .then(|| session.direct_peer_ip().parse::<IpAddr>().ok())
            .flatten();
        let remote_ip = self
            .needs_remote_ip
            .then(|| session.remote_ip().parse::<IpAddr>().ok())
            .flatten();
        // One profile-wide path index is queried before host or secondary-rule checks.
        let matched_urls = if self.url_index.candidates.is_empty() {
            Vec::new()
        } else {
            self.url_index.lookup(session.get_path())
        };
        let host = if !self.url_index.needs_host {
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
    profiles: Vec<RuntimeProfile>,
    profile_names: HashMap<String, usize>,
    active_profile_ref_key: Option<String>,
    status: u16,
    /// Prepared once per generation; `None` falls back to
    /// [`DEFAULT_DENY_BODY`], which never allocates.
    message: Option<Bytes>,
    init_failed: bool,
    needs_data_view: bool,
}

impl RequestAccessPolicy {
    pub(crate) fn create_bound(
        entry: HttpConfigDataEntry<'_>,
        body_requirement: Option<BodyRequirement>,
        ip: Option<&IpConditionCompiler<'_>>,
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
        Some(Self::build(config, key, invalid_selector, body_requirement, ip))
    }

    pub(crate) fn create_inline(
        config: &RequestAccessPolicyConfig,
        body_requirement: Option<BodyRequirement>,
        ip: Option<&IpConditionCompiler<'_>>,
    ) -> Option<Box<dyn RequestFilter>> {
        if request_access_has_external_data(config) {
            return None;
        }
        Some(Self::build(config, None, false, body_requirement, ip))
    }

    fn build(
        config: &RequestAccessPolicyConfig,
        active_profile_ref_key: Option<String>,
        invalid_selector: bool,
        body_requirement: Option<BodyRequirement>,
        ip: Option<&IpConditionCompiler<'_>>,
    ) -> Box<dyn RequestFilter> {
        let mut validation = config.clone();
        let mut init_failed = validation.validate_and_init().is_err() || invalid_selector;
        let mut profiles = Vec::new();
        let mut profile_names = HashMap::new();
        if !init_failed {
            let prepared_profiles = config.resolved_profiles.as_ref().unwrap_or(&config.profiles);
            let mut names: Vec<_> = prepared_profiles.keys().collect();
            names.sort();
            for name in names {
                let profile = &prepared_profiles[name];
                match RuntimeProfile::new(profile, body_requirement.clone(), ip) {
                    Ok(profile) => {
                        profile_names.insert(name.clone(), profiles.len());
                        profiles.push(profile);
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
        let needs_data_view = active_profile_ref_key.is_some()
            || profiles.iter().any(|profile| {
                profile
                    .groups
                    .iter()
                    .any(|group| group.0.iter().any(|rule| !matches!(rule, RuntimeRule::Url(_))))
                    || profile.url_index.candidates.iter().any(|candidate| {
                        candidate
                            .conditions
                            .as_ref()
                            .is_some_and(CompiledConditionSet::requires_config_data_view)
                    })
            });
        Box::new(Self {
            default_profile: config.default_profile.clone(),
            profiles,
            profile_names,
            active_profile_ref_key,
            status: config.status,
            message: config.message.clone().map(Bytes::from),
            init_failed,
            needs_data_view,
        })
    }

    fn active_profile<'a>(&'a self, session: &dyn PluginSession, log: &mut PluginLog) -> Option<&'a RuntimeProfile> {
        let index = if let Some(key) = &self.active_profile_ref_key {
            match session.config_data_profile_index(key, &self.default_profile, &self.profile_names) {
                ProfileSelection::Selected(index) => Some(index),
                ProfileSelection::WrongType => {
                    log.push_err("selector type mismatch");
                    None
                }
                ProfileSelection::Unavailable => {
                    log.push_err("selector unavailable");
                    None
                }
                ProfileSelection::UnknownProfile => None,
            }
        } else {
            self.profile_names.get(&self.default_profile).copied()
        };
        index.and_then(|index| self.profiles.get(index))
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
        if self.needs_data_view {
            session.ensure_config_data_view();
        }
        let Some(profile) = self.active_profile(session, log) else {
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
