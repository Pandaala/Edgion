use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use notify::{event::{ModifyKind, RenameMode}, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::fs;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};

use crate::core::conf_load::ConfigLoader;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::EventDispatcher;
use crate::core::utils::{extract_resource_metadata, is_base_conf, ResourceMetadata};

#[derive(Clone)]
struct FileInfo {
    metadata: ResourceMetadata,
    content: String,
}

pub struct FileSystemConfigLoader {
    root: PathBuf,
    dispatcher: Arc<dyn EventDispatcher>,
    // Track resource metadata to file paths mapping for duplicate detection
    // Key: ResourceMetadata (kind/namespace/name), Value: Vec<PathBuf> (file paths)
    resource_to_files: Arc<Mutex<HashMap<ResourceMetadata, Vec<PathBuf>>>>,
    files_to_resource: Arc<RwLock<HashMap<PathBuf, FileInfo>>>,
}

// TODO: Support nested directory watch and propagation. Currently only flat file
// updates inside the root directory are handled; directory-level operations are
// ignored with an error log.
impl FileSystemConfigLoader {
    pub fn new<P: Into<PathBuf>>(
        root: P,
        dispatcher: Arc<dyn EventDispatcher>,
    ) -> Arc<Self> {
        let loader = Arc::new(Self {
            root: root.into(),
            dispatcher,
            resource_to_files: Arc::new(Mutex::new(HashMap::new())),
            files_to_resource: Arc::new(RwLock::new(HashMap::new())),
        });
        
        // Spawn duplicate detection task
        let loader_clone = loader.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                loader_clone.check_duplicate_resources().await;
            }
        });
        
        loader
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

    async fn dispatch_change(&self, change: ResourceChange, data: String, use_base_conf: bool) {
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
        
        if use_base_conf {
            self.dispatcher
                .apply_base_conf(change, None, json_data, None);
        } else {
            self.dispatcher
                .apply_resource_change(change, None, json_data, None);
        }
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
    
    /// Normalize a path to its absolute canonical form
    /// This ensures that relative and absolute paths pointing to the same file are treated as identical
    async fn normalize_path(&self, path: &Path) -> PathBuf {
        // Try to canonicalize first (resolves symlinks and makes absolute)
        if let Ok(canonical) = fs::canonicalize(path).await {
            return canonical;
        }
        
        // If canonicalize fails (e.g., file doesn't exist), try to make it absolute
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            // For relative paths, resolve relative to the loader's root directory
            // This is more reliable than using current working directory
            let root_canonical = fs::canonicalize(&self.root).await.ok();
            if let Some(root) = root_canonical {
                root.join(path)
            } else {
                // Fallback: try current working directory
                std::env::current_dir()
                    .ok()
                    .map(|cwd| cwd.join(path))
                    .unwrap_or_else(|| path.to_path_buf())
            }
        }
    }

    async fn process_new_file(&self, path: &Path) -> Result<()> {
        self.process_file(path).await
    }

    /// 读取文件并存储到内部映射
    /// 返回: (metadata, content, existed_before)
    async fn load_and_store_file(&self, path: &Path) -> Result<Option<(ResourceMetadata, String, bool)>> {
        // 只处理 .yml 或 .yaml 文件
        let extension = path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        if extension != "yml" && extension != "yaml" {
            return Ok(None);
        }

        if !path.exists() || !path.is_file() {
            return Ok(None);
        }

        let normalized_path = self.normalize_path(path).await;
        let content = Self::read_file(path).await?;
        
        let metadata = match extract_resource_metadata(&content) {
            Some(m) => m,
            None => {
                tracing::warn!(
                    component = "file_system_loader",
                    event = "failed_to_extract_metadata",
                    path = ?path,
                    "Failed to extract resource metadata from file"
                );
                return Ok(None);
            }
        };

        // 检查 files_to_resource 中是否已存在该文件
        let mut files_to_resource = self.files_to_resource.write().await;
        let existed_before = files_to_resource.contains_key(&normalized_path);
        
        // 更新 files_to_resource,存储 metadata 和 content
        let file_info = FileInfo {
            metadata: metadata.clone(),
            content: content.clone(),
        };
        files_to_resource.insert(normalized_path.clone(), file_info);
        drop(files_to_resource);

        // 更新 resource_to_files
        let mut resource_to_files = self.resource_to_files.lock().await;
        let files = resource_to_files.entry(metadata.clone()).or_insert_with(Vec::new);
        if !files.contains(&normalized_path) {
            files.push(normalized_path.clone());
        }
        drop(resource_to_files);

        Ok(Some((metadata, content, existed_before)))
    }

    /// 处理初始化阶段的文件加载，固定使用 InitAdd
    async fn process_init_file(&self, path: &Path) -> Result<()> {
        let result = self.load_and_store_file(path).await?;
        
        let Some((metadata, content, _existed_before)) = result else {
            return Ok(());
        };

        tracing::info!(
            component = "file_system_loader",
            event = "init_file",
            path = ?path,
            kind = ?metadata.kind,
            namespace = ?metadata.namespace,
            name = ?metadata.name,
            "Loading file during initialization"
        );

        // 使用 InitAdd
        let use_base_conf = is_base_conf(&content);
        self.dispatch_change(ResourceChange::InitAdd, content, use_base_conf).await;
        Ok(())
    }

    /// 处理文件变化，自动判断是 add/update/delete
    async fn process_file(
        &self,
        path: &Path,
    ) -> Result<()> {

        tracing::debug!(
            component = "file_system_loader",
            path = ?path,
            "Processing file"
        );

        // Normalize path for consistent tracking
        let normalized_path = self.normalize_path(path).await;

        // 判断文件是否存在
        if !path.exists() || path.is_dir() || !path.is_file() {
            // 文件不存在,处理删除逻辑
            tracing::info!(
                component = "file_system_loader",
                event = "file_not_exists",
                path = ?path,
                "File does not exist, processing as deletion"
            );

            // 从 files_to_resource 中获取旧的文件信息
            let mut files_to_resource = self.files_to_resource.write().await;
            let old_file_info = files_to_resource.remove(&normalized_path);
            drop(files_to_resource);

            if let Some(file_info) = old_file_info {
                let metadata = file_info.metadata.clone();
                let content = file_info.content.clone();
                
                // 从 resource_to_files 中删除该文件
                let mut resource_to_files = self.resource_to_files.lock().await;
                
                if let Some(files) = resource_to_files.get_mut(&metadata) {
                    files.retain(|p| p != &normalized_path);
                    
                    // 如果 vec 为空,触发 delete 事件
                    if files.is_empty() {
                        resource_to_files.remove(&metadata);
                        drop(resource_to_files);
                        
                        tracing::info!(
                            component = "file_system_loader",
                            event = "resource_deleted",
                            path = ?path,
                            kind = ?metadata.kind,
                            namespace = ?metadata.namespace,
                            name = ?metadata.name,
                            "Resource has no more files, triggering delete event"
                        );
                        
                        // 触发 delete 事件
                        let use_base_conf = is_base_conf(&content);
                        self.dispatch_change(ResourceChange::EventDelete, content, use_base_conf).await;
                    } else {
                        let remaining_count = files.len();
                        drop(resource_to_files);
                        tracing::info!(
                            component = "file_system_loader",
                            event = "file_removed_but_resource_still_exists",
                            path = ?path,
                            remaining_files = remaining_count,
                            "File removed but resource still has other files"
                        );
                    }
                } else {
                    drop(resource_to_files);
                }
            }
            
            return Ok(());
        }

        // 尝试读取并存储文件
        let result = self.load_and_store_file(path).await?;
        
        let Some((metadata, content, existed_before)) = result else {
            // 文件不存在或不是有效的 yaml 文件
            return Ok(());
        };

        // 根据文件之前是否存在，自动判断触发 add 还是 update
        let change = if existed_before {
            ResourceChange::EventUpdate
        } else {
            ResourceChange::EventAdd
        };

        let kind_str = metadata.kind.as_deref().unwrap_or("Unknown");
        let name_str = metadata.name.as_deref().unwrap_or("Unknown");
        let namespace_str = metadata.namespace.as_deref();
        
        if let Some(ns) = namespace_str {
            tracing::info!(
                component = "file_system_loader",
                event = "file_change",
                path = ?path,
                change = ?change,
                kind = kind_str,
                namespace = ns,
                name = name_str,
                existed_before = existed_before,
                "Processing resource file change"
            );
        } else {
            tracing::info!(
                component = "file_system_loader",
                event = "file_change",
                path = ?path,
                change = ?change,
                kind = kind_str,
                name = name_str,
                existed_before = existed_before,
                "Processing cluster-scoped resource file change"
            );
        }
        
        // Determine if this is a base conf resource
        let use_base_conf = is_base_conf(&content);
        self.dispatch_change(change, content, use_base_conf).await;
        Ok(())
    }

    async fn process_removed_file(&self, path: &Path) -> Result<()> {
        // 直接调用 process_file,由它自动判断(文件不存在会触发 delete)
        self.process_file(path).await
    }

    async fn process_updated_file(&self, path: &Path) -> Result<()> {
        // 直接调用 process_file,由它自动判断(文件存在会触发 update 或 add)
        self.process_file(path).await
    }
    
    /// Check for duplicate resources (multiple files pointing to the same resource)
    async fn check_duplicate_resources(&self) {
        let resource_to_files = self.resource_to_files.lock().await;
        
        for (metadata, files) in resource_to_files.iter() {
            if files.len() > 1 {
                let kind_str = metadata.kind.as_deref().unwrap_or("Unknown");
                let name_str = metadata.name.as_deref().unwrap_or("Unknown");
                let namespace_str = metadata.namespace.as_deref();
                
                let files_str: Vec<String> = files.iter()
                    .map(|p| p.display().to_string())
                    .collect();
                
                if let Some(ns) = namespace_str {
                    tracing::error!(
                        component = "file_system_loader",
                        event = "duplicate_resource_detected",
                        kind = kind_str,
                        namespace = ns,
                        name = name_str,
                        file_count = files.len(),
                        files = ?files_str,
                        "Multiple files define the same resource: kind={}, namespace={}, name={}. Files: {:?}",
                        kind_str, ns, name_str, files_str
                    );
                } else {
                    tracing::error!(
                        component = "file_system_loader",
                        event = "duplicate_resource_detected",
                        kind = kind_str,
                        name = name_str,
                        file_count = files.len(),
                        files = ?files_str,
                        "Multiple files define the same cluster-scoped resource: kind={}, name={}. Files: {:?}",
                        kind_str, name_str, files_str
                    );
                }
            }
        }
    }

    async fn handle_event(&self, event: Event) -> Result<()> {
        match event.kind {
            EventKind::Create(_) => {
                // 新文件创建
                for path in event.paths.clone() {
                    self.process_new_file(&path).await?;
                }
            }
            EventKind::Modify(modify_kind) => match modify_kind {
                ModifyKind::Data(_) | ModifyKind::Metadata(_) => {
                    // 文件内容或元数据修改
                    for path in event.paths.clone() {
                        self.process_updated_file(&path).await?;
                    }
                }
                ModifyKind::Name(rename_mode) => {
                    // 文件重命名
                    let paths = event.paths.clone();
                    match rename_mode {
                        RenameMode::Both => {
                            // 同时提供旧路径和新路径
                            if paths.len() == 2 {
                                let old = paths[0].clone();
                                let new = paths[1].clone();
                                self.process_removed_file(&old).await?;
                                self.process_new_file(&new).await?;
                            } else {
                                tracing::warn!(
                                    component = "file_system_loader",
                                    event = "unexpected_rename_paths",
                                    path_count = paths.len(),
                                    "Expected 2 paths for RenameMode::Both, got {}",
                                    paths.len()
                                );
                            }
                        }
                        RenameMode::From => {
                            // 旧路径(文件被移走)
                            for from_path in paths.clone() {
                                tracing::debug!(
                                    component = "file_system_loader",
                                    event = "rename_from",
                                    path = ?from_path,
                                    "Received RenameMode::From event"
                                );
                                self.process_removed_file(&from_path).await?;
                            }
                        }
                        RenameMode::To => {
                            // 新路径(文件被移入)
                            for to_path in paths.clone() {
                                tracing::debug!(
                                    component = "file_system_loader",
                                    event = "rename_to",
                                    path = ?to_path,
                                    "Received RenameMode::To event"
                                );
                                self.process_new_file(&to_path).await?;
                            }
                        }
                        RenameMode::Any => {
                            // 通用处理
                            let path_count = paths.len();
                            if path_count == 2 {
                                let old = paths[0].clone();
                                let new = paths[1].clone();
                                self.process_removed_file(&old).await?;
                                self.process_new_file(&new).await?;
                            } else if path_count == 1 {
                                // 单个路径,检查文件是否存在来决定是添加还是删除
                                for path in paths {
                                    let normalized_path = self.normalize_path(&path).await;
                                    let files_to_resource = self.files_to_resource.read().await;
                                    let was_tracked = files_to_resource.contains_key(&normalized_path);
                                    drop(files_to_resource);
                                    
                                    let is_in_watched_dir = path.starts_with(&self.root);
                                    
                                    if path.exists() && is_in_watched_dir {
                                        // 文件存在,判断是新增还是更新
                                        if was_tracked {
                                            self.process_updated_file(&path).await?;
                                        } else {
                                            self.process_new_file(&path).await?;
                                        }
                                    } else if was_tracked {
                                        // 文件不存在但之前被跟踪,处理为删除
                                        self.process_removed_file(&path).await?;
                                    }
                                }
                            } else {
                                tracing::warn!(
                                    component = "file_system_loader",
                                    event = "unexpected_rename_any",
                                    path_count = path_count,
                                    "RenameMode::Any with {} paths, skipping",
                                    path_count
                                );
                            }
                        }
                        RenameMode::Other => {
                            // 未知重命名模式
                            tracing::warn!(
                                component = "file_system_loader",
                                event = "unknown_rename_mode",
                                path_count = paths.len(),
                                "Unknown rename mode, attempting to handle"
                            );
                            if paths.len() == 2 {
                                let old = paths[0].clone();
                                let new = paths[1].clone();
                                self.process_removed_file(&old).await?;
                                self.process_new_file(&new).await?;
                            } else {
                                for path in paths {
                                    self.process_updated_file(&path).await?;
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            EventKind::Remove(_) => {
                // 文件删除
                for path in event.paths.clone() {
                    tracing::debug!(
                        component = "file_system_loader",
                        event = "remove_event",
                        path = ?path,
                        "Received Remove event"
                    );
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

    /// Bootstrap and load base configuration resources (GatewayClass, EdgionGatewayConfig, Gateway)
    async fn bootstrap_base_conf(&self) -> Result<()> {
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
                    // Only process base conf files
                    if path.extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext == "yml" || ext == "yaml")
                        .unwrap_or(false)
                    {
                        let content = match Self::read_file(&path).await {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(
                                    component = "file_system_loader",
                                    event = "failed_to_read_file",
                                    path = ?path,
                                    error = %e,
                                );
                                continue;
                            }
                        };
                        
                        if is_base_conf(&content) {
                            self.process_init_file(&path).await?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Bootstrap and load user configuration resources (all other resources)
    async fn bootstrap_user_conf(&self) -> Result<()> {
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
                    // Only process non-base conf files
                    if path.extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext == "yml" || ext == "yaml")
                        .unwrap_or(false)
                    {
                        let content = match Self::read_file(&path).await {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(
                                    component = "file_system_loader",
                                    event = "failed_to_read_file",
                                    path = ?path,
                                    error = %e,
                                );
                                continue;
                            }
                        };
                        
                        if !is_base_conf(&content) {
                            self.process_init_file(&path).await?;
                        }
                    }
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
