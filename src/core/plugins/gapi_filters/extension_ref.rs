//! ExtensionRef filter implementation
//!
//! This filter handles references to EdgionPlugins resources.

use async_trait::async_trait;

use crate::core::plugins::edgion_plugins::get_global_plugin_store;
use crate::core::plugins::plugin_runtime::log::PluginLog;
use crate::core::plugins::plugin_runtime::traits::{PluginSession, RequestFilter, UpstreamResponseFilter};
use crate::types::filters::{PluginRunningResult, PluginRunningStage};
use crate::types::resources::LocalObjectReference;

/// Maximum allowed nested plugin references to avoid infinite loops
const MAX_PLUGIN_REF_DEPTH: usize = 5;

/// Filter that handles ExtensionRef to EdgionPlugins
#[derive(Clone)]
pub struct ExtensionRefFilter {
    /// The namespace to look up the plugin (from the HTTPRoute's namespace)
    namespace: String,
    /// The extension reference configuration
    ext_ref: LocalObjectReference,
}

impl ExtensionRefFilter {
    pub fn new(namespace: String, ext_ref: LocalObjectReference) -> Self {
        Self { namespace, ext_ref }
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

        let key = self.plugin_key();
        let store = get_global_plugin_store();

        // Detect recursion and depth overflow
        if session.has_plugin_ref(&key) {
            log.push(&format!("Detected cyclic plugin reference '{}'", key));
            return PluginRunningResult::ErrTerminateRequest;
        }
        if session.plugin_ref_depth() >= MAX_PLUGIN_REF_DEPTH {
            log.push(&format!(
                "Plugin reference depth exceeded {} while resolving '{}'",
                MAX_PLUGIN_REF_DEPTH, key
            ));
            return PluginRunningResult::ErrTerminateRequest;
        }
        session.push_plugin_ref(key.clone());

        // Look up the EdgionPlugins in global store
        let Some(edgion_plugins) = store.get(&key) else {
            log.push(&format!("EdgionPlugins '{}' not found, returning 500", key));
            session.pop_plugin_ref();
            return PluginRunningResult::ErrTerminateRequest;
        };

        // Get the pre-compiled plugin runtime
        let plugin_runtime = &edgion_plugins.spec.plugin_runtime;

        // Run edgion_plugins based on stage
        let result = match stage {
            PluginRunningStage::Request => {
                // Request stage plugins are async, cannot be called in sync context
                // This is a design limitation - Request filters should only be called via run_extension_async
                log.push("Warning: Request stage should use async path");
                PluginRunningResult::GoodNext
            }
            PluginRunningStage::UpstreamResponseFilter => {
                // For sync response stage
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);

                    // Inner plugin log is handled separately

                    if result == PluginRunningResult::ErrTerminateRequest {
                        session.pop_plugin_ref();
                        return result;
                    }
                }
                PluginRunningResult::GoodNext
            }
            PluginRunningStage::UpstreamResponse => {
                // For async response stage - but we're in sync mode here
                // This case won't be hit in sync execution
                PluginRunningResult::GoodNext
            }
        };

        log.push(&format!("EdgionPlugins '{}' executed successfully", key));
        session.pop_plugin_ref();
        result
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

        let key = self.plugin_key();
        let store = get_global_plugin_store();

        if session.has_plugin_ref(&key) {
            log.push(&format!("Detected cyclic plugin reference '{}'", key));
            return PluginRunningResult::ErrTerminateRequest;
        }
        if session.plugin_ref_depth() >= MAX_PLUGIN_REF_DEPTH {
            log.push(&format!(
                "Plugin reference depth exceeded {} while resolving '{}'",
                MAX_PLUGIN_REF_DEPTH, key
            ));
            return PluginRunningResult::ErrTerminateRequest;
        }
        session.push_plugin_ref(key.clone());

        let Some(edgion_plugins) = store.get(&key) else {
            log.push(&format!("EdgionPlugins '{}' not found, returning 500", key));
            session.pop_plugin_ref();
            return PluginRunningResult::ErrTerminateRequest;
        };

        let plugin_runtime = &edgion_plugins.spec.plugin_runtime;

        let result = match stage {
            PluginRunningStage::Request => {
                for plugin in plugin_runtime.request_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_request(session, &mut inner_log).await;

                    // Inner plugin log is handled separately

                    if result == PluginRunningResult::ErrTerminateRequest {
                        session.pop_plugin_ref();
                        return result;
                    }
                }
                PluginRunningResult::GoodNext
            }
            PluginRunningStage::UpstreamResponse => {
                for plugin in plugin_runtime.upstream_response_async_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response(session, &mut inner_log).await;

                    // Inner plugin log is handled separately

                    if result == PluginRunningResult::ErrTerminateRequest {
                        session.pop_plugin_ref();
                        return result;
                    }
                }
                PluginRunningResult::GoodNext
            }
            PluginRunningStage::UpstreamResponseFilter => {
                // Sync stage - handled in run_sync
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);

                    // Inner plugin log is handled separately

                    if result == PluginRunningResult::ErrTerminateRequest {
                        session.pop_plugin_ref();
                        return result;
                    }
                }
                PluginRunningResult::GoodNext
            }
        };

        log.push(&format!("EdgionPlugins '{}' executed successfully", key));
        session.pop_plugin_ref();
        result
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
