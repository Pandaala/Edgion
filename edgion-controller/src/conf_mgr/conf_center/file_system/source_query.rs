//! Filesystem source scans retain bounded identities, never a full document list.
use super::FileSystemStorage;
use crate::conf_mgr::conf_center::source_query::*;
use crate::conf_mgr::conf_center::traits::{
    ConfEntry, ConfWriterError, CountConsistency, ListOptions, ListResult, SourceCount,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
struct ScanBudget {
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    bytes: usize,
}
impl ScanBudget {
    fn check(&self) -> Result<(), ConfWriterError> {
        if self.cancelled.load(Ordering::Relaxed) || Instant::now() >= self.deadline {
            Err(ConfWriterError::SourceTimeout)
        } else {
            Ok(())
        }
    }
    fn read(&mut self, path: &Path) -> Result<String, ConfWriterError> {
        self.check()?;
        let file = std::fs::File::open(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                stale()
            } else {
                source_error()
            }
        })?;
        let mut bytes = Vec::new();
        file.take((MAX_SOURCE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| source_error())?;
        self.bytes = self.bytes.checked_add(bytes.len()).ok_or_else(budget_error)?;
        self.check()?;
        if bytes.len() > MAX_SOURCE_BYTES || self.bytes > MAX_SCAN_BYTES {
            return Err(budget_error());
        }
        String::from_utf8(bytes).map_err(|_| source_error())
    }
}
async fn blocking_scan<T: Send + 'static>(
    permit: tokio::sync::OwnedSemaphorePermit,
    work: impl FnOnce(Arc<AtomicBool>) -> Result<T, ConfWriterError> + Send + 'static,
) -> Result<T, ConfWriterError> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel = CancelOnDrop(cancelled.clone());
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work(cancelled)
    })
    .await
    .map_err(|_| source_error())?
}

fn read_source_path(
    path: &Path,
    registry: Option<&super::PathRegistry>,
    revision: Option<u64>,
    entry: ConfEntry,
    budget: &mut ScanBudget,
) -> Result<String, ConfWriterError> {
    budget.check()?;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        if registry.map(|r| r.revision()) != revision {
            stale()
        } else if error.kind() == std::io::ErrorKind::NotFound {
            ConfWriterError::NotFound("Source object not found".into())
        } else {
            source_error()
        }
    })?;
    if !metadata.is_file()
        || !matches!(path.extension().and_then(|s| s.to_str()), Some("yaml" | "yml"))
        || registry.is_some_and(|r| r.is_recorded_loser(path))
    {
        return Err(source_error());
    }
    let content = budget.read(path)?;
    if registry.map(|r| r.revision()) != revision {
        return Err(stale());
    }
    let entry = ConfEntry { content, ..entry };
    document(&entry)?;
    Ok(entry.content)
}

struct Candidate {
    path: PathBuf,
    digest: [u8; 32],
}
impl Candidate {
    fn read_unchanged(&self, budget: &mut ScanBudget) -> Result<String, ConfWriterError> {
        let content = budget.read(&self.path)?;
        let digest: [u8; 32] = Sha256::digest(content.as_bytes()).into();
        if digest != self.digest {
            return Err(stale());
        }
        Ok(content)
    }
}
struct ScanResult {
    page: ListResult,
    total: u64,
}

impl FileSystemStorage {
    pub(super) async fn replace_source(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
        expected: &SourcePrecondition,
    ) -> Result<(), ConfWriterError> {
        validate_locator(kind, namespace, name)?;
        let deadline = Instant::now() + SOURCE_TIMEOUT;
        tokio::time::timeout(SOURCE_TIMEOUT, async {
            let guard = self.update_lock(kind, namespace, name).lock().await;
            let permit = self
                .source_scans
                .clone()
                .try_acquire_owned()
                .map_err(|_| ConfWriterError::SourceBusy)?;
            let registry = self.registry.clone();
            let revision = registry.as_ref().map(|r| r.revision());
            let path = self.resolve_path(kind, namespace, name);
            let expected = expected.clone();
            let entry = ConfEntry {
                kind: kind.into(),
                namespace: namespace.map(str::to_owned),
                name: name.into(),
                content: String::new(),
            };
            blocking_scan(permit, move |cancelled| {
                let _guard = guard;
                let mut budget = ScanBudget {
                    deadline,
                    cancelled,
                    bytes: 0,
                };
                let current = read_source_path(&path, registry.as_deref(), revision, entry, &mut budget)?;
                expected.verify(&current)?;
                let merged = super::merge_operator_update_yaml(&current, &content).map_err(|_| source_error())?;
                if merged.len() > MAX_SOURCE_BYTES {
                    return Err(budget_error());
                }
                // External editors are not fenced; this protects same-process
                // writers and never redirects the write to an unread winner.
                super::write_existing_atomic_sync(&path, &merged, || {
                    budget.check()?;
                    if registry.as_ref().map(|r| r.revision()) != revision {
                        return Err(stale());
                    }
                    Ok(())
                })
            })
            .await
        })
        .await
        .map_err(|_| ConfWriterError::SourceTimeout)?
    }

    pub(super) async fn source_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
    ) -> Result<String, ConfWriterError> {
        validate_locator(kind, namespace, name)?;
        super::validate_path_components(kind, namespace, name)?;
        let deadline = Instant::now() + SOURCE_TIMEOUT;
        let permit = self
            .source_scans
            .clone()
            .try_acquire_owned()
            .map_err(|_| ConfWriterError::SourceBusy)?;
        let registry = self.registry.clone();
        let revision = registry.as_ref().map(|r| r.revision());
        let path = self.resolve_path(kind, namespace, name);
        let entry = ConfEntry {
            kind: kind.into(),
            namespace: namespace.map(str::to_owned),
            name: name.into(),
            content: String::new(),
        };
        tokio::time::timeout(
            SOURCE_TIMEOUT,
            blocking_scan(permit, move |cancelled| {
                let mut budget = ScanBudget {
                    deadline,
                    cancelled,
                    bytes: 0,
                };
                read_source_path(&path, registry.as_deref(), revision, entry, &mut budget)
            }),
        )
        .await
        .map_err(|_| ConfWriterError::SourceTimeout)?
    }

    async fn scan_source(
        &self,
        kind: &str,
        namespace: Option<&str>,
        opts: Option<ListOptions>,
    ) -> Result<ScanResult, ConfWriterError> {
        let deadline = Instant::now() + SOURCE_TIMEOUT;
        let permit = self
            .source_scans
            .clone()
            .try_acquire_owned()
            .map_err(|_| ConfWriterError::SourceBusy)?;
        let root = self.root.clone();
        let registry = self.registry.clone();
        let kind = kind.to_owned();
        let namespace = namespace.map(str::to_owned);
        let mut scope = SourceCursor {
            version: 1,
            backend: "filesystem".into(),
            generation: self.source_generation.clone(),
            kind: kind.clone(),
            namespace: namespace.clone(),
            limit: opts.as_ref().map_or(0, |o| o.limit),
            position: String::new(),
        };
        let after = if let Some(options) = &opts {
            if options.limit == 0 || options.limit > MAX_PAGE_SIZE {
                return Err(ConfWriterError::ValidationError("Invalid source page size".into()));
            }
            options
                .continue_token
                .as_deref()
                .map(|t| SourceCursor::decode(t, &scope))
                .transpose()?
                .map(|c| serde_json::from_str::<(String, String)>(&c.position).map_err(|_| stale()))
                .transpose()?
        } else {
            None
        };
        if let Some(position) = &after {
            let (ns, name) = position;
            if ns.len() > 63 || name.len() > 253 || name.is_empty() || name.contains('/') {
                return Err(stale());
            }
            super::validate_path_components(&kind, (!ns.is_empty()).then_some(ns), name).map_err(|_| stale())?;
            if namespace.as_deref().is_some_and(|scope| scope != ns) {
                return Err(stale());
            }
        }
        blocking_scan(permit, move |cancelled| {
            let revision = registry.as_ref().map(|r| r.revision());
            let mut budget = ScanBudget {
                deadline,
                cancelled,
                bytes: 0,
            };
            let mut seen = BTreeSet::new();
            let mut identity_bytes = 0usize;
            let mut candidates = BTreeMap::<(String, String), Candidate>::new();
            let mut total = 0;
            let prefix = namespace
                .as_ref()
                .map_or_else(|| format!("{kind}_"), |ns| format!("{kind}_{ns}_"));
            for (visited, entry) in walkdir::WalkDir::new(&root).follow_links(false).into_iter().enumerate() {
                budget.check()?;
                if visited >= MAX_SCAN_ENTRIES {
                    return Err(budget_error());
                }
                let entry = entry.map_err(|_| source_error())?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                if !matches!(path.extension().and_then(|s| s.to_str()), Some("yaml" | "yml")) {
                    continue;
                }
                let filename = path.file_name().and_then(|s| s.to_str()).ok_or_else(source_error)?;
                if !filename.starts_with(&prefix) {
                    continue;
                }
                if registry.as_ref().is_some_and(|r| r.is_recorded_loser(path)) {
                    continue;
                }
                if path.as_os_str().len() > 4096 {
                    return Err(budget_error());
                }
                let content = budget.read(path)?;
                let value: serde_json::Value = serde_yaml::from_str(&content).map_err(|_| source_error())?;
                // Match the source scope before retaining identities. A malformed matching
                // document fails the query rather than disappearing from count/list parity.
                if value["kind"].as_str() != Some(kind.as_str()) {
                    return Err(source_error());
                }
                let meta = metadata(&value)?;
                let name = meta.name.as_str();
                let ns = meta.namespace.as_deref();
                if name.is_empty() || name.len() > 253 || ns.is_some_and(|s| s.is_empty() || s.len() > 63) {
                    return Err(source_error());
                }
                super::validate_path_components(&kind, ns, name).map_err(|_| source_error())?;
                if namespace.as_deref().is_some_and(|expected| ns != Some(expected)) {
                    return Err(source_error());
                }
                let expected = format!("{kind}_{}_{}", ns.unwrap_or(""), name);
                if path.file_stem().and_then(|s| s.to_str()) != Some(expected.as_str()) {
                    return Err(source_error());
                }
                let key = (ns.unwrap_or("").to_owned(), name.to_owned());
                identity_bytes += key.0.len() + key.1.len() + 64;
                if identity_bytes > 16 * 1024 * 1024 {
                    return Err(budget_error());
                }
                if !seen.insert(key.clone()) {
                    return Err(source_error());
                }
                total += 1;
                if let Some(options) = &opts {
                    if after.as_ref().is_none_or(|a| &key > a) {
                        candidates.insert(
                            key,
                            Candidate {
                                path: path.to_owned(),
                                digest: Sha256::digest(content.as_bytes()).into(),
                            },
                        );
                        if candidates.len() > options.limit as usize + 1 {
                            candidates.pop_last();
                        }
                    }
                }
            }
            if registry.as_ref().map(|r| r.revision()) != revision {
                return Err(stale());
            }
            let mut items = Vec::new();
            let mut page_bytes = 0usize;
            let has_more = opts.as_ref().is_some_and(|o| candidates.len() > o.limit as usize);
            if has_more {
                candidates.pop_last();
            }
            for (key, candidate) in candidates {
                let content = candidate.read_unchanged(&mut budget)?;
                page_bytes += content.len();
                if page_bytes > MAX_SOURCE_BYTES {
                    return Err(budget_error());
                }
                scope.position = serde_json::to_string(&key).map_err(|_| source_error())?;
                let (ns, name) = key;
                items.push(ConfEntry {
                    kind: kind.clone(),
                    namespace: (!ns.is_empty()).then(|| ns.clone()),
                    name: name.clone(),
                    content,
                });
            }
            budget.check()?;
            if registry.as_ref().map(|r| r.revision()) != revision {
                return Err(stale());
            }
            let continue_token = if has_more { Some(scope.encode()?) } else { None };
            Ok(ScanResult {
                page: ListResult { items, continue_token },
                total,
            })
        })
        .await
    }

    pub(super) async fn source_page(
        &self,
        kind: &str,
        namespace: Option<&str>,
        opts: ListOptions,
    ) -> Result<ListResult, ConfWriterError> {
        self.scan_source(kind, namespace, Some(opts)).await.map(|s| s.page)
    }
    pub(super) async fn source_count(
        &self,
        kind: &str,
        namespace: Option<&str>,
    ) -> Result<SourceCount, ConfWriterError> {
        let scan = self.scan_source(kind, namespace, None).await?;
        Ok(SourceCount {
            total: scan.total,
            observed_at: chrono::Utc::now(),
            consistency: CountConsistency::ObservedScan,
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/file_system/source_query_test.rs"]
mod tests;
