//! ExtensionRef filter implementation
//!
//! This filter handles references to EdgionPlugins resources.

use async_trait::async_trait;

use crate::types::resources::LocalObjectReference;
use crate::types::filters::{PluginRunningResult, PluginRunningStage};
use crate::core::plugins::plugin_runtime::filters::{RequestFilter, PluginSession};
use crate::core::plugins::plugin_runtime::log::PluginLog;
use crate::core::plugins::edgion_plugins::get_global_plugin_store;

/// Filter that handles ExtensionRef to EdgionPlugins
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
        self.ext_ref.kind == "EdgionPlugins" 
            && (self.ext_ref.group.is_empty() || self.ext_ref.group == "edgion.io")
    }

    /// Execute the referenced EdgionPlugins's plugin runtime
    fn run_extension(
        &self,
        stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        if !self.is_edgion_plugins() {
            log.add_plugin_log(&format!(
                "ExtensionRef kind '{}' not supported, skipping",
                self.ext_ref.kind
            ));
            return PluginRunningResult::GoodNext;
        }

        let key = self.plugin_key();
        let store = get_global_plugin_store();

        // Look up the EdgionPlugins in global store
        let Some(edgion_plugins) = store.get(&key) else {
            log.add_plugin_log(&format!("EdgionPlugins '{}' not found, returning 500", key));
            return PluginRunningResult::ErrTerminateRequest;
        };

        // Get the pre-compiled plugin runtime
        let plugin_runtime = &edgion_plugins.spec.plugin_runtime;

        // Run edgion_plugins based on stage
        match stage {
            PluginRunningStage::Request => {
                // Request stage plugins are async, cannot be called in sync context
                // This is a design limitation - Request filters should only be called via run_extension_async
                log.add_plugin_log("Warning: Request stage should use async path");
            }
            PluginRunningStage::UpstreamResponseFilter => {
                // For sync response stage
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);
                    
                    if let Some(inner_log_str) = &inner_log.log {
                        log.add_plugin_log(inner_log_str);
                    }
                    
                    if result == PluginRunningResult::ErrTerminateRequest {
                        return result;
                    }
                }
            }
            PluginRunningStage::UpstreamResponse => {
                // For async response stage - but we're in sync mode here
                // This case won't be hit in sync execution
            }
        }

        log.add_plugin_log(&format!("EdgionPlugins '{}' executed successfully", key));
        PluginRunningResult::GoodNext
    }

    /// Async version for stages that require it
    async fn run_extension_async(
        &self,
        stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        if !self.is_edgion_plugins() {
            log.add_plugin_log(&format!(
                "ExtensionRef kind '{}' not supported, skipping",
                self.ext_ref.kind
            ));
            return PluginRunningResult::GoodNext;
        }

        let key = self.plugin_key();
        let store = get_global_plugin_store();

        let Some(edgion_plugins) = store.get(&key) else {
            log.add_plugin_log(&format!("EdgionPlugins '{}' not found, returning 500", key));
            return PluginRunningResult::ErrTerminateRequest;
        };

        let plugin_runtime = &edgion_plugins.spec.plugin_runtime;

        match stage {
            PluginRunningStage::Request => {
                for plugin in plugin_runtime.request_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_request(session, &mut inner_log).await;
                    
                    if let Some(inner_log_str) = &inner_log.log {
                        log.add_plugin_log(inner_log_str);
                    }
                    
                    if result == PluginRunningResult::ErrTerminateRequest {
                        return result;
                    }
                }
            }
            PluginRunningStage::UpstreamResponse => {
                for plugin in plugin_runtime.upstream_response_async_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response(session, &mut inner_log).await;
                    
                    if let Some(inner_log_str) = &inner_log.log {
                        log.add_plugin_log(inner_log_str);
                    }
                    
                    if result == PluginRunningResult::ErrTerminateRequest {
                        return result;
                    }
                }
            }
            PluginRunningStage::UpstreamResponseFilter => {
                // Sync stage - handled in run_sync
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);
                    
                    if let Some(inner_log_str) = &inner_log.log {
                        log.add_plugin_log(inner_log_str);
                    }
                    
                    if result == PluginRunningResult::ErrTerminateRequest {
                        return result;
                    }
                }
            }
        }

        log.add_plugin_log(&format!("EdgionPlugins '{}' executed successfully", key));
        PluginRunningResult::GoodNext
    }
}

#[async_trait]
impl RequestFilter for ExtensionRefFilter {
    fn name(&self) -> &str {
        "ExtensionRef"
    }

    async fn run_request(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        self.run_extension_async(PluginRunningStage::Request, session, log).await
    }
}

