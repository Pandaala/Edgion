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

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::tls_identity::{ServerIdentity, ServerIdentityHandle};

/// Debounce window for coalescing the burst of filesystem events produced by
/// an atomic Secret-mount `..data` symlink swap (typically several events in
/// quick succession for a single logical update).
const DEBOUNCE: Duration = Duration::from_secs(2);

/// The three file paths backing the active conf_sync server TLS identity.
#[derive(Debug, Clone)]
pub struct CertPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub ca: PathBuf,
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

/// Spawn a background task that watches the parent directories of `paths`'
/// cert/key/CA files and reloads the identity into `handle` on change,
/// debounced by [`DEBOUNCE`]. Stops when `shutdown` is cancelled.
pub fn spawn_server_cert_watcher(paths: CertPaths, handle: Arc<ServerIdentityHandle>, shutdown: CancellationToken) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(8);
    // Bridge notify's sync callback into tokio. Watch parent dirs, non-recursive.
    let dirs: BTreeSet<PathBuf> = [&paths.cert, &paths.key, &paths.ca]
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
    tokio::spawn(async move {
        // The watcher must stay alive for as long as we want to keep receiving
        // events, so it is moved into this task rather than dropped after setup.
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(component = "grpc_server", error = %e, "conf_sync cert watcher failed to start");
                return;
            }
        };
        use notify::Watcher;
        for d in &dirs {
            if let Err(e) = watcher.watch(d, notify::RecursiveMode::NonRecursive) {
                tracing::error!(component = "grpc_server", dir = %d.display(), error = %e, "conf_sync cert watch failed");
            }
        }
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                ev = rx.recv() => {
                    if ev.is_none() { break; }
                    // 2s debounce: coalesce the burst from an atomic Secret-mount swap.
                    tokio::time::sleep(DEBOUNCE).await;
                    while rx.try_recv().is_ok() {}
                    match reload_from_files(&paths, &handle).await {
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
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_sync::conf_server::tls_identity::{ServerIdentity, ServerIdentityHandle};
    use crate::services::conf_sync_ca::Ca;
    use std::sync::Arc;

    // rustls 0.23 requires a process-level CryptoProvider before `ServerConfig::builder()`
    // can be called (from_pems builds a rustls ServerConfig). Races harmlessly with other
    // test modules doing the same `install_default()` — first writer wins.
    fn install_test_crypto_provider() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    #[tokio::test]
    async fn reload_swaps_on_valid_files_and_keeps_old_on_invalid() {
        install_test_crypto_provider();
        let dir = tempfile::tempdir().unwrap();
        let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
        let leaf_a = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let write = |name: &str, content: &str| std::fs::write(dir.path().join(name), content).unwrap();
        write("tls.crt", &leaf_a.cert_pem);
        write("tls.key", leaf_a.key_pem.as_str());
        write("ca.crt", ca.ca_cert_pem());

        let handle = Arc::new(ServerIdentityHandle::new(
            ServerIdentity::from_pems(&leaf_a.cert_pem, &leaf_a.key_pem, ca.ca_cert_pem()).unwrap(),
        ));
        let paths = CertPaths {
            cert: dir.path().join("tls.crt"),
            key: dir.path().join("tls.key"),
            ca: dir.path().join("ca.crt"),
        };

        // Valid replacement -> swapped (longer-lived leaf B)
        let leaf_b = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(14))
            .unwrap();
        write("tls.crt", &leaf_b.cert_pem);
        write("tls.key", leaf_b.key_pem.as_str());
        let before = handle.load().not_after;
        reload_from_files(&paths, &handle).await.unwrap();
        assert!(handle.load().not_after > before);

        // Corrupt cert -> error, old config retained
        write("tls.crt", "garbage");
        let kept = handle.load().not_after;
        assert!(reload_from_files(&paths, &handle).await.is_err());
        assert_eq!(handle.load().not_after, kept);
    }
}
