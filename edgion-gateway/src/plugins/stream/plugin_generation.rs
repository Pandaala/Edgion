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
        let stream_plugin_runtime = if stages.connection {
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
        let tls_route_plugin_runtime = if stages.tls_route {
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
mod tests {
    use super::*;
    use crate::plugins::stream::{StreamContext, StreamLayer, StreamPluginResult};
    use edgion_resources::resources::plugin_policy::PluginPolicyConfig;

    const CONNECTION: StreamStages = StreamStages {
        connection: true,
        tls_route: false,
    };

    fn source() -> Arc<EdgionStreamPlugins> {
        Arc::new(
            serde_yaml::from_str(
                r#"
apiVersion: edgion.io/v1alpha1
kind: EdgionStreamPlugins
metadata: {name: shared-generation, namespace: default}
spec:
  plugins:
    - type: ConnectionRateLimit
      config:
        perListener: {rate: 2, interval: 1h}
  tlsRoutePlugins:
    - type: IpRestriction
      config: {defaultAction: allow}
"#,
            )
            .unwrap(),
        )
    }

    fn policy(action: &str) -> CompiledPluginPolicy {
        serde_yaml::from_str::<PluginPolicyConfig>(&format!(
            "deny:\n  - name: stream/ConnectionRateLimit\n    action: {action}"
        ))
        .unwrap()
        .compile()
        .unwrap()
    }

    #[tokio::test]
    async fn scopes_share_the_real_rate_limit_budget_and_revoke_releases_owner() {
        let mut generation = StreamPluginGeneration::new(source());
        let allow = CompiledPluginPolicy::default();
        let first = generation.compile(&allow, CONNECTION);
        let cached = generation.connection[0].as_ref().unwrap().as_ref().unwrap();
        let owner = Arc::downgrade(cached);
        let second = generation.compile(&allow, CONNECTION);
        assert_eq!(owner.strong_count(), 3, "one cache and two views share the same filter");
        let context = StreamContext::new("127.0.0.1".parse().unwrap(), 18491, StreamLayer::Stage1);
        assert!(matches!(
            first.stream_plugin_runtime.run(&context).await,
            StreamPluginResult::Allow
        ));
        assert!(matches!(
            second.stream_plugin_runtime.run(&context).await,
            StreamPluginResult::Allow
        ));
        assert!(matches!(
            first.stream_plugin_runtime.run(&context).await,
            StreamPluginResult::Deny(_)
        ));
        let bypass = generation.compile(&policy("bypass"), CONNECTION);
        assert!(bypass.stream_plugin_runtime.is_empty());
        generation.release_unneeded_entries(&StreamEntryDemand::default());
        assert_eq!(generation.cached_entry_count(), 0);
        assert!(owner.upgrade().is_some(), "admitted views keep their instance");
        drop(first);
        drop(second);
        assert!(owner.upgrade().is_none(), "a bypass view holds no executable reference");
        let restored = generation.compile(&allow, CONNECTION);
        assert!(matches!(
            restored.stream_plugin_runtime.run(&context).await,
            StreamPluginResult::Deny(_)
        ));
    }

    #[tokio::test]
    async fn denied_and_unreferenced_stages_never_construct_entries() {
        let mut generation = StreamPluginGeneration::new(source());
        let context = StreamContext::new("127.0.0.1".parse().unwrap(), 18492, StreamLayer::Stage1);
        let blocked = generation.compile(&policy("block"), CONNECTION);
        assert!(matches!(
            blocked.stream_plugin_runtime.run(&context).await,
            StreamPluginResult::Deny(_)
        ));
        assert_eq!(generation.cached_entry_count(), 0);
        assert!(blocked.tls_route_plugin_runtime.is_empty());
        let bypass = generation.compile(&policy("bypass"), CONNECTION);
        assert!(bypass.stream_plugin_runtime.is_empty());
        assert_eq!(generation.cached_entry_count(), 0);
        let tls = StreamStages {
            connection: false,
            tls_route: true,
        };
        let _tls_view = generation.compile(&CompiledPluginPolicy::default(), tls);
        assert_eq!(generation.cached_entry_count(), 1);
        let mut demand = generation.demand(&CompiledPluginPolicy::default(), tls);
        demand.extend(&generation.demand(&policy("block"), CONNECTION));
        generation.release_unneeded_entries(&demand);
        assert_eq!(generation.cached_entry_count(), 1);
        assert!(generation.connection[0].is_none());
    }

    #[tokio::test]
    async fn failed_entries_stay_fail_closed_and_do_not_pollute_bypassed_scope() {
        let mut source = (*source()).clone();
        source.spec.plugins = Some(
            serde_yaml::from_str(
                r#"
- type: GeoIpLocation
  config: {}
"#,
            )
            .unwrap(),
        );
        let mut generation = StreamPluginGeneration::new(Arc::new(source));
        let allow = CompiledPluginPolicy::default();
        let invalid = generation.compile(&allow, CONNECTION);
        assert!(generation.connection[0].as_ref().unwrap().is_err());
        let context = StreamContext::new("127.0.0.1".parse().unwrap(), 18493, StreamLayer::Stage1);
        assert!(matches!(
            invalid.stream_plugin_runtime.run(&context).await,
            StreamPluginResult::Deny(_)
        ));
        let bypass =
            serde_yaml::from_str::<PluginPolicyConfig>("deny:\n  - name: stream/GeoIpLocation\n    action: bypass")
                .unwrap()
                .compile()
                .unwrap();
        let scoped = generation.compile(&bypass, CONNECTION);
        assert!(scoped.stream_plugin_runtime.is_empty());
        generation.release_unneeded_entries(&generation.demand(&bypass, CONNECTION));
        assert_eq!(generation.cached_entry_count(), 0);
    }
}
