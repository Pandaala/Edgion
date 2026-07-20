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
use super::path_registry::{ForgetOutcome, PathRegistry, RecordOutcome, ResourceKey};
use crate::conf_mgr::sync_runtime::ShutdownSignal;
use anyhow::{Context, Result};
use edgion_resources::ResourceKind;
use notify::event::{ModifyKind, RenameMode};
use notify::{Config, Event, EventKind, PollWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, RwLock};
use walkdir::WalkDir;

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

    /// Shared path registry (written by the watcher, read by status/storage)
    registry: Arc<PathRegistry>,

    /// Event senders per resource kind
    /// Key is kind name (e.g., "HTTPRoute", "Gateway")
    senders: Arc<RwLock<HashMap<&'static str, KindEventSender>>>,

    /// Optional override for the PollWatcher poll interval.
    /// When `Some`, skips the file-count heuristic and uses this duration directly.
    poll_interval_override: Option<Duration>,

    /// Scan snapshot captured by `prime`, consumed by `dispatch_init`.
    /// Maps kind -> winning (path, content) pairs from the last scan.
    primed: Mutex<Option<HashMap<&'static str, Vec<(PathBuf, String)>>>>,
}

impl FileSystemWatcher {
    /// Create a new FileSystemWatcher
    pub fn new(conf_dir: PathBuf, registry: Arc<PathRegistry>, poll_interval_override: Option<Duration>) -> Self {
        Self {
            conf_dir,
            registry,
            senders: Arc::new(RwLock::new(HashMap::new())),
            poll_interval_override,
            primed: Mutex::new(None),
        }
    }

    /// Register a kind and get its event receiver
    ///
    /// Must be called before `dispatch_init` for each resource kind that needs events.
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
            // logged with no automatic recovery.
            let (tx, rx) = broadcast::channel(4096);
            senders.insert(kind, tx);
            rx
        }
    }

    /// Get the configuration directory
    pub fn conf_dir(&self) -> &Path {
        &self.conf_dir
    }

    /// Scan the conf dir once: build the next path generation, swap the registry
    /// atomically, and store the per-kind winning files for `dispatch_init`.
    /// Emits no events.
    pub async fn prime(&self) -> Result<()> {
        tracing::info!(
            component = "fs_watcher",
            conf_dir = %self.conf_dir.display(),
            "Starting init phase (recursive)"
        );

        let conf_dir = self.conf_dir.clone();
        let scan: Vec<(PathBuf, String)> = tokio::task::spawn_blocking(move || -> Vec<(PathBuf, String)> {
            let mut out = Vec::new();
            for entry in WalkDir::new(&conf_dir).follow_links(false) {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(
                            component = "fs_watcher",
                            path = ?e.path(),
                            error = %e,
                            "Skipping unreadable subtree during init"
                        );
                        continue;
                    }
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path().to_path_buf();
                if !is_yaml_file(&path) {
                    continue;
                }
                let content = match std::fs::read_to_string(&path) {
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
                out.push((path, content));
            }
            out.sort_by(|a, b| a.0.cmp(&b.0));
            out
        })
        .await
        .context("init scan join failed")?;

        let mut files_by_kind: HashMap<&'static str, Vec<(PathBuf, String)>> = HashMap::new();
        // Build the next path generation privately. Reusing the live registry
        // would classify every path from the previous watcher generation as
        // AlreadyKnown and reload empty processor caches. The final atomic swap
        // keeps read APIs on a complete generation throughout the scan.
        let next_registry = PathRegistry::new();

        for (path, content) in scan {
            let Some(kind) = ResourceKind::from_content(&content) else {
                continue;
            };
            let kind_str = kind.as_str();
            let Some(info) = parse_resource_filename(&path) else {
                tracing::trace!(
                    component = "fs_watcher",
                    path = %path.display(),
                    "Skipping init file: filename does not match Kind_*.yaml"
                );
                continue;
            };
            let rkey = ResourceKey::new(info.kind.clone(), info.key.clone());
            match next_registry.record(rkey, path.clone()) {
                RecordOutcome::NewWinner { .. } => {
                    files_by_kind.entry(kind_str).or_default().push((path, content));
                }
                RecordOutcome::ConflictAdded { winner, .. } => {
                    tracing::info!(
                        component = "fs_watcher",
                        kind = %info.kind,
                        key = %info.key,
                        winner = %winner.display(),
                        conflict = %path.display(),
                        "init: collision detected, keeping first scan as winner"
                    );
                }
                RecordOutcome::AlreadyKnown { .. } => {
                    tracing::trace!(
                        component = "fs_watcher",
                        path = %path.display(),
                        "init: walkdir yielded duplicate path (defensive)"
                    );
                }
            }
        }

        self.registry.replace_from(&next_registry);

        let total_files: usize = files_by_kind.values().map(|v| v.len()).sum();
        tracing::info!(
            component = "fs_watcher",
            total_files = total_files,
            kinds = files_by_kind.len(),
            "Prime scan complete"
        );

        *self.primed.lock().unwrap() = Some(files_by_kind);
        Ok(())
    }

    /// Dispatch `Init` -> `InitApply` x N -> `InitDone` for exactly `kinds`,
    /// drawn from the snapshot captured by `prime`. A kind absent from the
    /// snapshot still receives an empty `Init` + `InitDone` so its processor
    /// reaches ready.
    pub async fn dispatch_init(&self, kinds: &[&'static str]) -> Result<()> {
        // Clone the needed slice of the snapshot out from under the lock.
        let per_kind: HashMap<&'static str, Vec<(PathBuf, String)>> = {
            let guard = self.primed.lock().unwrap();
            let snap = guard
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("dispatch_init called before prime"))?;
            kinds
                .iter()
                .filter_map(|k| snap.get(*k).map(|v| (*k, v.clone())))
                .collect()
        };

        let senders = self.senders.read().await;
        for kind in kinds {
            let Some(sender) = senders.get(*kind) else {
                continue;
            };
            let _ = sender.send(FileSystemEvent::Init);
            if let Some(files) = per_kind.get(kind) {
                for (path, content) in files {
                    let _ = sender.send(FileSystemEvent::InitApply {
                        path: path.clone(),
                        content: content.clone(),
                    });
                }
            }
            let _ = sender.send(FileSystemEvent::InitDone);
        }
        Ok(())
    }

    /// One-shot init used by unit tests: prime the scan, then dispatch to every
    /// currently-subscribed kind. Production uses `prime` + phased `dispatch_init`.
    pub async fn init_phase(&self) -> Result<()> {
        self.prime().await?;
        let kinds: Vec<&'static str> = self.senders.read().await.keys().copied().collect();
        self.dispatch_init(&kinds).await
    }

    /// Runtime phase only: watch for changes and dispatch Apply/Delete events.
    pub async fn run_runtime(&self, mut shutdown_signal: ShutdownSignal) -> Result<()> {
        self.runtime_phase(&mut shutdown_signal).await
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
        // operators can restart to retune.
        let poll_interval = match self.poll_interval_override {
            Some(d) => {
                tracing::info!(
                    component = "fs_watcher",
                    interval_ms = d.as_millis(),
                    "Using overridden PollWatcher interval"
                );
                d
            }
            None => {
                let conf_file_count = count_yaml_files(&self.conf_dir).await;
                let interval = poll_interval_for(conf_file_count);
                if conf_file_count > 10_000 {
                    tracing::warn!(
                        component = "fs_watcher",
                        conf_file_count,
                        interval_secs = interval.as_secs(),
                        "Conf directory has unusually many YAML files; consider partitioning the deployment \
                         or moving the controller to the K8s backend",
                    );
                } else {
                    tracing::info!(
                        component = "fs_watcher",
                        conf_file_count,
                        interval_secs = interval.as_secs(),
                        "Selected PollWatcher interval based on conf size",
                    );
                }
                interval
            }
        };

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
                    .with_compare_contents(true)
                    .with_follow_symlinks(false),
            )
            .context("Failed to create file watcher")?;

            watcher
                .watch(&conf_dir, RecursiveMode::Recursive)
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
    ///
    /// The registry is consulted for every event so that conflict-resolution
    /// rules are enforced at runtime:
    /// - Apply: only dispatched when the path is (or becomes) the winner.
    /// - Delete: if a conflict exists, the first conflict is promoted to winner
    ///   and a synthetic Apply is dispatched instead of a Delete.
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

        let rkey = ResourceKey::new(info.kind.clone(), info.key.clone());

        match action {
            DispatchAction::Apply => match tokio::fs::read_to_string(path).await {
                Ok(content) => match self.registry.record(rkey, path.to_path_buf()) {
                    RecordOutcome::NewWinner { .. } | RecordOutcome::AlreadyKnown { is_winner: true } => {
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
                    RecordOutcome::ConflictAdded { winner, .. } => {
                        tracing::info!(
                            component = "fs_watcher",
                            kind = %info.kind,
                            key = %info.key,
                            winner = %winner.display(),
                            conflict = %path.display(),
                            "Apply ignored: path is a non-winner conflict"
                        );
                    }
                    RecordOutcome::AlreadyKnown { is_winner: false } => {
                        tracing::info!(
                            component = "fs_watcher",
                            kind = %info.kind,
                            key = %info.key,
                            path = %path.display(),
                            "Apply ignored: modify on non-winner conflict path"
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        component = "fs_watcher",
                        path = %path.display(),
                        error = %e,
                        "Failed to read file for Apply event"
                    );
                }
            },
            DispatchAction::Delete => match self.registry.forget(path) {
                ForgetOutcome::Removed { .. } => {
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
                ForgetOutcome::Promoted { new_winner, .. } => match tokio::fs::read_to_string(&new_winner).await {
                    Ok(content) => {
                        let _ = sender.send(FileSystemEvent::Apply {
                            path: new_winner.clone(),
                            content,
                        });
                        tracing::info!(
                            component = "fs_watcher",
                            kind = %info.kind,
                            key = %info.key,
                            promoted_to = %new_winner.display(),
                            "Promoted conflict path to winner; dispatched Apply"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            component = "fs_watcher",
                            path = %new_winner.display(),
                            error = %e,
                            "Failed to read promoted winner content"
                        );
                    }
                },
                ForgetOutcome::ConflictRemoved { .. } => {
                    tracing::info!(
                        component = "fs_watcher",
                        kind = %info.kind,
                        key = %info.key,
                        path = %path.display(),
                        "Removed non-winner conflict; no dispatch"
                    );
                }
                ForgetOutcome::Unknown => {
                    // Common: a deleted file we never tracked (status sidecars, tmp files).
                }
            },
            // Recheck is resolved to Apply or Delete in the block above; this arm is structurally unreachable.
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

/// Count YAML files (excluding `.status`) recursively under `dir`.
///
/// Walks the tree off the async runtime via `spawn_blocking`. Errors and
/// non-yaml entries are silently skipped; this counter only feeds
/// `poll_interval_for` and a failure to count just means we fall back to
/// a 1 s interval, which is the conservative default.
async fn count_yaml_files(dir: &Path) -> usize {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || -> usize {
        let mut count = 0usize;
        for entry in walkdir::WalkDir::new(&dir).follow_links(false) {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            if is_yaml_file(entry.path()) {
                count += 1;
            }
        }
        count
    })
    .await
    .unwrap_or(0)
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

    #[tokio::test]
    async fn init_phase_lex_winner_for_collision() {
        use crate::conf_mgr::conf_center::file_system::path_registry::{PathRegistry, ResourceKey};
        use std::sync::Arc;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let yaml =
            "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: r1\n  namespace: default\n";
        std::fs::write(a.join("HTTPRoute_default_r1.yaml"), yaml).unwrap();
        std::fs::write(b.join("HTTPRoute_default_r1.yaml"), yaml).unwrap();

        let registry = Arc::new(PathRegistry::new());
        let watcher = FileSystemWatcher::new(root, registry.clone(), None);
        watcher.init_phase().await.unwrap();

        let key = ResourceKey::new("HTTPRoute".to_string(), "default/r1".to_string());
        let winner = registry.winner_of(&key).expect("winner exists");
        assert!(
            winner.to_string_lossy().contains("/a/"),
            "expected lex-smallest path to win, got {:?}",
            winner
        );
        let conflicts = registry.snapshot_conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].losers.len(), 1);
    }

    #[tokio::test]
    async fn dispatch_init_is_phased() {
        use super::super::event::FileSystemEvent;
        let dir = tempfile::tempdir().unwrap();
        // One foundation file (Gateway) and one dependent file (HTTPRoute).
        std::fs::write(
            dir.path().join("Gateway_default_gw.yaml"),
            "apiVersion: gateway.networking.k8s.io/v1\nkind: Gateway\nmetadata:\n  name: gw\n  namespace: default\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("HTTPRoute_default_rt.yaml"),
            "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: rt\n  namespace: default\n",
        )
        .unwrap();

        let registry = std::sync::Arc::new(PathRegistry::new());
        let watcher = FileSystemWatcher::new(dir.path().to_path_buf(), registry, None);
        let mut gw_rx = watcher.subscribe("Gateway").await;
        let mut rt_rx = watcher.subscribe("HTTPRoute").await;

        watcher.prime().await.unwrap();
        watcher.dispatch_init(&["Gateway"]).await.unwrap();

        // Gateway got Init -> InitApply -> InitDone.
        assert!(matches!(gw_rx.recv().await.unwrap(), FileSystemEvent::Init));
        assert!(matches!(gw_rx.recv().await.unwrap(), FileSystemEvent::InitApply { .. }));
        assert!(matches!(gw_rx.recv().await.unwrap(), FileSystemEvent::InitDone));
        // HTTPRoute has NOT been dispatched yet.
        assert!(
            rt_rx.try_recv().is_err(),
            "Phase-2 kind must not receive before its dispatch"
        );

        watcher.dispatch_init(&["HTTPRoute"]).await.unwrap();
        assert!(matches!(rt_rx.recv().await.unwrap(), FileSystemEvent::Init));
        assert!(matches!(rt_rx.recv().await.unwrap(), FileSystemEvent::InitApply { .. }));
        assert!(matches!(rt_rx.recv().await.unwrap(), FileSystemEvent::InitDone));
    }

    #[tokio::test]
    async fn second_watcher_generation_replays_existing_resources() {
        use crate::conf_mgr::conf_center::file_system::event::FileSystemEvent;
        use crate::conf_mgr::conf_center::file_system::path_registry::PathRegistry;
        use std::sync::Arc;
        use tempfile::TempDir;

        async fn init_apply_count(watcher: &FileSystemWatcher) -> usize {
            let mut rx = watcher.subscribe("HTTPRoute").await;
            watcher.init_phase().await.unwrap();
            let mut count = 0;
            loop {
                match rx.recv().await.unwrap() {
                    FileSystemEvent::InitApply { .. } => count += 1,
                    FileSystemEvent::InitDone => return count,
                    _ => {}
                }
            }
        }

        let dir = TempDir::new().unwrap();
        let yaml =
            "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: r1\n  namespace: default\n";
        std::fs::write(dir.path().join("HTTPRoute_default_r1.yaml"), yaml).unwrap();
        let registry = Arc::new(PathRegistry::new());

        let first = FileSystemWatcher::new(dir.path().to_path_buf(), registry.clone(), None);
        assert_eq!(init_apply_count(&first).await, 1);

        let second = FileSystemWatcher::new(dir.path().to_path_buf(), registry, None);
        assert_eq!(init_apply_count(&second).await, 1);
    }
}
