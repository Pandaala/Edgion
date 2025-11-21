use anyhow::{anyhow, Context, Result};
use tokio::fs;

use crate::core::conf_load::ConfigLoader;
use crate::types::ResourceKind;

use super::loader::LocalPathLoader;

#[async_trait::async_trait]
impl ConfigLoader for LocalPathLoader {
    /// Connect to localpath (no-op for localpath loader)
    async fn connect(&self) -> Result<()> {
        // LOCAL_PATH doesn't need connection setup
        if !self.root().exists() {
            return Err(anyhow!("Config directory {:?} does not exist", self.root()));
        }
        Ok(())
    }

    async fn load_base(&self) -> Result<crate::core::conf_sync::GatewayBaseConf> {
        use crate::types::{GatewayClass, EdgionGatewayConfig, Gateway};
        use crate::core::conf_sync::GatewayBaseConf;

        tracing::info!("Starting to load base configuration from filesystem");

        let mut gateway_class: Option<GatewayClass> = None;
        let mut edgion_gateway_config: Option<EdgionGatewayConfig> = None;
        let mut gateways: Vec<Gateway> = Vec::new();

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

                // Try to parse as GatewayClass
                if gateway_class.is_none() {
                    if let Ok(gc) = serde_yaml::from_str::<GatewayClass>(&content) {
                        tracing::info!("Found GatewayClass: {:?}", gc.metadata.name);
                        gateway_class = Some(gc);
                        continue;
                    }
                }

                // Try to parse as EdgionGatewayConfig
                if edgion_gateway_config.is_none() {
                    if let Ok(egwc) = serde_yaml::from_str::<EdgionGatewayConfig>(&content) {
                        tracing::info!("Found EdgionGatewayConfig: {:?}", egwc.metadata.name);
                        edgion_gateway_config = Some(egwc);
                        continue;
                    }
                }

                // Try to parse as Gateway
                if let Ok(gw) = serde_yaml::from_str::<Gateway>(&content) {
                    tracing::info!("Found Gateway: {:?}", gw.metadata.name);
                    gateways.push(gw);
                }
            }
        }

        // Validate required resources exist
        let gc = gateway_class.ok_or_else(|| anyhow!("GatewayClass not found in configuration directory"))?;
        let egwc = edgion_gateway_config.ok_or_else(|| anyhow!("EdgionGatewayConfig not found in configuration directory"))?;

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
        self.dispatcher().set_ready();
    }

    /// Main run loop for watching configuration changes
    async fn run(&self) -> Result<()> {
        // Delegate to the internal run method
        self.run_watcher().await
    }

    fn set_enable_resource_version_fix(&self) {
        if self.enable_resource_version_fix {
            self.dispatcher().enable_version_fix_mode()
        }
    }
}
