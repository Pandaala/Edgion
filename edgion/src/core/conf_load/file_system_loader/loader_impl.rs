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

    /// Bootstrap and load base configuration resources (GatewayClass, EdgionGatewayConfig, Gateway)
    /// If kind is specified, only load resources of that kind
    async fn bootstrap_base_conf(&self, kind: Option<ResourceKind>) -> Result<()> {
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
                                    event = "failed to read file",
                                    path = ?path,
                                    error = %e,
                                );
                                continue;
                            }
                        };

                        // Check if this file matches the kind filter
                        if let Some(target_kind) = kind {
                            if let Some(content_kind) =
                                crate::types::ResourceKind::from_content(&content)
                            {
                                if content_kind != target_kind {
                                    continue;
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
                        let is_base_conf = if let Some(kind) = ResourceKind::from_content(&content)
                        {
                            matches!(
                                kind,
                                ResourceKind::GatewayClass
                                    | ResourceKind::EdgionGatewayConfig
                                    | ResourceKind::Gateway
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
