//! ExtensionRef filter implementation
//!
//! This filter handles references to EdgionPlugins resources.

use async_trait::async_trait;

use crate::core::plugins::edgion_plugins::get_global_plugin_store;
use crate::core::plugins::plugin_runtime::log::PluginLog;
use crate::core::plugins::plugin_runtime::traits::{PluginSession, RequestFilter, UpstreamResponseFilter};
use crate::types::filters::{PluginRunningResult, PluginRunningStage};
use crate::types::resources::LocalObjectReference;

/// Default maximum allowed nested plugin references to avoid infinite loops
pub const DEFAULT_PLUGIN_REF_DEPTH: usize = 5;

/// Filter that handles ExtensionRef to EdgionPlugins
#[derive(Clone)]
pub struct ExtensionRefFilter {
    /// The namespace to look up the plugin (from the HTTPRoute's namespace)
    namespace: String,
    /// The extension reference configuration
    ext_ref: LocalObjectReference,
    /// Max depth allowed for this filter instance
    max_depth: usize,
}

impl ExtensionRefFilter {
    pub fn new(namespace: String, ext_ref: LocalObjectReference, max_depth: usize) -> Self {
        Self {
            namespace,
            ext_ref,
            max_depth,
        }
    }

    #[inline]
    fn finish(session: &mut dyn PluginSession, result: PluginRunningResult) -> PluginRunningResult {
        session.pop_plugin_ref();
        result
    }

    /// Build the lookup key: namespace/name
    fn plugin_key(&self) -> String {
        format!("{}/{}", self.namespace, self.ext_ref.name)
    }

    /// Check if this extension ref points to EdgionPlugins
    fn is_edgion_plugins(&self) -> bool {
        self.ext_ref.kind == "EdgionPlugins" && (self.ext_ref.group.is_empty() || self.ext_ref.group == "edgion.io")
    }

    /// Execute the referenced EdgionPlugins's plugin runtime
    fn run_extension(
        &self,
        stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        if !self.is_edgion_plugins() {
            log.push(&format!(
                "ExtensionRef kind '{}' not supported, skipping",
                self.ext_ref.kind
            ));
            return PluginRunningResult::GoodNext;
        }

        // Set refer_to name on the ExtensionRef log (simplified from full LocalObjectReference)
        log.set_refer_to(self.ext_ref.name.clone());

        let key = self.plugin_key();
        let store = get_global_plugin_store();

        // Detect recursion and depth overflow
        if session.has_plugin_ref(&key) {
            log.push(&format!("Detected cyclic plugin reference '{}'", key));
            return PluginRunningResult::ErrTerminateRequest;
        }
        if session.plugin_ref_depth() >= self.max_depth {
            log.push(&format!(
                "Plugin reference depth exceeded {} while resolving '{}'",
                self.max_depth, key
            ));
            return PluginRunningResult::ErrTerminateRequest;
        }
        session.push_plugin_ref(key.clone());

        // Look up the EdgionPlugins in global store
        let Some(edgion_plugins) = store.get(&key) else {
            log.push(&format!("EdgionPlugins '{}' not found, returning 500", key));
            return Self::finish(session, PluginRunningResult::ErrTerminateRequest);
        };

        // Get the pre-compiled plugin runtime
        let plugin_runtime = &edgion_plugins.spec.plugin_runtime;

        // Start EdgionPlugins log with token (ensures correct order for nested refs)
        let log_token = session.start_edgion_plugins_log(self.ext_ref.name.clone());

        // Run edgion_plugins based on stage
        match stage {
            PluginRunningStage::Request => {
                log.push("Warning: Request stage should use async path");
                Self::finish(session, PluginRunningResult::GoodNext)
            }
            PluginRunningStage::UpstreamResponseFilter => {
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if result == PluginRunningResult::ErrTerminateRequest {
                        return Self::finish(session, result);
                    }
                }
                Self::finish(session, PluginRunningResult::GoodNext)
            }
            PluginRunningStage::UpstreamResponse => {
                // Not executed in sync path
                Self::finish(session, PluginRunningResult::GoodNext)
            }
        }
    }

    /// Async version for stages that require it
    async fn run_extension_async(
        &self,
        stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        if !self.is_edgion_plugins() {
            log.push(&format!(
                "ExtensionRef kind '{}' not supported, skipping",
                self.ext_ref.kind
            ));
            return PluginRunningResult::GoodNext;
        }

        // Set refer_to name on the ExtensionRef log (simplified from full LocalObjectReference)
        log.set_refer_to(self.ext_ref.name.clone());

        let key = self.plugin_key();
        let store = get_global_plugin_store();

        if session.has_plugin_ref(&key) {
            log.push(&format!("Detected cyclic plugin reference '{}'", key));
            return PluginRunningResult::ErrTerminateRequest;
        }
        if session.plugin_ref_depth() >= self.max_depth {
            log.push(&format!(
                "Plugin reference depth exceeded {} while resolving '{}'",
                self.max_depth, key
            ));
            return PluginRunningResult::ErrTerminateRequest;
        }
        session.push_plugin_ref(key.clone());

        let Some(edgion_plugins) = store.get(&key) else {
            log.push(&format!("EdgionPlugins '{}' not found, returning 500", key));
            return Self::finish(session, PluginRunningResult::ErrTerminateRequest);
        };

        let plugin_runtime = &edgion_plugins.spec.plugin_runtime;

        // Start EdgionPlugins log with token (ensures correct order for nested refs)
        let log_token = session.start_edgion_plugins_log(self.ext_ref.name.clone());

        match stage {
            PluginRunningStage::Request => {
                for plugin in plugin_runtime.request_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_request(session, &mut inner_log).await;
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if result == PluginRunningResult::ErrTerminateRequest {
                        return Self::finish(session, result);
                    }
                }
                Self::finish(session, PluginRunningResult::GoodNext)
            }
            PluginRunningStage::UpstreamResponse => {
                for plugin in plugin_runtime.upstream_response_async_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response(session, &mut inner_log).await;
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if result == PluginRunningResult::ErrTerminateRequest {
                        return Self::finish(session, result);
                    }
                }
                Self::finish(session, PluginRunningResult::GoodNext)
            }
            PluginRunningStage::UpstreamResponseFilter => {
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if result == PluginRunningResult::ErrTerminateRequest {
                        return Self::finish(session, result);
                    }
                }
                Self::finish(session, PluginRunningResult::GoodNext)
            }
        }
    }
}

#[async_trait]
impl RequestFilter for ExtensionRefFilter {
    fn name(&self) -> &str {
        "ExtensionRef"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        self.run_extension_async(PluginRunningStage::Request, session, log)
            .await
    }
}

impl UpstreamResponseFilter for ExtensionRefFilter {
    fn name(&self) -> &str {
        "ExtensionRef"
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        self.run_extension(PluginRunningStage::UpstreamResponseFilter, session, log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::edgion_plugins::get_global_plugin_store;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;
    use crate::types::resources::edgion_plugins::{EdgionPlugins, EdgionPluginsSpec, RequestFilterEntry};
    use crate::types::resources::gateway_api_plugins::EdgionPlugin;
    use crate::types::resources::http_route::HTTPHeaderFilter;
    use kube::core::ObjectMeta;
    use mockall::predicate::eq;
    use std::collections::HashMap;

    fn make_plugin(key_ns: &str, name: &str) {
        let mut ep = EdgionPlugins {
            metadata: ObjectMeta {
                namespace: Some(key_ns.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: EdgionPluginsSpec {
                request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
                    HTTPHeaderFilter {
                        set: None,
                        add: None,
                        remove: None,
                    },
                ))]),
                upstream_response_filter_plugins: None,
                upstream_response_plugins: None,
                plugin_runtime: Default::default(),
            },
            status: None,
        };
        ep.preparse();

        let mut map = HashMap::new();
        map.insert(format!("{}/{}", key_ns, name), ep);
        get_global_plugin_store().replace_all(map);
    }

    #[test]
    fn test_cycle_detected() {
        let mut session = MockPluginSession::new();
        session.expect_has_plugin_ref().with(eq("ns/p1")).return_const(true);
        let filter = ExtensionRefFilter::new(
            "ns".to_string(),
            LocalObjectReference {
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
            },
            DEFAULT_PLUGIN_REF_DEPTH,
        );
        let mut log = PluginLog::new("test");
        let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }

    #[test]
    fn test_depth_exceeded() {
        let mut session = MockPluginSession::new();
        session.expect_has_plugin_ref().with(eq("ns/p1")).return_const(false);
        session.expect_plugin_ref_depth().return_const(DEFAULT_PLUGIN_REF_DEPTH);
        let filter = ExtensionRefFilter::new(
            "ns".to_string(),
            LocalObjectReference {
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
            },
            DEFAULT_PLUGIN_REF_DEPTH,
        );
        let mut log = PluginLog::new("test");
        let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }

    #[test]
    fn test_depth_within_limit_runs() {
        use crate::core::plugins::plugin_runtime::log::EdgionPluginsLogToken;

        // Prepare store with plugin
        make_plugin("ns", "p1");

        let mut session = MockPluginSession::new();
        session.expect_has_plugin_ref().with(eq("ns/p1")).return_const(false);
        session.expect_plugin_ref_depth().return_const(0usize);
        session.expect_push_plugin_ref().return_const(());
        session.expect_pop_plugin_ref().return_const(());
        session
            .expect_start_edgion_plugins_log()
            .returning(|_| EdgionPluginsLogToken::new(0, 0));
        session.expect_push_to_edgion_plugins_log().return_const(());

        let filter = ExtensionRefFilter::new(
            "ns".to_string(),
            LocalObjectReference {
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
            },
            DEFAULT_PLUGIN_REF_DEPTH,
        );
        let mut log = PluginLog::new("test");
        let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
        assert_eq!(result, PluginRunningResult::GoodNext);
        // Verify refer_to is set (now just the name string)
        assert!(log.refer_to.is_some());
        assert_eq!(log.refer_to.as_ref().unwrap(), "p1");
    }
}
