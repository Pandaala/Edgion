//! FileSystemStorage implementation
//!
//! Implements the CenterApi trait using local file system as the backend.
//! Stores resources as YAML files with naming convention: Kind_namespace_name.yaml

use crate::conf_mgr::conf_center::common::{ensure_expected_resource_version, merge_operator_update_yaml};
use crate::conf_mgr::conf_center::traits::{CenterApi, ConfEntry, ConfWriterError, ListOptions, ListResult};
use async_trait::async_trait;
use edgion_resources::resource::meta::extract_resource_metadata;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::fs;
use tokio::sync::Mutex;

use super::path_registry::PathRegistry;

static CREATE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Format YAML content to pretty format
///
/// This ensures consistent, readable YAML output by:
/// 1. Parsing the input YAML
/// 2. Re-serializing with proper formatting
///
/// If parsing fails, returns the original content unchanged.
fn format_yaml_pretty(content: &str) -> String {
    match yaml_serde::from_str::<yaml_serde::Value>(content) {
        Ok(value) => yaml_serde::to_string(&value).unwrap_or_else(|_| content.to_string()),
        Err(_) => content.to_string(),
    }
}

/// File system based configuration writer
///
/// Stores resources as YAML files with naming convention: Kind_namespace_name.yaml
/// or Kind__name.yaml for cluster-scoped resources.
pub struct FileSystemStorage {
    root: PathBuf,
    registry: Option<Arc<PathRegistry>>,
}

impl FileSystemStorage {
    /// Create a new file system writer with root directory path
    pub fn new<P: Into<PathBuf>>(root: P) -> Self {
        let root_abs = absolutize_path(root.into());

        tracing::info!(
            component = "file_system_writer",
            event = "init",
            root = ?root_abs,
            "Initialized FileSystemStorage"
        );

        Self {
            root: root_abs,
            registry: None,
        }
    }

    /// Attach a shared path registry; used by `resolve_path` and `create_one` to locate the canonical spec path.
    pub fn with_registry(mut self, registry: Arc<PathRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Get the root directory path
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// Resolve the file-system path for an existing resource.
    ///
    /// If a `PathRegistry` is attached and has a recorded winner for the
    /// given (kind, namespace/name) key, that winner path is returned.
    /// Otherwise falls back to the default landing path produced by
    /// `build_resource_path`.
    fn resolve_path(&self, kind: &str, namespace: Option<&str>, name: &str) -> PathBuf {
        if let Some(reg) = &self.registry {
            let key = match namespace {
                Some(ns) if !ns.is_empty() => format!("{}/{}", ns, name),
                _ => name.to_string(),
            };
            let rkey = super::path_registry::ResourceKey::new(kind.to_string(), key);
            if let Some(p) = reg.winner_of(&rkey) {
                return p;
            }
        }
        build_resource_path(&self.root, kind, namespace, name)
    }

    fn update_lock(&self, kind: &str, namespace: Option<&str>, name: &str) -> &Mutex<()> {
        static UPDATE_LOCKS: OnceLock<[Mutex<()>; 64]> = OnceLock::new();
        let locks = UPDATE_LOCKS.get_or_init(|| std::array::from_fn(|_| Mutex::new(())));
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.root.hash(&mut hasher);
        kind.hash(&mut hasher);
        namespace.hash(&mut hasher);
        name.hash(&mut hasher);
        &locks[(hasher.finish() as usize) % locks.len()]
    }

    async fn write_existing_atomic(&self, path: &Path, content: &str) -> Result<(), ConfWriterError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| ConfWriterError::io("Failed to create directory", e))?;
        }

        let formatted_content = format_yaml_pretty(content);
        let seq = CREATE_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_path = path.with_extension(format!("set.{seq}.tmp"));
        fs::write(&tmp_path, formatted_content)
            .await
            .map_err(|e| ConfWriterError::io("Failed to write tmp file", e))?;

        if let Err(e) = fs::rename(&tmp_path, path).await {
            let _ = fs::remove_file(&tmp_path).await;
            return Err(ConfWriterError::io("Failed to rename tmp file", e));
        }
        Ok(())
    }
}

#[async_trait]
impl CenterApi for FileSystemStorage {
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        validate_path_components(kind, namespace, name)?;
        let _guard = self.update_lock(kind, namespace, name).lock().await;
        let path = self.resolve_path(kind, namespace, name);
        self.write_existing_atomic(&path, &content).await?;

        tracing::info!(
            component = "file_system_writer",
            event = "resource_set",
            kind = kind,
            namespace = ?namespace,
            name = name,
            path = ?path,
            "Resource written to file"
        );

        Ok(())
    }

    async fn create_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        validate_path_components(kind, namespace, name)?;
        let _guard = self.update_lock(kind, namespace, name).lock().await;
        // Check the registry first: if a winner already exists for this key (e.g. a
        // legacy flat-layout file at root, or a previously-recorded nested file),
        // refuse the create rather than writing a new file at the default nested
        // landing path — that would create a cross-subdir collision and surface as
        // a /diagnostics/conf-conflicts entry instead of a clean AlreadyExists.
        if let Some(reg) = &self.registry {
            let key = match namespace {
                Some(ns) if !ns.is_empty() => format!("{}/{}", ns, name),
                _ => name.to_string(),
            };
            let rkey = super::path_registry::ResourceKey::new(kind.to_string(), key);
            if reg.winner_of(&rkey).is_some() {
                return Err(ConfWriterError::AlreadyExists(format!(
                    "{}/{}/{}",
                    kind,
                    namespace.unwrap_or("_"),
                    name
                )));
            }
        }

        let path = build_resource_path(&self.root, kind, namespace, name);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| ConfWriterError::io("Failed to create directory", e))?;
        }

        let formatted_content = format_yaml_pretty(&content);
        // Unique per-call tmp name (seq counter) avoids collisions between concurrent
        // create_one calls for the same resource, and is distinct from set_one's "yaml.tmp".
        // Path::extension() on "*.create.N.tmp" returns "tmp", correctly skipped by list scans.
        let seq = CREATE_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_path = path.with_extension(format!("create.{seq}.tmp"));

        fs::write(&tmp_path, formatted_content)
            .await
            .map_err(|e| ConfWriterError::io("Failed to write tmp file", e))?;

        // hard_link(tmp, dest) is atomic on POSIX and fails with AlreadyExists if dest
        // exists, closing the TOCTOU window that a plain exists()-then-rename would leave.
        match fs::hard_link(&tmp_path, &path).await {
            Ok(()) => {
                let _ = fs::remove_file(&tmp_path).await;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&tmp_path).await;
                return Err(ConfWriterError::AlreadyExists(format!(
                    "{}/{}/{}",
                    kind,
                    namespace.unwrap_or("_"),
                    name
                )));
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp_path).await;
                return Err(ConfWriterError::io("Failed to link tmp file", e));
            }
        }

        tracing::info!(
            component = "file_system_writer",
            event = "resource_created",
            kind = kind,
            namespace = ?namespace,
            name = name,
            path = ?path,
            "Resource created on file system"
        );

        Ok(())
    }

    async fn update_one_if_version(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
        expected_resource_version: Option<&str>,
    ) -> Result<(), ConfWriterError> {
        validate_path_components(kind, namespace, name)?;
        let _guard = self.update_lock(kind, namespace, name).lock().await;
        let path = self.resolve_path(kind, namespace, name);

        // Check if file exists
        if !path.exists() {
            return Err(ConfWriterError::NotFound(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        let current = fs::read_to_string(&path)
            .await
            .map_err(|e| ConfWriterError::io("Failed to read current resource for update", e))?;
        ensure_expected_resource_version(&current, expected_resource_version)?;
        let merged = merge_operator_update_yaml(&current, &content)?;

        self.write_existing_atomic(&path, &merged).await?;

        tracing::info!(
            component = "file_system_writer",
            event = "resource_updated",
            kind = kind,
            namespace = ?namespace,
            name = name,
            path = ?path,
            "Resource updated on file system"
        );

        Ok(())
    }

    async fn get_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<String, ConfWriterError> {
        validate_path_components(kind, namespace, name)?;
        let path = self.resolve_path(kind, namespace, name);

        if !path.exists() {
            return Err(ConfWriterError::NotFound(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| ConfWriterError::io("Failed to read file", e))?;

        Ok(content)
    }

    async fn list_all(&self, _opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        let root = self.root.clone();
        let entries: Vec<(PathBuf, String)> = tokio::task::spawn_blocking(move || -> Vec<(PathBuf, String)> {
            use walkdir::WalkDir;
            let mut out = Vec::new();
            for entry in WalkDir::new(&root).follow_links(false) {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path().to_path_buf();
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if !matches!(ext, "yaml" | "yml") {
                    continue;
                }
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    if name.ends_with(".status") || name.ends_with(".tmp") {
                        continue;
                    }
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    out.push((path, content));
                }
            }
            out
        })
        .await
        .map_err(|e| ConfWriterError::io("list_all join failed", std::io::Error::other(e)))?;

        let mut resources = Vec::with_capacity(entries.len());
        for (path, content) in entries {
            if let Some(reg) = &self.registry {
                if reg.is_recorded_loser(&path) {
                    continue;
                }
            }
            if let Some(metadata) = extract_resource_metadata(&content) {
                resources.push(ConfEntry {
                    kind: metadata.kind.unwrap_or_default(),
                    namespace: metadata.namespace,
                    name: metadata.name.unwrap_or_default(),
                    content,
                });
            } else {
                tracing::warn!(
                    component = "file_system_writer",
                    path = ?path,
                    "Failed to extract metadata from file"
                );
            }
        }

        tracing::info!(
            component = "file_system_writer",
            count = resources.len(),
            "Loaded all resources from file system (recursive)"
        );

        Ok(ListResult {
            items: resources,
            continue_token: None,
        })
    }

    async fn delete_one_if_version(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        expected_resource_version: Option<&str>,
    ) -> Result<(), ConfWriterError> {
        validate_path_components(kind, namespace, name)?;
        let _guard = self.update_lock(kind, namespace, name).lock().await;
        let path = self.resolve_path(kind, namespace, name);

        if !path.exists() {
            return Err(ConfWriterError::NotFound(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        // Validate before destroying: a rejected precondition must leave the live file
        // untouched. Every in-process writer to this key is serialized behind the same
        // update_lock, and FileSystem mode is the unconditional single leader, so there
        // is no concurrent writer to guard the read/unlink pair against.
        if expected_resource_version.is_some() {
            let current = fs::read_to_string(&path)
                .await
                .map_err(|e| ConfWriterError::io("Failed to read current resource for delete", e))?;
            ensure_expected_resource_version(&current, expected_resource_version)?;
        }

        fs::remove_file(&path)
            .await
            .map_err(|e| ConfWriterError::io("Failed to delete file", e))?;

        // Immediately hide this path from winner_of() so that a create_one() issued
        // before the fs_watcher fires its Remove event does not see a stale
        // AlreadyExists.  We intentionally do NOT call forget() here: the path stays
        // in by_path / by_key so the watcher can deliver the correct ForgetOutcome
        // (Removed / Promoted) and dispatch the Delete event through the normal pipeline.
        if let Some(reg) = &self.registry {
            reg.mark_api_deleted(&path);
        }

        // Best-effort cleanup of the sidecar `<file>.yaml.status` so it doesn't
        // outlive its config file.  The resource_controller also deletes status
        // files when it processes a Delete event, but that path depends on the
        // file_watcher delivering the event in time — for bulk deletes (e.g.
        // wipe_all) PollWatcher's poll_interval can leave many `.status`
        // orphans visible for a moment.  Removing it here closes that window.
        // NotFound is fine (resource may have never produced a status).
        let status_path = super::status::FileSystemStatusHandler::status_path(&path);
        if let Err(e) = fs::remove_file(&status_path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(
                    component = "file_system_writer",
                    status_file = %status_path.display(),
                    error = %e,
                    "Failed to remove sidecar status file (non-fatal)"
                );
            }
        }

        tracing::info!(
            component = "file_system_writer",
            event = "resource_deleted",
            kind = kind,
            namespace = ?namespace,
            name = name,
            "Resource deleted from file system"
        );

        Ok(())
    }

    async fn get_list_by_kind(&self, kind: &str, _opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        let root = self.root.clone();
        let prefix = format!("{}_", kind);
        let entries: Vec<(PathBuf, String)> = tokio::task::spawn_blocking(move || -> Vec<(PathBuf, String)> {
            use walkdir::WalkDir;
            let mut out = Vec::new();
            for entry in WalkDir::new(&root).follow_links(false) {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path().to_path_buf();
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if !matches!(ext, "yaml" | "yml") {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if name.ends_with(".status") || name.ends_with(".tmp") {
                    continue;
                }
                if !name.starts_with(&prefix) {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    out.push((path, content));
                }
            }
            out
        })
        .await
        .map_err(|e| ConfWriterError::io("list_by_kind join failed", std::io::Error::other(e)))?;

        let mut resources = Vec::with_capacity(entries.len());
        for (path, content) in entries {
            if let Some(reg) = &self.registry {
                if reg.is_recorded_loser(&path) {
                    continue;
                }
            }
            if let Some(metadata) = extract_resource_metadata(&content) {
                if metadata.kind.as_deref() == Some(kind) {
                    resources.push(ConfEntry {
                        kind: kind.to_string(),
                        namespace: metadata.namespace,
                        name: metadata.name.unwrap_or_default(),
                        content,
                    });
                }
            }
        }

        tracing::debug!(
            component = "file_system_writer",
            kind = kind,
            count = resources.len(),
            "Loaded resources by kind (recursive)"
        );

        Ok(ListResult {
            items: resources,
            continue_token: None,
        })
    }

    async fn get_list_by_kind_ns(
        &self,
        kind: &str,
        namespace: &str,
        _opts: Option<ListOptions>,
    ) -> Result<ListResult, ConfWriterError> {
        let root = self.root.clone();
        let prefix = format!("{}_{}_", kind, namespace);
        let entries: Vec<(PathBuf, String)> = tokio::task::spawn_blocking(move || -> Vec<(PathBuf, String)> {
            use walkdir::WalkDir;
            let mut out = Vec::new();
            for entry in WalkDir::new(&root).follow_links(false) {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path().to_path_buf();
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if !matches!(ext, "yaml" | "yml") {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if name.ends_with(".status") || name.ends_with(".tmp") {
                    continue;
                }
                if !name.starts_with(&prefix) {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    out.push((path, content));
                }
            }
            out
        })
        .await
        .map_err(|e| ConfWriterError::io("list_by_kind_ns join failed", std::io::Error::other(e)))?;

        let mut resources = Vec::with_capacity(entries.len());
        for (path, content) in entries {
            if let Some(reg) = &self.registry {
                if reg.is_recorded_loser(&path) {
                    continue;
                }
            }
            if let Some(metadata) = extract_resource_metadata(&content) {
                if metadata.kind.as_deref() == Some(kind) && metadata.namespace.as_deref() == Some(namespace) {
                    resources.push(ConfEntry {
                        kind: kind.to_string(),
                        namespace: Some(namespace.to_string()),
                        name: metadata.name.unwrap_or_default(),
                        content,
                    });
                }
            }
        }

        tracing::debug!(
            component = "file_system_writer",
            kind = kind,
            namespace = namespace,
            count = resources.len(),
            "Loaded resources by kind and namespace (recursive)"
        );

        Ok(ListResult {
            items: resources,
            continue_token: None,
        })
    }
}

/// Reject any resource identifier that could escape the conf root via directory
/// traversal before it is used to construct an on-disk path.
///
/// The namespaced admin API binds `{namespace}` / `{name}` as raw single URL
/// segments (axum percent-decodes but does not normalize them), and `kind` is
/// interpolated into the filename. `build_resource_path` uses `namespace` as a
/// raw subdirectory, so a literal `..` namespace would resolve `root.join("..")`
/// one level above the conf root (write/read/delete of an adjacent `.yaml`).
/// Legitimate Kubernetes identifiers are RFC 1123 labels and never contain
/// `..`, `/`, or `\`, so rejecting these characters is safe for all real input.
fn validate_path_components(kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError> {
    for (label, value) in [("kind", Some(kind)), ("namespace", namespace), ("name", Some(name))] {
        let Some(component) = value else { continue };
        if component.contains("..") || component.contains('/') || component.contains('\\') {
            return Err(ConfWriterError::ValidationError(format!(
                "invalid {label} path component: {component:?}"
            )));
        }
    }
    Ok(())
}

/// Build resource file path:
///   namespace-scoped: `<root>/<ns>/Kind_ns_name.yaml`
///   cluster-scoped:   `<root>/cluster/Kind__name.yaml`
pub(super) fn build_resource_path(root: &Path, kind: &str, namespace: Option<&str>, name: &str) -> PathBuf {
    let (subdir, filename) = match namespace {
        Some(ns) if !ns.is_empty() => (ns.to_string(), format!("{}_{}_{}.yaml", kind, ns, name)),
        _ => ("cluster".to_string(), format!("{}__{}.yaml", kind, name)),
    };
    root.join(subdir).join(filename)
}

/// Compute the canonical resource path from a (kind, key) pair under the
/// nested namespace-subdir layout. `key` is `"namespace/name"` for
/// namespace-scoped resources and just `"name"` for cluster-scoped.
///
/// Mirrors `build_resource_path` but accepts the canonical key form used by
/// the registry, watcher, and status handler.
pub(super) fn build_resource_path_from_key(root: &Path, kind: &str, key: &str) -> PathBuf {
    let (namespace, name) = match key.split_once('/') {
        Some((ns, n)) => (Some(ns), n),
        None => (None, key),
    };
    build_resource_path(root, kind, namespace, name)
}

/// Absolutize a possibly-relative path against the current working directory,
/// WITHOUT resolving symlinks. Used by both `FileSystemStorage::new` and
/// `FileSystemCenter::new` so the storage scan base and the watcher's recorded
/// paths share one identical base (design §0). Resolving symlinks on only one
/// side would create a new divergence, so this deliberately does not canonicalize.
pub(super) fn absolutize_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map(|cwd| cwd.join(&path)).unwrap_or(path)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/file_system/storage_test/path_tests.rs"]
mod path_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/file_system/storage_test/list_filter_tests.rs"]
mod list_filter_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/file_system/storage_test/update_contract_tests.rs"]
mod update_contract_tests;
