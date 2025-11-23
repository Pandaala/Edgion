use anyhow::{anyhow, Context, Result};
use tokio::fs;
use std::sync::Arc;

use crate::core::conf_load::ConfigLoader;
use crate::core::conf_sync::traits::ConfigServerEventDispatcher;
use crate::types::ResourceKind;

use super::loader::LocalPathLoader;

#[async_trait::async_trait]
impl ConfigLoader for LocalPathLoader {
    /// Register a dispatcher for handling configuration events
    async fn register_dispatcher(&self, dispatcher: Arc<dyn ConfigServerEventDispatcher>) {
        self.register_dispatcher(dispatcher).await;
    }

    /// Connect to localpath (no-op for localpath loader)
    async fn connect(&self) -> Result<()> {
        // LOCAL_PATH doesn't need connection setup
        if !self.root().exists() {
            return Err(anyhow!("Config directory {:?} does not exist", self.root()));
        }
        Ok(())
    }

    async fn load_base(&self, gateway_class_name: &str) -> Result<crate::types::GatewayBaseConf> {
        use crate::types::{GatewayClass, EdgionGatewayConfig, Gateway, GatewayBaseConf};

        tracing::info!(
            "Starting to load base configuration from filesystem for gateway_class_name: {}",
            gateway_class_name
        );

        // Step 1: Collect all base resources in one pass
        let mut gateway_classes: Vec<GatewayClass> = Vec::new();
        let mut edgion_gateway_configs: Vec<EdgionGatewayConfig> = Vec::new();
        let mut gateways: Vec<Gateway> = Vec::new();

        let root = self.root();
        let mut stack = vec![root.clone()];

        // Traverse all files and collect resources
        while let Some(dir) = stack.pop() {
            let mut entries = fs::read_dir(&dir)
                .await
                .with_context(|| format!("Failed to read directory {:?}", dir))?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();

                if path.is_dir() {
                    stack.push(path);
                    continue;
                }

                // Only process yaml/yml/json files
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy();
                    if !matches!(ext_str.as_ref(), "yaml" | "yml" | "json") {
                        continue;
                    }
                } else {
                    continue;
                }

                // Read and parse file
                let content = match LocalPathLoader::read_file(&path).await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("Failed to read file {:?}: {}", path, e);
                        continue;
                    }
                };

                // First, determine the resource kind from content
                use crate::types::ResourceKind;
                let resource_kind = ResourceKind::from_content(&content);

                // Parse based on resource kind to avoid false matches
                match resource_kind {
                    Some(ResourceKind::GatewayClass) => {
                        match serde_yaml::from_str::<GatewayClass>(&content) {
                            Ok(gc) => {
                                tracing::debug!("Found GatewayClass: {:?} in file {:?}", gc.metadata.name, path);
                                gateway_classes.push(gc);
                            }
                            Err(e) => {
                                tracing::warn!("Failed to parse GatewayClass from file {:?}: {}", path, e);
                            }
                        }
                    }
                    Some(ResourceKind::EdgionGatewayConfig) => {
                        match serde_yaml::from_str::<EdgionGatewayConfig>(&content) {
                            Ok(egwc) => {
                                tracing::debug!("Found EdgionGatewayConfig: {:?} in file {:?}", egwc.metadata.name, path);
                                edgion_gateway_configs.push(egwc);
                            }
                            Err(e) => {
                                tracing::warn!("Failed to parse EdgionGatewayConfig from file {:?}: {}", path, e);
                            }
                        }
                    }
                    Some(ResourceKind::Gateway) => {
                        match serde_yaml::from_str::<Gateway>(&content) {
                            Ok(gw) => {
                                tracing::debug!(
                                    "Found Gateway: {:?} with gateway_class_name: {} in file {:?}",
                                    gw.metadata.name,
                                    gw.spec.gateway_class_name,
                                    path
                                );
                    gateways.push(gw);
                }
                            Err(e) => {
                                tracing::warn!("Failed to parse Gateway from file {:?}: {}", path, e);
                            }
                        }
                    }
                    Some(_) => {
                        // Other resource types (HTTPRoute, Service, etc.) - skip for base conf
                        tracing::trace!("Skipping non-base-conf resource in file {:?}", path);
                    }
                    None => {
                        // Could not determine resource kind - try fallback parsing
                        // This handles edge cases where kind might not be easily extractable
                        tracing::trace!("Could not determine resource kind from file {:?}, attempting fallback parsing", path);
                    }
                }
            }
        }

        tracing::info!(
            "Collected resources: {} GatewayClasses, {} EdgionGatewayConfigs, {} Gateways",
            gateway_classes.len(),
            edgion_gateway_configs.len(),
            gateways.len()
        );

        // Log all collected Gateway names and their gateway_class_name for debugging
        if !gateways.is_empty() {
            tracing::info!("All collected Gateways:");
            for gw in &gateways {
                tracing::info!(
                    "  - Gateway: {:?} (namespace: {:?}), gateway_class_name: {}",
                    gw.metadata.name,
                    gw.metadata.namespace,
                    gw.spec.gateway_class_name
                );
            }
        } else {
            tracing::warn!("No Gateways were collected from filesystem!");
        }

        // Step 2: Find the matching GatewayClass by name
        let gateway_class = gateway_classes
            .into_iter()
            .find(|gc| gc.metadata.name.as_ref().map(|n| n == gateway_class_name).unwrap_or(false))
            .ok_or_else(|| {
                anyhow!(
                    "GatewayClass '{}' not found in configuration directory",
                    gateway_class_name
                )
            })?;

        tracing::info!("Found matching GatewayClass: {:?}", gateway_class.metadata.name);

        // Step 3: Extract EdgionGatewayConfig name from GatewayClass parameters_ref
        let egwc_name = gateway_class
            .spec
            .parameters_ref
            .as_ref()
            .and_then(|params_ref| {
                if params_ref.kind == "EdgionGatewayConfig" {
                    Some(params_ref.name.clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                anyhow!(
                    "GatewayClass '{}' does not reference an EdgionGatewayConfig via parameters_ref",
                    gateway_class_name
                )
            })?;

        tracing::info!(
            "GatewayClass references EdgionGatewayConfig: {}",
            egwc_name
        );

        // Step 4: Find the matching EdgionGatewayConfig by name
        let edgion_gateway_config = edgion_gateway_configs
            .into_iter()
            .find(|egwc| egwc.metadata.name.as_ref().map(|n| n == &egwc_name).unwrap_or(false))
            .ok_or_else(|| {
                anyhow!(
                    "EdgionGatewayConfig '{}' not found in configuration directory",
                    egwc_name
                )
            })?;

        tracing::info!("Found matching EdgionGatewayConfig: {:?}", edgion_gateway_config.metadata.name);

        // Step 5: Filter Gateways by gateway_class_name
        let matching_gateways: Vec<Gateway> = gateways
            .into_iter()
            .filter(|gw| {
                let matches = gw.spec.gateway_class_name == gateway_class_name;
                if !matches {
                    tracing::debug!(
                        "Gateway {:?} (gateway_class_name: {}) does not match target: {}",
                        gw.metadata.name,
                        gw.spec.gateway_class_name,
                        gateway_class_name
                    );
                }
                matches
            })
            .collect();

        if matching_gateways.is_empty() {
            tracing::error!(
                "No matching Gateways found for gateway_class_name: {}. Please check that Gateway resources have spec.gatewayClassName set correctly.",
                gateway_class_name
            );
        } else {
            tracing::info!(
                "Found {} matching Gateways for gateway_class_name: {}",
                matching_gateways.len(),
                gateway_class_name
            );
            for gw in &matching_gateways {
                tracing::info!(
                    "  - Matching Gateway: {:?} (namespace: {:?})",
                    gw.metadata.name,
                    gw.metadata.namespace
                );
            }
        }

        tracing::info!(
            "Successfully loaded base configuration: GatewayClass={:?}, EdgionGatewayConfig={:?}, Gateways count={}",
            gateway_class.metadata.name,
            edgion_gateway_config.metadata.name,
            matching_gateways.len()
        );

        Ok(GatewayBaseConf::new(gateway_class, edgion_gateway_config, matching_gateways))
    }

    /// Bootstrap and load user configuration resources (all other resources)
    async fn bootstrap_user_conf(&self) -> Result<()> {
        let root = self.root();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let mut entries = fs::read_dir(&dir)
                .await
                .with_context(|| format!("Failed to read directory {:?}", dir))?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    // Only process non-base conf files
                    if path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext == "yml" || ext == "yaml")
                        .unwrap_or(false)
                    {
                        let content = match LocalPathLoader::read_file(&path).await {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(
                                    component = "file_system_loader",
                                    event = "failed to read user conf",
                                    path = ?path,
                                    error = %e,
                                );
                                continue;
                            }
                        };

                        // Only process non-base-conf resources
                        let is_base_conf = if let Some(kind) = ResourceKind::from_content(&content) {
                            matches!(
                                kind,
                                ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway
                            )
                        } else {
                            false
                        };

                        if !is_base_conf {
                            self.process_init_file(&path, None).await?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Set ready state after initialization
    async fn set_ready(&self) {
        if let Some(dispatcher) = self.dispatcher().await {
            dispatcher.set_ready();
        } else {
            tracing::warn!(
                component = "file_system_loader",
                event = "dispatcher_not_registered",
                "Dispatcher not registered, cannot set ready state"
            );
        }
    }

    /// Main run loop for watching configuration changes
    async fn run(&self) -> Result<()> {
        // Delegate to the internal run method
        self.run_watcher().await
    }

    async fn set_enable_resource_version_fix(&self) {
        if self.enable_resource_version_fix {
            if let Some(dispatcher) = self.dispatcher().await {
                dispatcher.enable_version_fix_mode();
            } else {
                tracing::warn!(
                    component = "file_system_loader",
                    event = "dispatcher_not_registered",
                    "Dispatcher not registered, cannot enable version fix mode"
                );
            }
        }
    }
}
