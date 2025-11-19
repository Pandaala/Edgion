use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::fs;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::EventDispatcher;
use crate::core::utils::{extract_resource_metadata, is_base_conf, ResourceMetadata};
use crate::types::ResourceKind;

use super::types::FileInfo;

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
        let root_path = root.into();
        
        // 将 root 转换为绝对路径
        let root_abs = if root_path.is_absolute() {
            root_path
        } else {
            // 相对路径转换为绝对路径
            std::env::current_dir()
                .map(|cwd| cwd.join(&root_path))
                .unwrap_or(root_path)
        };
        
        tracing::info!(
            component = "file_system_loader",
            event = "init",
            root = ?root_abs,
            "Initialized FileSystemConfigLoader with absolute root path"
        );
        
        let loader = Arc::new(Self {
            root: root_abs,
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
            if let Err(err) = self.run_watcher().await {
                eprintln!(
                    "[FileSystemConfigLoader] watcher exited with error for {:?}: {}",
                    root, err
                );
            }
        })
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn dispatcher(&self) -> &Arc<dyn EventDispatcher> {
        &self.dispatcher
    }

    async fn dispatch_change(&self, change: ResourceChange, data: String, use_base_conf: bool) {
        // Pass YAML data directly to dispatcher
        if use_base_conf {
            self.dispatcher
                .apply_base_conf(change, None, data, None);
        } else {
            self.dispatcher
                .apply_resource_change(change, None, data, None);
        }
    }

    pub async fn read_file(path: &Path) -> Result<String> {
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

    /// 验证文件名是否符合命名规范: namespace_name_kind.yaml
    /// 对于 cluster-scoped 资源（namespace 为 None），格式为: _name_kind.yaml
    /// 返回期望的文件名，如果验证通过返回 None，如果失败返回期望的文件名
    fn validate_filename_format(&self, path: &Path, metadata: &ResourceMetadata) -> Option<String> {
        // 获取文件名（不含路径）
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => return None,
        };

        // 根据 metadata 构建期望的文件名
        let expected_name = if let (Some(ns), Some(name), Some(kind)) = 
            (metadata.namespace.as_ref(), metadata.name.as_ref(), metadata.kind.as_ref()) {
            format!("{}_{}_{}.yaml", ns, name, kind)
        } else if let (Some(name), Some(kind)) = (metadata.name.as_ref(), metadata.kind.as_ref()) {
            // cluster-scoped 资源
            format!("_{}_{}.yaml", name, kind)
        } else {
            return None;
        };

        // 如果文件名匹配，返回 None（表示验证通过）
        // 如果不匹配，返回期望的文件名
        if file_name == expected_name {
            None
        } else {
            Some(expected_name)
        }
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

        // 验证文件名格式是否符合规范: namespace_name_kind.yaml
        if let Some(expected_filename) = self.validate_filename_format(path, &metadata) {
            tracing::error!(
                component = "file_system_loader",
                event = "filename_format_mismatch",
                path = ?path,
                actual_filename = ?path.file_name(),
                expected_filename = expected_filename,
                kind = ?metadata.kind,
                namespace = ?metadata.namespace,
                name = ?metadata.name,
                "File name does not match required format. Your file should be named: {}",
                expected_filename
            );
            return Ok(None);
        }

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
    /// 如果指定了 filter_kind，则只处理匹配该类型的资源
    pub async fn process_init_file(&self, path: &Path, filter_kind: Option<ResourceKind>) -> Result<()> {
        let result = self.load_and_store_file(path).await?;
        
        let Some((metadata, content, _existed_before)) = result else {
            return Ok(());
        };

        // Check kind filter if specified
        if let Some(target_kind) = filter_kind {
            let current_kind = metadata.kind.as_deref()
                .and_then(|k| ResourceKind::from_str_name(k));
            
            if current_kind != Some(target_kind) {
                return Ok(());
            }
        }

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
    pub async fn process_file(&self, path: &Path) -> Result<()> {

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

    pub async fn handle_event(&self, event: Event) -> Result<()> {
        // 提取所有路径，只处理在监控目录 root 下的文件
        // process_file 会自动判断是 add/update/delete
        for path in event.paths {
            // 只处理在监控目录下的路径
            if !path.starts_with(&self.root) {
                tracing::debug!(
                    component = "file_system_loader",
                    event = "skip_path_outside_root",
                    path = ?path,
                    root = ?self.root,
                    "Skipping path outside of monitored root directory"
                );
                continue;
            }

            // 只处理 .yaml 或 .yml 文件，其他文件直接跳过
            let extension = path.extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("");
            if extension != "yml" && extension != "yaml" {
                continue;
            }

            tracing::debug!(
                component = "file_system_loader",
                event = "handle_event",
                path = ?path,
                event_kind = ?event.kind,
                "Processing file event"
            );

            self.process_file(&path).await?;
        }
        Ok(())
    }

    pub async fn run_watcher(&self) -> Result<()> {
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

