use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use notify::{event::ModifyKind, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::fs;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use crate::core::conf_load::ConfigLoader;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::EventDispatcher;
use crate::types::ResourceKind;

pub struct FileSystemConfigLoader {
    root: PathBuf,
    dispatcher: Arc<dyn EventDispatcher>,
    resource_kind: Option<ResourceKind>,
    cache: Arc<Mutex<HashMap<PathBuf, String>>>,
}

// TODO: Support nested directory watch and propagation. Currently only flat file
// updates inside the root directory are handled; directory-level operations are
// ignored with an error log.
impl FileSystemConfigLoader {
    pub fn new<P: Into<PathBuf>>(
        root: P,
        dispatcher: Arc<dyn EventDispatcher>,
        resource_kind: Option<ResourceKind>,
    ) -> Arc<Self> {
        Arc::new(Self {
            root: root.into(),
            dispatcher,
            resource_kind,
            cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn spawn(self: Arc<Self>) -> JoinHandle<()> {
        let root = self.root.clone();
        tokio::spawn(async move {
            if let Err(err) = self.run().await {
                eprintln!(
                    "[FileSystemConfigLoader] watcher exited with error for {:?}: {}",
                    root, err
                );
            }
        })
    }

    async fn dispatch_change(&self, change: ResourceChange, data: String) {
        let resource_type = self.resource_kind;
        
        // Convert YAML to JSON for dispatcher
        let json_data = match Self::yaml_to_json(&data) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(
                    "Failed to convert YAML to JSON: {}, skipping file",
                    e
                );
                return;
            }
        };
        
        self.dispatcher
            .apply_resource_change(change, resource_type, json_data, None);
    }
    
    fn yaml_to_json(yaml_str: &str) -> Result<String> {
        let value: serde_yaml::Value = serde_yaml::from_str(yaml_str)?;
        let json_str = serde_json::to_string(&value)?;
        Ok(json_str)
    }

    async fn read_file(path: &Path) -> Result<String> {
        let content = fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read file {:?}", path))?;
        Ok(content)
    }

    async fn process_new_file(&self, path: &Path) -> Result<()> {
        self.process_file_with_change(path, ResourceChange::EventAdd)
            .await
    }

    async fn process_init_file(&self, path: &Path) -> Result<()> {
        self.process_file_with_change(path, ResourceChange::InitAdd)
            .await
    }

    async fn process_file_with_change(
        &self,
        path: &Path,
        change: ResourceChange,
    ) -> Result<()> {

        tracing::info!(
            component = "file_system_config_loader",
            event = "process_file_with_change",
            path = ?path,
            change = ?change,
            "Processing file with change"
        );

        if path.is_dir() {
            tracing::warn!(
                component = "file_system_config_loader",
                event = "directory_not_supported",
                path = ?path,
                "Directory changes are not supported"
            );
            log_directory_not_supported(path);
            return Ok(());
        }

        if !path.is_file() {
            tracing::warn!(
                component = "file_system_config_loader",
                event = "not_a_file",
                path = ?path,
                "Not a file"
            );
            return Ok(());
        }

        let content = Self::read_file(path).await?;
        self.cache
            .lock()
            .await
            .insert(path.to_path_buf(), content.clone());
        self.dispatch_change(change, content).await;
        Ok(())
    }

    async fn process_removed_file(&self, path: &Path) -> Result<()> {
        let mut cache = self.cache.lock().await;
        if let Some(old) = cache.remove(path) {
            drop(cache);
            self.dispatch_change(ResourceChange::EventDelete, old).await;
        } else {
            let has_children = cache.keys().any(|entry| entry.starts_with(path));
            drop(cache);
            if has_children {
                log_directory_not_supported(path);
            }
        }
        Ok(())
    }

    async fn process_updated_file(&self, path: &Path) -> Result<()> {
        if path.is_dir() {
            log_directory_not_supported(path);
            return Ok(());
        }

        if !path.is_file() {
            return Ok(());
        }

        let new_content = Self::read_file(path).await?;
        let mut cache = self.cache.lock().await;
        if let Some(old) = cache.remove(path) {
            drop(cache);
            self.dispatch_change(ResourceChange::EventDelete, old).await;
        }
        let mut cache = self.cache.lock().await;
        cache.insert(path.to_path_buf(), new_content.clone());
        drop(cache);
        self.dispatch_change(ResourceChange::EventAdd, new_content)
            .await;
        Ok(())
    }

    async fn handle_event(&self, event: Event) -> Result<()> {
        match event.kind {
            EventKind::Create(_) => {
                for path in event.paths {
                    self.process_new_file(&path).await?;
                }
            }
            EventKind::Modify(modify_kind) => match modify_kind {
                ModifyKind::Data(_) | ModifyKind::Metadata(_) => {
                    for path in event.paths {
                        self.process_updated_file(&path).await?;
                    }
                }
                ModifyKind::Name(_) => {
                    if event.paths.len() == 2 {
                        let old = event.paths[0].clone();
                        let new = event.paths[1].clone();
                        self.process_removed_file(&old).await?;
                        self.process_new_file(&new).await?;
                    } else {
                        for path in event.paths {
                            self.process_updated_file(&path).await?;
                        }
                    }
                }
                _ => {}
            },
            EventKind::Remove(_) => {
                for path in event.paths {
                    self.process_removed_file(&path).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn log_directory_not_supported(path: &Path) {
    eprintln!(
        "[FileSystemConfigLoader] directory changes are not supported: {:?}",
        path
    );
}

#[async_trait::async_trait]
impl ConfigLoader for FileSystemConfigLoader {
    /// Connect to filesystem (no-op for filesystem loader)
    async fn connect(&self) -> Result<()> {
        // Filesystem doesn't need connection setup
        if !self.root.exists() {
            return Err(anyhow!("Config directory {:?} does not exist", self.root));
        }
        Ok(())
    }

    /// Bootstrap and load all existing configuration files
    async fn bootstrap_existing(&self) -> Result<()> {
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let mut entries = fs::read_dir(&dir)
                .await
                .with_context(|| format!("Failed to read directory {:?}", dir))?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    // Use InitAdd for bootstrap phase
                    self.process_init_file(&path).await?;
                }
            }
        }

        Ok(())
    }

    /// Set ready state after initialization
    async fn set_ready(&self) {
        self.dispatcher.set_ready();
    }

    /// Main run loop for watching configuration changes
    async fn run(&self) -> Result<()> {
        // Start watching for changes
        let (tx, mut rx) = mpsc::channel::<Result<Event>>(128);
        let tx_watch = tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                let _ = tx_watch.blocking_send(res.map_err(|e| anyhow!(e)));
            },
            notify::Config::default(),
        )?;

        watcher
            .watch(self.root.as_path(), RecursiveMode::Recursive)
            .with_context(|| format!("Failed to watch directory {:?}", self.root))?;

        while let Some(event) = rx.recv().await {
            match event {
                Ok(event) => {
                    if let Err(err) = self.handle_event(event).await {
                        eprintln!(
                            "[FileSystemConfigLoader] failed to handle event in {:?}: {}",
                            self.root, err
                        );
                    }
                }
                Err(err) => {
                    eprintln!(
                        "[FileSystemConfigLoader] watcher error in {:?}: {}",
                        self.root, err
                    );
                }
            }
        }

        Ok(())
    }
}
