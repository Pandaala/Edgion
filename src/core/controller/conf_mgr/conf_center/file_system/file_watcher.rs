//! FileSystemWatcher - Centralized file watching with event dispatch
//!
//! Provides a unified `notify::PollWatcher` (stat-loop polling, 1 s interval)
//! that dispatches events to per-kind broadcast channels. Each resource type
//! gets its own independent event stream, similar to how K8s mode uses one
//! watch per resource.
//!
//! We use PollWatcher rather than the OS-native `RecommendedWatcher` because
//! macOS FSEvents silently drops events under bursty bulk-delete workloads
//! (~90 % loss when hundreds of files are unlinked concurrently). PollWatcher
//! trades real-time delivery for reliability — fixed poll-interval latency
//! but never loses an event.

use super::event::{FileSystemEvent, ParsedFileInfo};
use crate::core::controller::conf_mgr::sync_runtime::ShutdownSignal;
use crate::types::ResourceKind;
use anyhow::{Context, Result};
use notify::event::{ModifyKind, RenameMode};
use notify::{Config, Event, EventKind, PollWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, RwLock};

/// Sender type for parsed resource events per kind
pub type KindEventSender = broadcast::Sender<FileSystemEvent<String>>;
/// Receiver type for parsed resource events per kind
pub type KindEventReceiver = broadcast::Receiver<FileSystemEvent<String>>;

/// FileSystemWatcher - Centralized file watching with event dispatch by kind
///
/// Architecture:
/// ```text
/// [PollWatcher (1 s stat diff)] -> [dispatcher] -> [kind channels]
///                                       │
///                                       ├── HTTPRoute channel
///                                       ├── Gateway channel
///                                       ├── Secret channel
///                                       └── ... other channels
/// ```
pub struct FileSystemWatcher {
    /// Configuration directory to watch
    conf_dir: PathBuf,

    /// Event senders per resource kind
    /// Key is kind name (e.g., "HTTPRoute", "Gateway")
    senders: Arc<RwLock<HashMap<&'static str, KindEventSender>>>,
}

impl FileSystemWatcher {
    /// Create a new FileSystemWatcher
    pub fn new(conf_dir: PathBuf) -> Self {
        Self {
            conf_dir,
            senders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a kind and get its event receiver
    ///
    /// Must be called before `run()` for each resource kind that needs events.
    pub async fn subscribe(&self, kind: &'static str) -> KindEventReceiver {
        let mut senders = self.senders.write().await;

        if let Some(sender) = senders.get(kind) {
            sender.subscribe()
        } else {
            // Per-kind broadcast capacity. PollWatcher batches all per-cycle
            // diffs into rapid sequential sends; bulk operations like wipe-all
            // can push many events to one kind channel before a slow consumer
            // drains them.  4096 leaves headroom for our test-fixture loads
            // (single-kind peaks ~110 in the integration suite); larger fleets
            // could in principle exceed this, in which case `Lagged` would be
            // logged with no automatic recovery — that's a known follow-up.
            let (tx, rx) = broadcast::channel(4096);
            senders.insert(kind, tx);
            rx
        }
    }

    /// Get the configuration directory
    pub fn conf_dir(&self) -> &Path {
        &self.conf_dir
    }

    /// Run the watcher: init phase + runtime phase
    ///
    /// This method:
    /// 1. Init phase: Scans directory, dispatches Init/InitApply/InitDone per kind
    /// 2. Runtime phase: Watches for changes, dispatches Apply/Delete events
    pub async fn run(&self, mut shutdown_signal: ShutdownSignal) -> Result<()> {
        tracing::info!(
            component = "fs_watcher",
            conf_dir = %self.conf_dir.display(),
            "Starting FileSystemWatcher"
        );

        // Phase 1: Init - scan directory and dispatch events
        self.init_phase().await?;

        // Phase 2: Runtime - watch for changes
        self.runtime_phase(&mut shutdown_signal).await?;

        tracing::info!(component = "fs_watcher", "FileSystemWatcher stopped");

        Ok(())
    }

    /// Init phase: scan directory and dispatch Init/InitApply/InitDone events per kind
    pub async fn init_phase(&self) -> Result<()> {
        tracing::info!(
            component = "fs_watcher",
            conf_dir = %self.conf_dir.display(),
            "Starting init phase"
        );

        // Collect files grouped by kind
        let mut files_by_kind: HashMap<&'static str, Vec<(PathBuf, String)>> = HashMap::new();

        // Scan directory
        let mut entries = tokio::fs::read_dir(&self.conf_dir)
            .await
            .with_context(|| format!("Failed to read directory: {}", self.conf_dir.display()))?;

        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Failed to read directory entry: {}", self.conf_dir.display()));
                }
            };
            let path = entry.path();

            // Skip non-YAML files
            if !is_yaml_file(&path) {
                continue;
            }

            // Read file content
            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        component = "fs_watcher",
                        path = %path.display(),
                        error = %e,
                        "Failed to read file during init"
                    );
                    continue;
                }
            };

            // Determine kind from content
            if let Some(kind) = ResourceKind::from_content(&content) {
                let kind_str = kind.as_str();
                files_by_kind.entry(kind_str).or_default().push((path, content));
            }
        }

        // Dispatch events per kind
        let senders = self.senders.read().await;

        for (kind, files) in &files_by_kind {
            if let Some(sender) = senders.get(kind) {
                // Send Init
                let _ = sender.send(FileSystemEvent::Init);

                // Send InitApply for each file
                for (path, content) in files {
                    // The content is sent as the event payload with file path
                    // ResourceController will parse it to the actual type
                    let _ = sender.send(FileSystemEvent::InitApply {
                        path: path.clone(),
                        content: content.clone(),
                    });

                    tracing::trace!(
                        component = "fs_watcher",
                        kind = kind,
                        path = %path.display(),
                        "Dispatched InitApply event"
                    );
                }

                // Send InitDone
                let _ = sender.send(FileSystemEvent::InitDone);

                tracing::debug!(
                    component = "fs_watcher",
                    kind = kind,
                    count = files.len(),
                    "Completed init for kind"
                );
            }
        }

        // Send Init/InitDone for kinds that have subscribers but no files
        for (kind, sender) in senders.iter() {
            if !files_by_kind.contains_key(kind) {
                let _ = sender.send(FileSystemEvent::Init);
                let _ = sender.send(FileSystemEvent::InitDone);

                tracing::debug!(
                    component = "fs_watcher",
                    kind = kind,
                    "Sent empty init for kind (no files found)"
                );
            }
        }

        let total_files: usize = files_by_kind.values().map(|v| v.len()).sum();
        tracing::info!(
            component = "fs_watcher",
            total_files = total_files,
            kinds = files_by_kind.len(),
            "Init phase complete"
        );

        Ok(())
    }

    /// Runtime phase: watch for file changes and dispatch Apply/Delete events
    async fn runtime_phase(&self, shutdown_signal: &mut ShutdownSignal) -> Result<()> {
        tracing::info!(
            component = "fs_watcher",
            conf_dir = %self.conf_dir.display(),
            "Starting runtime phase"
        );

        // Decide poll_interval up front based on the conf directory size.
        // PollWatcher reads + hashes every watched file each cycle (see
        // `with_compare_contents(true)` below), so a busy directory should
        // poll less often or it eats CPU/IO for no real benefit. Counting
        // is one read_dir syscall (+ stat per entry) so it's cheap even at
        // tens of thousands of files. We do this once per controller
        // lifetime — conf size doesn't change in bursts in practice, and
        // operators can restart to retune; see tasks/todo/multi-conf-dir.md
        // for the recursive variant when nested directories are supported.
        let conf_file_count = count_yaml_files(&self.conf_dir).await;
        let poll_interval = poll_interval_for(conf_file_count);
        if conf_file_count > 10_000 {
            tracing::warn!(
                component = "fs_watcher",
                conf_file_count,
                interval_secs = poll_interval.as_secs(),
                "Conf directory has unusually many YAML files; consider partitioning the deployment \
                 or moving the controller to the K8s backend",
            );
        } else {
            tracing::info!(
                component = "fs_watcher",
                conf_file_count,
                interval_secs = poll_interval.as_secs(),
                "Selected PollWatcher interval based on conf size",
            );
        }

        // Unbounded channel for raw file events.  The producer is the notify
        // PollWatcher callback (runs on a non-tokio thread).  An unbounded
        // sender means the callback never blocks; each `Event` is small and
        // the tokio consumer drains in microseconds, so unbounded growth is
        // not a realistic concern.
        let (tx, mut rx) = mpsc::unbounded_channel::<Event>();

        let conf_dir = self.conf_dir.clone();
        let watcher = tokio::task::spawn_blocking(move || -> Result<PollWatcher> {
            let tx_clone = tx.clone();
            // PollWatcher: stat-based polling instead of OS push events.
            //
            // We deliberately use PollWatcher here even though notify's
            // RecommendedWatcher would pick a faster OS-native backend
            // (FSEvents on macOS, inotify on Linux).  Reason: macOS FSEvents
            // silently drops events under bursty bulk-delete workloads —
            // empirically ~90% loss when hundreds of files are unlinked
            // concurrently in <200 ms (e.g. wipe_all).  PollWatcher trades
            // real-time delivery for reliability: fixed poll_interval latency
            // (1 s) but never loses an event because diff is computed by
            // walking the directory and comparing to its last snapshot.
            //
            // `with_compare_contents(true)` is required: atomic writes via
            // tmp + rename (FileSystemStorage::set_one) can land two
            // back-to-back updates within a single poll cycle.  When the
            // resulting files have the same length and mtime granularity is
            // coarse, the default size+mtime diff misses the second update
            // and the controller never re-applies it.  Hashing each file's
            // contents per cycle is O(KB·N) per second on a normal conf dir
            // and closes that window.
            let mut watcher = PollWatcher::new(
                move |res: Result<Event, notify::Error>| {
                    if let Ok(event) = res {
                        // Surface any backend rescan signal (e.g. inotify Q_OVERFLOW
                        // if we ever switch back) so it's visible in logs.
                        if let Some(flag) = event.flag() {
                            tracing::warn!(
                                component = "fs_watcher",
                                flag = ?flag,
                                "Notify backend signaled rescan-needed flag"
                            );
                        }
                        match event.kind {
                            EventKind::Access(_) => {} // Ignore read-only access
                            _ => {
                                tracing::info!(
                                    component = "fs_watcher",
                                    event_kind = ?event.kind,
                                    paths = ?event.paths,
                                    "Notify backend emitted event"
                                );
                                let _ = tx_clone.send(event);
                            }
                        }
                    }
                },
                Config::default()
                    .with_poll_interval(poll_interval)
                    .with_compare_contents(true),
            )
            .context("Failed to create file watcher")?;

            watcher
                .watch(&conf_dir, RecursiveMode::NonRecursive)
                .context("Failed to start watching directory")?;

            Ok(watcher)
        })
        .await??;

        // Simple direct dispatch: events arrive from the notify callback via `rx`,
        // we hand each one straight to `dispatch_file_event` which uses the event kind
        // (with a brief 200ms recheck for ambiguous kinds) to decide Apply vs Delete.
        //
        // No debounce / pending HashMap — PollWatcher already coalesces events at the
        // poll-cycle granularity (one Modify per file per cycle even if it changed
        // multiple times), so we don't need application-level debouncing.
        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    for path in event.paths {
                        if is_yaml_file(&path) {
                            self.dispatch_file_event(&path, event.kind.clone()).await;
                        }
                    }
                }
                _ = shutdown_signal.wait() => {
                    tracing::info!(
                        component = "fs_watcher",
                        "Received shutdown signal"
                    );
                    break;
                }
            }
        }

        // Explicitly drop watcher to stop file monitoring
        drop(watcher);

        Ok(())
    }

    /// Dispatch a file event (Apply or Delete) to the appropriate kind channel.
    ///
    /// `last_kind` is mapped to an `Apply` or `Delete` action via
    /// `infer_dispatch_action`. Under PollWatcher this is a clean Create /
    /// Modify / Remove with no ambiguity, so the `Recheck` branch is dormant;
    /// it remains as defense-in-depth in case we ever route ambiguous kinds
    /// (e.g. a future inotify-based fallback) through this code path.
    async fn dispatch_file_event(&self, path: &Path, last_kind: EventKind) {
        let Some(info) = parse_resource_filename(path) else {
            tracing::trace!(
                component = "fs_watcher",
                path = %path.display(),
                "Skipping file: not a valid resource filename"
            );
            return;
        };

        // Clone the sender so we release the read-lock before any await points.
        let sender = {
            let senders = self.senders.read().await;
            let Some(s) = senders.get(info.kind.as_str()) else {
                tracing::trace!(
                    component = "fs_watcher",
                    kind = %info.kind,
                    path = %path.display(),
                    "No subscriber for kind"
                );
                return;
            };
            s.clone()
        };

        let mut action = infer_dispatch_action(&last_kind, path.exists());

        if action == DispatchAction::Recheck {
            // Defense-in-depth: only triggered for ambiguous EventKinds, which
            // PollWatcher never emits.  Kept for safety if an inotify-style
            // backend is ever wired in alongside.
            tokio::time::sleep(Duration::from_millis(200)).await;
            action = if path.exists() {
                DispatchAction::Apply
            } else {
                DispatchAction::Delete
            };
        }

        match action {
            DispatchAction::Apply => match tokio::fs::read_to_string(path).await {
                Ok(content) => {
                    let _ = sender.send(FileSystemEvent::Apply {
                        path: path.to_path_buf(),
                        content,
                    });
                    tracing::info!(
                        component = "fs_watcher",
                        kind = %info.kind,
                        key = %info.key,
                        last_kind = ?last_kind,
                        "Dispatched Apply event"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        component = "fs_watcher",
                        path = %path.display(),
                        error = %e,
                        "Failed to read file for Apply event"
                    );
                }
            },
            DispatchAction::Delete => {
                let delete_info = format!("__DELETE__:{}:{}", info.kind, info.key);
                let _ = sender.send(FileSystemEvent::Delete(delete_info));
                tracing::info!(
                    component = "fs_watcher",
                    kind = %info.kind,
                    key = %info.key,
                    last_kind = ?last_kind,
                    "Dispatched Delete event"
                );
            }
            DispatchAction::Recheck => unreachable!(),
        }
    }
}

// ============================================================================
// Dispatch classification
// ============================================================================

/// Outcome of dispatch classification for a pending file path.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DispatchAction {
    Apply,
    Delete,
    /// Event type is ambiguous; caller should recheck path existence after a short delay.
    Recheck,
}

/// Classify whether a pending path should trigger Apply, Delete, or a re-existence check.
///
/// Uses the last-seen EventKind to distinguish atomic-replace (rename) from true deletion.
/// Falls back to `path_exists` for ambiguous event types, emitting Recheck when the path
/// is absent so the caller can wait out any in-flight create before committing to Delete.
pub(crate) fn infer_dispatch_action(last_kind: &EventKind, path_exists: bool) -> DispatchAction {
    match last_kind {
        // Explicit removal or "renamed away" — treat as Delete regardless of path state.
        EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => DispatchAction::Delete,
        // Explicit creation or "renamed into" — treat as Apply.
        EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => DispatchAction::Apply,
        // All other kinds (data/metadata modify, Any, Other, Name(Both/Any)) are ambiguous.
        // Resolve by path existence; if absent, ask caller to recheck after a short delay
        // so an in-flight atomic-replace can complete before we commit to Delete.
        _ => {
            if path_exists {
                DispatchAction::Apply
            } else {
                DispatchAction::Recheck
            }
        }
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Count YAML files (excluding `.status`) directly under `dir`.
///
/// One `read_dir` + per-entry stat — cheap even for tens of thousands of
/// files. Errors and non-yaml entries are silently skipped; this counter
/// only feeds `poll_interval_for` and a failure to count just means we fall
/// back to a 1 s interval, which is the conservative default.
///
/// NOTE: this walks only the top level. When/if the FileSystem backend gains
/// recursive directory support (see `tasks/todo/multi-conf-dir.md`), switch
/// this to a recursive walk so the count still reflects the watch set.
async fn count_yaml_files(dir: &Path) -> usize {
    let mut count = 0usize;
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return 0,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if is_yaml_file(&entry.path()) {
            count += 1;
        }
    }
    count
}

/// Pick a PollWatcher poll interval based on the conf directory size.
///
/// PollWatcher with `with_compare_contents(true)` reads + hashes every watched
/// file each cycle, so cost scales with file count. Larger conf trees should
/// poll less often; tiny ones stay at 1 s for snappy reaction. The mapping is:
///
/// |    files    | interval |
/// |-------------|----------|
/// |   0 –  2000 |     1 s  |
/// |   ≤  4000   |     2 s  |
/// |   ≤  6000   |     3 s  |
/// |   ≤  8000   |     4 s  |
/// |   > 8000    |     5 s  |
///
/// 5 s is the upper bound — beyond ~10 000 files PollWatcher is no longer the
/// right tool, and we log a WARN instead of pushing the interval higher.
fn poll_interval_for(file_count: usize) -> Duration {
    let bucket = (file_count as f64 / 2000.0).ceil() as u64;
    let secs = bucket.clamp(1, 5);
    Duration::from_secs(secs)
}

/// Check if a path is a YAML file (excluding .status files)
fn is_yaml_file(path: &Path) -> bool {
    // Skip .status files
    if let Some(name) = path.file_name() {
        if name.to_string_lossy().ends_with(".status") {
            return false;
        }
    }

    path.extension()
        .map(|ext| ext == "yaml" || ext == "yml")
        .unwrap_or(false)
}

/// Parse resource info from filename
///
/// Filename format:
/// - With namespace: `{Kind}_{namespace}_{name}.yaml`
/// - Cluster-scoped: `{Kind}__{name}.yaml` (double underscore)
///
/// Returns ParsedFileInfo with kind, namespace, name, and key
fn parse_resource_filename(path: &Path) -> Option<ParsedFileInfo> {
    let filename = path.file_stem()?.to_str()?;

    // Split into at most 3 parts by underscore
    let parts: Vec<&str> = filename.splitn(3, '_').collect();

    if parts.len() == 3 {
        let kind = parts[0].to_string();
        let namespace = if parts[1].is_empty() {
            None // Double underscore means cluster-scoped
        } else {
            Some(parts[1].to_string())
        };
        let name = parts[2].to_string();
        Some(ParsedFileInfo::new(kind, namespace, name))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};

    #[test]
    fn poll_interval_clamps_at_min_for_empty_or_small_conf() {
        assert_eq!(poll_interval_for(0), Duration::from_secs(1));
        assert_eq!(poll_interval_for(1), Duration::from_secs(1));
        assert_eq!(poll_interval_for(2_000), Duration::from_secs(1));
    }

    #[test]
    fn poll_interval_steps_every_2000_files() {
        assert_eq!(poll_interval_for(2_001), Duration::from_secs(2));
        assert_eq!(poll_interval_for(4_000), Duration::from_secs(2));
        assert_eq!(poll_interval_for(4_001), Duration::from_secs(3));
        assert_eq!(poll_interval_for(6_000), Duration::from_secs(3));
        assert_eq!(poll_interval_for(6_001), Duration::from_secs(4));
        assert_eq!(poll_interval_for(8_000), Duration::from_secs(4));
    }

    #[test]
    fn poll_interval_clamps_at_max_for_large_conf() {
        assert_eq!(poll_interval_for(8_001), Duration::from_secs(5));
        assert_eq!(poll_interval_for(10_000), Duration::from_secs(5));
        assert_eq!(poll_interval_for(50_000), Duration::from_secs(5));
        assert_eq!(poll_interval_for(usize::MAX), Duration::from_secs(5));
    }

    #[test]
    fn remove_file_event_triggers_delete() {
        assert_eq!(
            infer_dispatch_action(&EventKind::Remove(RemoveKind::File), false),
            DispatchAction::Delete
        );
    }

    #[test]
    fn remove_any_event_triggers_delete() {
        assert_eq!(
            infer_dispatch_action(&EventKind::Remove(RemoveKind::Any), false),
            DispatchAction::Delete
        );
    }

    #[test]
    fn remove_event_triggers_delete_even_when_path_exists() {
        // Conservative: trust the kernel Remove event, not the existence check.
        assert_eq!(
            infer_dispatch_action(&EventKind::Remove(RemoveKind::File), true),
            DispatchAction::Delete
        );
    }

    #[test]
    fn rename_from_event_triggers_delete() {
        // Modify(Name(From)) = file was renamed/moved away = effectively deleted.
        assert_eq!(
            infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::From)), false),
            DispatchAction::Delete
        );
    }

    #[test]
    fn rename_from_event_triggers_delete_even_when_path_exists() {
        assert_eq!(
            infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::From)), true),
            DispatchAction::Delete
        );
    }

    #[test]
    fn create_event_with_existing_file_triggers_apply() {
        assert_eq!(
            infer_dispatch_action(&EventKind::Create(CreateKind::File), true),
            DispatchAction::Apply
        );
    }

    #[test]
    fn rename_to_event_triggers_apply() {
        // Modify(Name(To)) = file was renamed/moved into watched dir.
        assert_eq!(
            infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::To)), true),
            DispatchAction::Apply
        );
    }

    #[test]
    fn modify_data_event_with_existing_file_triggers_apply() {
        assert_eq!(
            infer_dispatch_action(&EventKind::Modify(ModifyKind::Data(DataChange::Content)), true),
            DispatchAction::Apply
        );
    }

    #[test]
    fn modify_data_event_with_missing_file_triggers_recheck() {
        // Ambiguous-kind defense-in-depth: a Modify whose path no longer exists could
        // be an in-flight rename (file briefly absent between unlink and link).
        // PollWatcher never reports this kind, but the classifier must still answer.
        assert_eq!(
            infer_dispatch_action(&EventKind::Modify(ModifyKind::Data(DataChange::Content)), false),
            DispatchAction::Recheck
        );
    }

    #[test]
    fn any_event_with_existing_file_triggers_apply() {
        assert_eq!(infer_dispatch_action(&EventKind::Any, true), DispatchAction::Apply);
    }

    #[test]
    fn any_event_with_missing_file_triggers_recheck() {
        assert_eq!(infer_dispatch_action(&EventKind::Any, false), DispatchAction::Recheck);
    }

    // Modify(Name(To)) — the destination half of an atomic rename. Whether the
    // sequence reaches us this way (notify backends differ in event granularity)
    // or as a single Modify, file existing → Apply.
    #[test]
    fn rename_to_with_existing_file_triggers_apply() {
        assert_eq!(
            infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::To)), true),
            DispatchAction::Apply
        );
    }

    // Plain Create on an existing file → Apply.
    #[test]
    fn create_with_existing_file_triggers_apply() {
        assert_eq!(
            infer_dispatch_action(&EventKind::Create(CreateKind::Any), true),
            DispatchAction::Apply
        );
    }

    // Rename(Both) is uncertain (two paths in one event, per-path semantic unclear).
    #[test]
    fn rename_both_with_existing_file_triggers_apply() {
        assert_eq!(
            infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::Both)), true),
            DispatchAction::Apply
        );
    }

    #[test]
    fn rename_both_with_missing_file_triggers_recheck() {
        assert_eq!(
            infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::Both)), false),
            DispatchAction::Recheck
        );
    }
}

/// Build file path from kind and key
///
/// Key format:
/// - Namespaced: "namespace/name"
/// - Cluster-scoped: "name"
pub fn build_path_from_key(conf_dir: &Path, kind: &str, key: &str) -> PathBuf {
    let (namespace, name) = if let Some(pos) = key.find('/') {
        (Some(&key[..pos]), &key[pos + 1..])
    } else {
        (None, key)
    };

    let filename = match namespace {
        Some(ns) => format!("{}_{}_{}.yaml", kind, ns, name),
        None => format!("{}__{}.yaml", kind, name),
    };

    conf_dir.join(filename)
}
