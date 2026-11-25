//! Immutable HTTP plugin source and its bounded, demand-owned constructor cache.

use std::collections::HashSet;
use std::sync::Arc;

use edgion_resources::resources::edgion_plugins::{duplicate_alias_error, EdgionPlugins};
use edgion_resources::resources::plugin_policy::{CompiledPluginPolicy, PluginFamily, PluginPolicyAction};

use super::plugin_store::{clone_for_compilation, compile_plugins_with_cache, CompiledPlugins};
use crate::plugins::runtime::entry_bundle::{EntryCache, EntryDemand, EntrySlot, EntryStage};

/// Slots never cross this immutable source boundary. Policy edits reuse this
/// owner; source replacement creates another owner, even for the same key.
pub(crate) struct PluginGeneration {
    source: EdgionPlugins,
    alias_errors: Vec<(EntrySlot, String)>,
    cache: EntryCache,
}

impl PluginGeneration {
    pub(crate) fn from_source(source: &EdgionPlugins) -> Self {
        Self::new(clone_for_compilation(source, |_| true))
    }

    pub fn new(mut source: EdgionPlugins) -> Self {
        let mut cache = EntryCache::default();
        cache.prepare_request_outcomes(source.spec.request_plugins.as_deref().unwrap_or_default(), 0);
        let mut alias_errors = Vec::new();
        macro_rules! prepare {
            ($entries:expr, $stage:ident, $label:literal) => {
                if let Some(entries) = $entries.as_mut() {
                    let mut aliases = HashSet::new();
                    for (index, entry) in entries.iter_mut().enumerate() {
                        if !entry.is_enabled() || entry.plugin.is_non_executable() {
                            continue;
                        }
                        let error = entry
                            .validate_alias()
                            .or_else(|| duplicate_alias_error(&mut aliases, entry.alias.as_deref()));
                        if let Some(error) = error {
                            alias_errors.push((
                                EntrySlot {
                                    stage: EntryStage::$stage,
                                    index,
                                },
                                format!("{}[{}] ({}): {}", $label, index, entry.plugin.type_name(), error),
                            ));
                            entry.alias = None;
                        }
                    }
                }
            };
        }
        prepare!(source.spec.request_plugins, Request, "requestPlugins");
        prepare!(
            source.spec.upstream_response_filter_plugins,
            ResponseFilter,
            "upstreamResponseFilterPlugins"
        );
        prepare!(
            source.spec.upstream_response_body_filter_plugins,
            ResponseBody,
            "upstreamResponseBodyFilterPlugins"
        );
        prepare!(
            source.spec.upstream_response_plugins,
            Response,
            "upstreamResponsePlugins"
        );
        Self {
            source,
            alias_errors,
            cache,
        }
    }

    pub fn source(&self) -> &EdgionPlugins {
        &self.source
    }

    /// Preserve declaration order so the first current-source Block determines
    /// the response before restrictions found only in a retained candidate.
    pub fn policy_actions(&self, policy: Option<&CompiledPluginPolicy>) -> Vec<(&'static str, PluginPolicyAction)> {
        let mut actions = Vec::new();
        macro_rules! collect {
            ($entries:expr) => {
                for entry in $entries.iter().flatten().filter(|entry| entry.is_enabled()) {
                    if let Some(action) =
                        policy.and_then(|p| p.action_for(PluginFamily::Http, entry.plugin.policy_type_name()))
                    {
                        actions.push((entry.plugin.policy_type_name(), action));
                    }
                }
            };
        }
        collect!(self.source.spec.request_plugins);
        collect!(self.source.spec.upstream_response_filter_plugins);
        collect!(self.source.spec.upstream_response_body_filter_plugins);
        collect!(self.source.spec.upstream_response_plugins);
        actions
    }

    /// Demand is the union of these sets across scopes that currently attach
    /// this resource. Computing a mask never constructs an executable plugin.
    pub fn demand(&self, policy: Option<&CompiledPluginPolicy>) -> EntryDemand {
        let mut needed = EntryDemand::new();
        macro_rules! collect {
            ($entries:expr, $stage:ident) => {
                for (index, entry) in $entries.iter().flatten().enumerate() {
                    if entry.is_enabled()
                        && policy
                            .and_then(|p| p.action_for(PluginFamily::Http, entry.plugin.policy_type_name()))
                            .is_none()
                    {
                        needed.insert(EntrySlot {
                            stage: EntryStage::$stage,
                            index,
                        });
                    }
                }
            };
        }
        collect!(self.source.spec.request_plugins, Request);
        collect!(self.source.spec.upstream_response_filter_plugins, ResponseFilter);
        collect!(self.source.spec.upstream_response_body_filter_plugins, ResponseBody);
        collect!(self.source.spec.upstream_response_plugins, Response);
        if self.source.spec.waf.is_some() {
            needed.insert(EntrySlot {
                stage: EntryStage::Waf,
                index: 0,
            });
        }
        needed
    }

    /// Construct only permitted slots and assemble a new effective view.
    /// Block/Bypass gates are recorded separately from transport entry fields.
    pub fn compile(&mut self, policy: Option<&CompiledPluginPolicy>) -> Arc<CompiledPlugins> {
        self.compile_with_additional_actions(policy, &[])
    }

    /// A retained executable source must also enforce restrictions declared by
    /// the latest source, including entries that the retained source never had.
    pub fn compile_with_additional_actions(
        &mut self,
        policy: Option<&CompiledPluginPolicy>,
        extra_actions: &[(&'static str, PluginPolicyAction)],
    ) -> Arc<CompiledPlugins> {
        let needed = self.demand(policy);
        let mut effective = clone_for_compilation(&self.source, |index| {
            needed.contains(&EntrySlot {
                stage: EntryStage::Request,
                index,
            })
        });
        let mut actions = extra_actions.to_vec();
        actions.extend(self.policy_actions(policy));
        let mut bypassed = HashSet::new();
        actions.retain(|(name, action)| *action != PluginPolicyAction::Bypass || bypassed.insert(*name));
        macro_rules! mask {
            ($entries:expr) => {
                for entry in $entries.iter_mut().flatten() {
                    if !entry.is_enabled() {
                        continue;
                    }
                    if policy
                        .and_then(|p| p.action_for(PluginFamily::Http, entry.plugin.policy_type_name()))
                        .is_some()
                    {
                        entry.enable = false;
                    }
                }
            };
        }
        mask!(effective.spec.request_plugins);
        mask!(effective.spec.upstream_response_filter_plugins);
        mask!(effective.spec.upstream_response_body_filter_plugins);
        mask!(effective.spec.upstream_response_plugins);
        let errors = self
            .alias_errors
            .iter()
            .filter(|(slot, _)| needed.contains(slot))
            .map(|(_, error)| error.clone())
            .collect();
        compile_plugins_with_cache(&effective, &mut self.cache, &actions, errors)
    }

    /// Clear publisher-owned strong references after all active scopes have
    /// contributed demand. In-flight runtime Arcs retain their own filters.
    pub fn release_unneeded_entries(&mut self, needed: &EntryDemand) {
        self.cache.release_unneeded_entries(needed);
    }

    pub fn cached_entry_count(&self) -> usize {
        self.cache.len()
    }

    pub fn cached_entry_keys(&self) -> impl Iterator<Item = EntrySlot> + '_ {
        self.cache.keys()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/http/plugin_generation_test.rs"]
mod tests;
