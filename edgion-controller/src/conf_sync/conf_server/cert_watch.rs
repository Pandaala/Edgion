//! File-mode hot reload for the conf_sync server TLS identity (spec §6 Task 3).
//!
//! Watches the **parent directories** of the active cert/key/CA files, not the
//! file inodes themselves: a Kubernetes Secret mount updates via an atomic
//! `..data` symlink swap, which replaces the directory entries rather than
//! writing into the existing inode, so an inode watch would silently miss
//! every update.
//!
//! Scope (spec §6 T3): this only covers in-place replacement of the files
//! backing the currently-**active** TLS set (cert/key/CA content rotated
//! under the same paths, e.g. cert-manager renewal or a Secret update).
//! Flipping `conf_sync_security.active` to a different named set, or editing
//! the set list itself, is **not** watched and remains restart-bound in v0.
//!
//! Reload is validate-before-swap: a corrupt or mismatched replacement is
//! rejected and the previously-served identity is kept, so a bad write never
//! takes the server offline or serves a broken handshake.
//!
//! A 5-minute periodic re-stat backstop catches inotify events dropped during a
//! kubelet remount race, and also keeps hot reload alive (at a coarser latency)
//! when the filesystem watcher itself could not be established.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio_util::sync::CancellationToken;

use super::tls_identity::{ServerIdentity, ServerIdentityHandle};

/// Debounce window for coalescing the burst of filesystem events produced by
/// an atomic Secret-mount `..data` symlink swap (typically several events in
/// quick succession for a single logical update).
const DEBOUNCE: Duration = Duration::from_secs(2);

/// Periodic re-stat interval: a backstop for inotify events missed during a
/// kubelet remount race, and the only reload path left if the watcher failed to
/// start. Kept identical to the gateway-side constant in
/// `edgion-gateway/src/conf_sync/conf_client/cert_watch.rs`.
const RESTAT_INTERVAL: Duration = Duration::from_secs(300);

/// The three file paths backing the active conf_sync server TLS identity.
#[derive(Debug, Clone)]
pub struct CertPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub ca: PathBuf,
}

impl CertPaths {
    fn all(&self) -> [PathBuf; 3] {
        [self.cert.clone(), self.key.clone(), self.ca.clone()]
    }
}

/// Per-file `(mtime, len)` signature used by the periodic re-stat backstop to
/// detect a change even if the inotify event was dropped (or the watcher never
/// started).
///
/// Deliberately duplicated from the gateway's sibling watcher
/// (`edgion-gateway/src/conf_sync/conf_client/cert_watch.rs::stat_signature`):
/// the two live in different crates with no shared dependency edge between them,
/// and hoisting a 6-line stat helper into `edgion-common` would buy less than it
/// costs. Keep the two implementations in sync.
pub(crate) fn stat_signature(paths: &[PathBuf]) -> std::io::Result<Vec<(SystemTime, u64)>> {
    paths
        .iter()
        .map(|p| std::fs::metadata(p).map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len())))
        .collect()
}

/// Pure reload decision: reload only when the on-disk signature actually moved.
///
/// Both the notify-event arm and the re-stat backstop arm gate on this, so a
/// no-op filesystem event (e.g. a directory metadata touch) never churns the
/// served identity, and a quiet re-stat interval is free.
fn signature_changed(last: &Option<Vec<(SystemTime, u64)>>, now: &Option<Vec<(SystemTime, u64)>>) -> bool {
    last != now
}

/// Re-read the cert/key/CA files, validate them, and atomically swap the
/// handle on success. On failure the handle is left untouched — the caller
/// (the watcher loop) keeps serving the previously-loaded identity.
pub(crate) async fn reload_from_files(paths: &CertPaths, handle: &ServerIdentityHandle) -> anyhow::Result<()> {
    let cert = tokio::fs::read_to_string(&paths.cert).await?;
    let key = tokio::fs::read_to_string(&paths.key).await?;
    let ca = tokio::fs::read_to_string(&paths.ca).await?;
    let identity = ServerIdentity::from_pems(&cert, &key, &ca)?;
    let serial = identity.serial.clone();
    let not_after = identity.not_after;
    handle.store(identity);
    tracing::info!(
        component = "grpc_server",
        event = "conf_sync_cert_reloaded",
        serial = %serial,
        not_after = %not_after,
        "conf_sync server cert reloaded from disk"
    );
    Ok(())
}

/// Mark the latching `cert_watch_failed` system error: the filesystem watcher is
/// (partially or wholly) unavailable, so hot reload has degraded to the
/// [`RESTAT_INTERVAL`] backstop. Never cleared — we do not retry the watch.
fn mark_watch_failed() {
    edgion_common::observe::system_metrics::mark_system_error(
        edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
        edgion_common::observe::system_metrics::reasons::CERT_WATCH_FAILED,
    );
}

/// Spawn a background task that watches the parent directories of `paths`'
/// cert/key/CA files and reloads the identity into `handle` on change,
/// debounced by [`DEBOUNCE`], with a [`RESTAT_INTERVAL`] re-stat backstop.
/// Stops when `shutdown` is cancelled.
///
/// If the watcher cannot be constructed, or a directory cannot be watched, the
/// task does **not** die: it marks `cert_watch_failed` and keeps running in
/// backstop-only mode, so a rotated cert is still picked up (within one re-stat
/// interval rather than ~2 seconds).
pub fn spawn_server_cert_watcher(paths: CertPaths, handle: Arc<ServerIdentityHandle>, shutdown: CancellationToken) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(8);
    // Bridge notify's sync callback into tokio. Watch parent dirs, non-recursive.
    let dirs: BTreeSet<PathBuf> = paths
        .all()
        .iter()
        .filter_map(|p| p.parent().map(|d| d.to_path_buf()))
        .collect();
    let watcher = {
        let tx = tx.clone();
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            if res.is_ok() {
                let _ = tx.try_send(());
            }
        })
    };
    // Keepalive sender: if the watcher failed to construct, the notify closure
    // (which owns the only other `tx` clone) never exists, so without this the
    // last sender would drop when this function returns and `rx.recv()` would
    // immediately yield `None` — killing the backstop-only mode we just took
    // pains to preserve. Moving a sender into the task guarantees the channel
    // stays open, so the loop only ever exits via `shutdown.cancelled()`.
    let tx_keepalive = tx;
    tokio::spawn(async move {
        let _tx_keepalive = tx_keepalive;
        // The watcher must stay alive for as long as we want to keep receiving
        // events, so it is moved into this task rather than dropped after setup.
        let mut watcher = match watcher {
            Ok(w) => Some(w),
            Err(e) => {
                tracing::error!(
                    component = "grpc_server",
                    error = %e,
                    "conf_sync cert watcher failed to start; cert hot reload has fallen back to the \
                     periodic re-stat backstop (rotations picked up within 5 minutes, not seconds)"
                );
                mark_watch_failed();
                None
            }
        };
        use notify::Watcher;
        if let Some(w) = watcher.as_mut() {
            for d in &dirs {
                if let Err(e) = w.watch(d, notify::RecursiveMode::NonRecursive) {
                    tracing::error!(
                        component = "grpc_server",
                        dir = %d.display(),
                        error = %e,
                        "conf_sync cert watch failed for directory; hot reload for its files has fallen \
                         back to the periodic re-stat backstop"
                    );
                    mark_watch_failed();
                }
            }
        }

        let watched = paths.all();
        let mut last_sig = stat_signature(&watched).ok();
        let mut restat = tokio::time::interval(RESTAT_INTERVAL);
        // The first tick fires immediately; consume it so the backstop only
        // compares against the baseline after a full interval.
        restat.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                ev = rx.recv() => {
                    // Unreachable-but-harmless: `_tx_keepalive` above keeps a sender
                    // alive for the life of this task, so the channel never closes.
                    // Treat a `None` as "nothing to do" rather than breaking, so a
                    // hypothetical future change cannot silently kill the backstop.
                    if ev.is_none() { continue; }
                    // 2s debounce: coalesce the burst from an atomic Secret-mount swap.
                    tokio::time::sleep(DEBOUNCE).await;
                    while rx.try_recv().is_ok() {}
                    // Gate on the same stat-signature comparison as the backstop below,
                    // so a no-op filesystem event never churns the served identity.
                    let now_sig = stat_signature(&watched).ok();
                    if signature_changed(&last_sig, &now_sig) {
                        do_reload(&paths, &handle).await;
                        last_sig = now_sig;
                    } else {
                        tracing::debug!(component = "grpc_server", "conf_sync server cert event with unchanged signature; skipping reload");
                    }
                }
                _ = restat.tick() => {
                    // Backstop: reload only when the on-disk signature changed, so a
                    // quiet interval never churns the identity.
                    let now_sig = stat_signature(&watched).ok();
                    if signature_changed(&last_sig, &now_sig) {
                        tracing::info!(component = "grpc_server", "conf_sync server cert re-stat detected a change; reloading");
                        do_reload(&paths, &handle).await;
                        last_sig = now_sig;
                    }
                }
            }
        }
    });
}

/// Run one reload attempt; clear the reload-invalid mark on success, WARN + mark
/// it on failure (keeping the currently-served identity).
async fn do_reload(paths: &CertPaths, handle: &ServerIdentityHandle) {
    match reload_from_files(paths, handle).await {
        Ok(()) => {
            edgion_common::observe::system_metrics::clear_system_error(
                edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
                edgion_common::observe::system_metrics::reasons::CERT_RELOAD_INVALID,
            );
        }
        Err(e) => {
            tracing::warn!(component = "grpc_server", error = %e, "conf_sync server cert reload rejected; keeping previous identity");
            edgion_common::observe::system_metrics::mark_system_error(
                edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
                edgion_common::observe::system_metrics::reasons::CERT_RELOAD_INVALID,
            );
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_sync/conf_server/cert_watch_test.rs"]
mod tests;
