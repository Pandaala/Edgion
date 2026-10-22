//! Cold-path sharing of one immutable source entry's constructed stage filters.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use edgion_resources::resources::edgion_plugins::{CorsConfig, EdgionPlugin, RequestFilterEntry};

use super::plugin_runtime::{InvalidPlugins, WafCandidate};
use super::traits::{RequestFilter, UpstreamResponse, UpstreamResponseBodyFilter, UpstreamResponseFilter};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EntryStage {
    Request,
    ResponseFilter,
    ResponseBody,
    Response,
    Waf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EntrySlot {
    pub stage: EntryStage,
    pub index: usize,
}

pub(crate) type EntryDemand = HashSet<EntrySlot>;

/// Conservative certificate from successful typed stage construction. Dynamic
/// conditions cannot remove an unsafe companion from this cold certificate.
#[derive(Clone)]
pub(crate) struct CacheCapabilities {
    pub safe: bool,
    pub credential_headers: Vec<String>,
}

impl Default for CacheCapabilities {
    fn default() -> Self {
        Self {
            safe: true,
            credential_headers: Vec::new(),
        }
    }
}

impl CacheCapabilities {
    pub(crate) fn for_constructed(plugin: &EdgionPlugin, stage: EntryStage) -> Self {
        use edgion_resources::common::KeyGet;
        let mut certificate = Self::default();
        certificate.safe = match stage {
            EntryStage::Request => match plugin {
                EdgionPlugin::RequestHeaderModifier(_)
                | EdgionPlugin::RequestRedirect(_)
                | EdgionPlugin::ProxyRewrite(_)
                | EdgionPlugin::RequestBodyBuffer(_)
                | EdgionPlugin::ResponseCache(_)
                | EdgionPlugin::RequestRestriction(_)
                | EdgionPlugin::RateLimitLocal(_)
                | EdgionPlugin::RateLimitRedis(_)
                | EdgionPlugin::RealIp(_)
                | EdgionPlugin::RequestId(_)
                | EdgionPlugin::Cors(_)
                | EdgionPlugin::CtxSet(_)
                | EdgionPlugin::GraphqlGuard(_)
                | EdgionPlugin::Dsl(_) => true,
                EdgionPlugin::KeyAuth(config) => {
                    let safe = config.hide_credentials
                        && config.anonymous.is_none()
                        && config.upstream_header_fields.is_empty()
                        && !config.key_sources.is_empty()
                        && config
                            .key_sources
                            .iter()
                            .all(|source| matches!(source, KeyGet::Header { .. }));
                    if safe {
                        certificate.credential_headers = config
                            .key_sources
                            .iter()
                            .filter_map(|source| match source {
                                KeyGet::Header { name } => Some(name.to_ascii_lowercase()),
                                _ => None,
                            })
                            .collect();
                    }
                    safe
                }
                _ => false,
            },
            EntryStage::ResponseFilter | EntryStage::Response => matches!(
                plugin,
                EdgionPlugin::ResponseHeaderModifier(_) | EdgionPlugin::Cors(_) | EdgionPlugin::RequestId(_)
            ),
            EntryStage::ResponseBody | EntryStage::Waf => false,
        };
        certificate
    }

    pub(crate) fn combine(&mut self, other: &Self) {
        self.safe &= other.safe;
        for name in &other.credential_headers {
            if !self.credential_headers.contains(name) {
                self.credential_headers.push(name.clone());
            }
        }
    }
}

/// A request entry may also own a response companion. These filters are always
/// cached and selected together; no extra dispatch wrapper is introduced.
pub(crate) struct EntryBundle {
    pub cache_capabilities: CacheCapabilities,
    pub requires_complete_request_body: bool,
    pub has_ai_conversion_owner: bool,
    pub has_unary_body_consumer: bool,
    pub request: Vec<Arc<dyn RequestFilter>>,
    pub response_filter: Vec<Arc<dyn UpstreamResponseFilter>>,
    pub response_body: Vec<Arc<dyn UpstreamResponseBodyFilter>>,
    pub response: Vec<Arc<dyn UpstreamResponse>>,
    pub cors: Option<Arc<CorsConfig>>,
    pub waf: Option<WafCandidate>,
    pub invalid: InvalidPlugins,
    pub errors: Vec<String>,
}

/// Failure remains explicit even if construction produced no executable filter.
pub(crate) enum EntryOutcome {
    Ready(EntryBundle),
    Failed(EntryBundle),
}

impl EntryOutcome {
    pub fn bundle(&self) -> &EntryBundle {
        match self {
            Self::Ready(bundle) | Self::Failed(bundle) => bundle,
        }
    }
}

/// Owned by exactly one immutable source generation, never shared across sources.
/// The publisher supplies the union of active demands when releasing slots.
#[derive(Default)]
pub(crate) struct EntryCache {
    slots: HashMap<EntrySlot, EntryOutcome>,
    request_outcome_aliases: Option<HashMap<String, usize>>,
    request_source_aliases: Vec<Option<String>>,
}

impl EntryCache {
    /// Freeze bindings before policy masking. Sharing a producer across views
    /// must not depend on whether its consumer was permitted in the first view.
    pub fn prepare_request_outcomes(&mut self, entries: &[RequestFilterEntry], base: usize) -> &HashMap<String, usize> {
        if self.request_outcome_aliases.is_none() {
            self.request_source_aliases = entries.iter().map(|entry| entry.alias.clone()).collect();
        }
        self.request_outcome_aliases.get_or_insert_with(|| {
            let referenced: HashSet<&str> = entries
                .iter()
                .filter(|entry| entry.is_enabled())
                .filter_map(|entry| entry.conditions.as_ref()?.run.as_ref())
                .flat_map(|set| set.iter_conditions())
                .filter_map(|condition| match condition {
                    edgion_resources::resources::edgion_plugins::conditions::Condition::RateLimitExceeded(config) => {
                        Some(config.plugin_alias.as_str())
                    }
                    _ => None,
                })
                .collect();
            entries
                .iter()
                .filter(|entry| entry.is_enabled() && matches!(entry.plugin, EdgionPlugin::RateLimitLocal(_)))
                .filter_map(|entry| entry.alias.as_deref().filter(|alias| referenced.contains(alias)))
                // Outcome slots are private generation bindings, not EntrySlot indexes.
                .enumerate()
                .map(|(index, alias)| (alias.to_owned(), base + index))
                .collect()
        })
    }

    pub fn request_source_aliases(&self) -> &[Option<String>] {
        &self.request_source_aliases
    }

    pub fn get(&self, slot: EntrySlot) -> Option<&EntryOutcome> {
        self.slots.get(&slot)
    }

    pub fn insert(&mut self, slot: EntrySlot, outcome: EntryOutcome) {
        self.slots.insert(slot, outcome);
    }

    pub fn release_unneeded_entries(&mut self, needed: &EntryDemand) {
        self.slots.retain(|slot, _| needed.contains(slot));
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn keys(&self) -> impl Iterator<Item = EntrySlot> + '_ {
        self.slots.keys().copied()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/entry_bundle_test.rs"]
mod tests;
