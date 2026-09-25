//! Source-bound stream implementations shared across policy views.

use std::sync::Arc;

use edgion_resources::resources::plugin_policy::{CompiledPluginPolicy, PluginFamily};
use edgion_resources::{EdgionStreamPlugins, ResourceMeta};

use super::stream_plugin_runtime::StreamEntryOutcome;
use super::{CompiledStreamPlugins, StreamPluginRuntime, TlsRoutePlugin, TlsRoutePluginRuntime};

/// Stages actually referenced by serving attachments in one scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StreamStages {
    pub connection: bool,
    pub tls_route: bool,
}

impl StreamStages {
    pub(crate) fn extend(&mut self, other: Self) {
        self.connection |= other.connection;
        self.tls_route |= other.tls_route;
    }
}

#[derive(Default)]
pub(crate) struct StreamEntryDemand {
    connection: Vec<bool>,
    tls_route: Vec<bool>,
}

impl StreamEntryDemand {
    pub(crate) fn extend(&mut self, other: &Self) {
        self.connection
            .resize(self.connection.len().max(other.connection.len()), false);
        self.tls_route
            .resize(self.tls_route.len().max(other.tls_route.len()), false);
        for (needed, incoming) in self.connection.iter_mut().zip(&other.connection) {
            *needed |= incoming;
        }
        for (needed, incoming) in self.tls_route.iter_mut().zip(&other.tls_route) {
            *needed |= incoming;
        }
    }
}

/// This cache is bound to the complete immutable source, never just a resource
/// name or plugin type. The publisher releases slots without allowing demand.
pub(crate) struct StreamPluginGeneration {
    source: Arc<EdgionStreamPlugins>,
    connection: Vec<Option<StreamEntryOutcome>>,
    tls_route: Vec<Option<Arc<dyn TlsRoutePlugin>>>,
}

impl StreamPluginGeneration {
    pub(crate) fn new(source: Arc<EdgionStreamPlugins>) -> Self {
        Self {
            connection: vec![None; source.spec.plugins.as_ref().map_or(0, Vec::len)],
            tls_route: vec![None; source.spec.tls_route_plugins.as_ref().map_or(0, Vec::len)],
            source,
        }
    }

    pub(crate) fn demand(&self, policy: &CompiledPluginPolicy, stages: StreamStages) -> StreamEntryDemand {
        StreamEntryDemand {
            connection: self
                .source
                .spec
                .plugins
                .iter()
                .flatten()
                .map(|entry| {
                    stages.connection
                        && entry.is_enabled()
                        && policy.action_for(PluginFamily::Stream, entry.type_name()).is_none()
                })
                .collect(),
            tls_route: self
                .source
                .spec
                .tls_route_plugins
                .iter()
                .flatten()
                .map(|entry| {
                    stages.tls_route
                        && entry.is_enabled()
                        && policy.action_for(PluginFamily::TlsRoute, entry.type_name()).is_none()
                })
                .collect(),
        }
    }

    pub(crate) fn compile(
        &mut self,
        policy: &CompiledPluginPolicy,
        stages: StreamStages,
    ) -> Arc<CompiledStreamPlugins> {
        let namespace = self.source.metadata.namespace.as_deref().unwrap_or("default");
        let unsupported = !self.source.unsupported_plugin_errors().is_empty();
        let stream_plugin_runtime = if stages.connection && unsupported {
            StreamPluginRuntime::unsupported_source()
        } else if stages.connection {
            StreamPluginRuntime::from_cached_entries(
                self.source.spec.plugins.as_deref().unwrap_or_default(),
                namespace,
                &self.source.key_name(),
                Some(policy),
                &mut self.connection,
            )
        } else {
            StreamPluginRuntime::new()
        };
        let tls_route_plugin_runtime = if stages.tls_route && unsupported {
            TlsRoutePluginRuntime::unsupported_source()
        } else if stages.tls_route {
            TlsRoutePluginRuntime::from_cached_entries(
                self.source.spec.tls_route_plugins.as_deref().unwrap_or_default(),
                namespace,
                Some(policy),
                &mut self.tls_route,
            )
        } else {
            TlsRoutePluginRuntime::new()
        };
        Arc::new(CompiledStreamPlugins {
            spec: Arc::clone(&self.source),
            stream_plugin_runtime: Arc::new(stream_plugin_runtime),
            tls_route_plugin_runtime: Arc::new(tls_route_plugin_runtime),
        })
    }

    pub(crate) fn release_unneeded_entries(&mut self, demand: &StreamEntryDemand) {
        for (index, slot) in self.connection.iter_mut().enumerate() {
            if !demand.connection.get(index).copied().unwrap_or(false) {
                *slot = None;
            }
        }
        for (index, slot) in self.tls_route.iter_mut().enumerate() {
            if !demand.tls_route.get(index).copied().unwrap_or(false) {
                *slot = None;
            }
        }
    }

    pub(crate) fn cached_entry_count(&self) -> usize {
        self.connection.iter().filter(|slot| slot.is_some()).count()
            + self.tls_route.iter().filter(|slot| slot.is_some()).count()
    }

    pub(crate) fn cached_entry_keys(&self) -> Vec<(bool, usize)> {
        self.connection
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.as_ref().map(|_| (false, index)))
            .chain(
                self.tls_route
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| entry.as_ref().map(|_| (true, index))),
            )
            .collect()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/stream/plugin_generation_test.rs"]
mod tests;
