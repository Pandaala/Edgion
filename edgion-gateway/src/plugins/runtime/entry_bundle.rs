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

/// A request entry may also own a response companion. These filters are always
/// cached and selected together; no extra dispatch wrapper is introduced.
pub(crate) struct EntryBundle {
    pub requires_complete_request_body: bool,
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
                .enumerate()
                .filter(|(_, entry)| entry.is_enabled() && matches!(entry.plugin, EdgionPlugin::RateLimitLocal(_)))
                .filter_map(|(index, entry)| {
                    entry
                        .alias
                        .as_deref()
                        .filter(|alias| referenced.contains(alias))
                        .map(|alias| (alias.to_owned(), base + index))
                })
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
