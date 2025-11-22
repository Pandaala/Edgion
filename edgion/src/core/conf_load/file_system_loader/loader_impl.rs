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

    async fn load_base(&self, gateway_class_name: &str) -> Result<crate::core::conf_sync::GatewayBaseConf> {
        use crate::types::{GatewayClass, EdgionGatewayConfig, Gateway};
        use crate::core::conf_sync::GatewayBaseConf;

        tracing::info!(
            "Starting to load base configuration from filesystem for gateway_class_name: {}",
            gateway_class_name
        );

        let mut gateway_class: Option<GatewayClass> = None;
        let mut edgion_gateway_config_name: Option<String> = None;
        let mut edgion_gateway_config: Option<EdgionGatewayConfig> = None;
        let mut gateways: Vec<Gateway> = Vec::new();

        let root = self.root();
        let mut stack = vec![root.clone()];

        // First pass: find the matching GatewayClass and extract EdgionGatewayConfig name
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

                // Try to parse as GatewayClass and check if name matches
                if gateway_class.is_none() {
                    if let Ok(gc) = serde_yaml::from_str::<GatewayClass>(&content) {
                        if let Some(ref name) = gc.metadata.name {
                            if name == gateway_class_name {
                                tracing::info!("Found matching GatewayClass: {:?}", name);
                                // Extract EdgionGatewayConfig name from parameters_ref
                                if let Some(ref params_ref) = gc.spec.parameters_ref {
                                    if params_ref.kind == "EdgionGatewayConfig" {
                                        edgion_gateway_config_name = Some(params_ref.name.clone());
                                        tracing::info!(
                                            "GatewayClass references EdgionGatewayConfig: {}",
                                            params_ref.name
                                        );
                                    }
                                }
                                gateway_class = Some(gc);
                                continue;
                            }
                        }
                    }
                }
            }
        }

        // Validate GatewayClass was found
        let gc = gateway_class.ok_or_else(|| {
            anyhow!(
                "GatewayClass '{}' not found in configuration directory",
                gateway_class_name
            )
        })?;

        // Validate EdgionGatewayConfig name was found
        let egwc_name = edgion_gateway_config_name.ok_or_else(|| {
            anyhow!(
                "GatewayClass '{}' does not reference an EdgionGatewayConfig via parameters_ref",
                gateway_class_name
            )
        })?;

        // Second pass: find EdgionGatewayConfig and matching Gateways
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

                // Try to parse as EdgionGatewayConfig and check if name matches
                if edgion_gateway_config.is_none() {
                    if let Ok(egwc) = serde_yaml::from_str::<EdgionGatewayConfig>(&content) {
                        if let Some(ref name) = egwc.metadata.name {
                            if name == &egwc_name {
                                tracing::info!("Found matching EdgionGatewayConfig: {:?}", name);
                                edgion_gateway_config = Some(egwc);
                                continue;
                            }
                        }
                    }
                }

                // Try to parse as Gateway and check if gateway_class_name matches
                if let Ok(gw) = serde_yaml::from_str::<Gateway>(&content) {
                    if gw.spec.gateway_class_name == gateway_class_name {
                        tracing::info!(
                            "Found matching Gateway: {:?} (gateway_class_name: {})",
                            gw.metadata.name,
                            gateway_class_name
                        );
                        gateways.push(gw);
                    }
                }
            }
        }

        // Validate required resources exist
        let egwc = edgion_gateway_config.ok_or_else(|| {
            anyhow!(
                "EdgionGatewayConfig '{}' not found in configuration directory",
                egwc_name
            )
        })?;

        tracing::info!(
            "Successfully loaded base configuration: GatewayClass={:?}, EdgionGatewayConfig={:?}, Gateways count={}",
            gc.metadata.name,
            egwc.metadata.name,
            gateways.len()
        );

        Ok(GatewayBaseConf::new(gc, egwc, gateways))
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
