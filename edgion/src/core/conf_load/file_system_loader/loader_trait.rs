use anyhow::{anyhow, Context, Result};
use tokio::fs;

use crate::core::conf_load::ConfigLoader;
use crate::core::utils::is_base_conf;
use crate::types::ResourceKind;

use super::loader::FileSystemConfigLoader;

#[async_trait::async_trait]
impl ConfigLoader for FileSystemConfigLoader {
    /// Connect to filesystem (no-op for filesystem loader)
    async fn connect(&self) -> Result<()> {
        // Filesystem doesn't need connection setup
        if !self.root().exists() {
            return Err(anyhow!("Config directory {:?} does not exist", self.root()));
        }
        Ok(())
    }

    /// Bootstrap and load base configuration resources (GatewayClass, EdgionGatewayConfig, Gateway)
    /// If kind is specified, only load resources of that kind
    async fn bootstrap_base_conf(&self, kind: Option<crate::types::ResourceKind>) -> Result<()> {
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
                    // Only process YAML files
                    if path.extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext == "yml" || ext == "yaml")
                        .unwrap_or(false)
                    {
                        let content = match FileSystemConfigLoader::read_file(&path).await {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(
                                    component = "file_system_loader",
                                    event = "failed to read file",
                                    path = ?path,
                                    error = %e,
                                );
                                continue;
                            }
                        };
                        
                        // Check if this file matches the kind filter
                        if let Some(target_kind) = kind {
                            if let Some(content_kind) = crate::types::ResourceKind::from_content(&content) {
                                if content_kind != target_kind {
                                    continue;
                                }
                                
                                // For EdgionGatewayConfig, check if it's referenced by GatewayClass
                                if content_kind == crate::types::ResourceKind::EdgionGatewayConfig {
                                    // Parse the config name from content
                                    if let Ok(config) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
                                        if let Some(name) = config.get("metadata")
                                            .and_then(|m| m.get("name"))
                                            .and_then(|n| n.as_str())
                                        {
                                            if !self.dispatcher().should_load_edgion_gateway_config(name) {
                                                tracing::debug!(
                                                    component = "file_system_loader",
                                                    event = "skip_config_not_referenced",
                                                    path = ?path,
                                                    config_name = name,
                                                    "Skipping EdgionGatewayConfig not referenced by GatewayClass parametersRef"
                                                );
                                                continue;
                                            }
                                        }
                                    }
                                }
                            } else {
                                continue;
                            }
                        }

                        self.process_init_file(&path, kind).await?;
                    }
                }
            }
        }

        Ok(())
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
                    if path.extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext == "yml" || ext == "yaml")
                        .unwrap_or(false)
                    {
                        let content = match FileSystemConfigLoader::read_file(&path).await {
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
                        
                        if !is_base_conf(&content) {
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
}

