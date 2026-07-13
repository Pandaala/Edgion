//! File-mode hot reload for the conf_sync **client** TLS identity (spec §6 Task 4).
//!
//! Watches the **parent directories** of the active cert/key/CA files, not the
//! file inodes: a Kubernetes Secret mount updates via an atomic `..data`
//! symlink swap, which replaces the directory entries rather than writing into
//! the existing inode, so an inode watch would silently miss every update. A
//! 5-minute periodic re-stat backstop catches inotify events that are dropped
//! during a kubelet remount race.
//!
//! Scope (spec §6 T4): this only covers in-place replacement of the files
//! backing the currently-**active** TLS set (cert/key/CA content rotated under
//! the same paths, e.g. cert-manager renewal or a Secret update). Flipping
//! `conf_sync_security.active` to a different named set, or editing the set
//! list itself, is **not** watched and remains restart-bound.
//!
//! Cold-start behavior is unchanged: absent cert files at boot still fail
//! `ConfigSyncClient::new()` (no tolerance is added). This watcher only reloads
//! an already-configured, successfully-started TLS channel.
//!
//! Reload is validate-before-swap: a corrupt or mismatched replacement is
//! rejected (`validate_identity`) and the previously-installed channel is kept,
//! so a bad write never severs the control-plane connection.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio_util::sync::CancellationToken;

use super::ConfigSyncClient;

/// Debounce window for coalescing the burst of filesystem events produced by
/// an atomic Secret-mount `..data` symlink swap.
const DEBOUNCE: Duration = Duration::from_secs(2);

/// Periodic re-stat interval: a backstop for inotify events missed during a
/// kubelet remount race (spec §6 T4).
const RESTAT_INTERVAL: Duration = Duration::from_secs(300);

/// The three file paths backing the active conf_sync client TLS identity.
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

/// Read the cert/key/CA files and validate the identity (leaf validity + chain
/// to CA + key match) before it is used to build a new channel. Returns the raw
/// PEM bytes on success so the caller can reuse them without a second read.
pub(crate) async fn read_and_validate(paths: &CertPaths) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let cert = tokio::fs::read(&paths.cert).await?;
    let key = tokio::fs::read(&paths.key).await?;
    let ca = tokio::fs::read(&paths.ca).await?;
    edgion_common::grpc_tls::validate_identity(&cert, &key, &ca)?;
    Ok((cert, key, ca))
}

/// Per-file `(mtime, len)` signature used by the periodic re-stat backstop to
/// detect a change even if the inotify event was dropped.
pub(crate) fn stat_signature(paths: &[PathBuf]) -> std::io::Result<Vec<(SystemTime, u64)>> {
    paths
        .iter()
        .map(|p| std::fs::metadata(p).map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len())))
        .collect()
}

/// Spawn a background task that watches the parent directories of `paths`'
/// cert/key/CA files and hot-reloads the client TLS channel on change,
/// debounced by [`DEBOUNCE`], with a [`RESTAT_INTERVAL`] re-stat backstop.
/// Stops when `shutdown` is cancelled. On reload failure the current channel is
/// kept (WARN + `CERT_RELOAD_INVALID` system-error mark).
pub fn spawn_client_cert_watcher(client: Arc<ConfigSyncClient>, paths: CertPaths, shutdown: CancellationToken) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(8);
    // Watch the parent dirs (non-recursive): the Secret-mount `..data` swap
    // replaces directory entries, which an inode watch would miss.
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
    tokio::spawn(async move {
        // The watcher must stay alive for the lifetime of the task, so it is
        // moved in rather than dropped after setup.
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(component = "grpc_client", error = %e, "conf_sync client cert watcher failed to start");
                return;
            }
        };
        use notify::Watcher;
        for d in &dirs {
            if let Err(e) = watcher.watch(d, notify::RecursiveMode::NonRecursive) {
                tracing::error!(component = "grpc_client", dir = %d.display(), error = %e, "conf_sync client cert watch failed");
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
                    if ev.is_none() { break; }
                    // 2s debounce: coalesce the burst from an atomic Secret swap.
                    tokio::time::sleep(DEBOUNCE).await;
                    while rx.try_recv().is_ok() {}
                    // Gate on the same stat-signature comparison as the backstop
                    // below: a successful reload cancels every conf_sync watch
                    // stream and rebuilds the channel, so a no-op filesystem event
                    // (e.g. a directory metadata touch that isn't a content
                    // change) must not trigger a full reconnect storm.
                    let now_sig = stat_signature(&watched).ok();
                    if now_sig == last_sig {
                        tracing::debug!(component = "grpc_client", "conf_sync client cert event with unchanged signature; skipping reload");
                    } else {
                        do_reload(&client).await;
                        last_sig = now_sig;
                    }
                }
                _ = restat.tick() => {
                    // Backstop: reload only when the on-disk signature changed,
                    // so a quiet interval never churns the channel.
                    let now_sig = stat_signature(&watched).ok();
                    if now_sig != last_sig {
                        tracing::info!(component = "grpc_client", "conf_sync client cert re-stat detected a change; reloading");
                        do_reload(&client).await;
                        last_sig = now_sig;
                    }
                }
            }
        }
    });
}

/// Run one reload attempt; log INFO on success, WARN + mark the system error on
/// failure (keeping the current channel).
async fn do_reload(client: &Arc<ConfigSyncClient>) {
    match client.reload_client_tls().await {
        Ok(()) => {
            tracing::info!(
                component = "grpc_client",
                event = "conf_sync_cert_reloaded",
                "conf_sync client cert reloaded from disk"
            );
            edgion_common::observe::system_metrics::clear_system_error(
                edgion_common::observe::system_metrics::components::CONF_SYNC_CLIENT,
                edgion_common::observe::system_metrics::reasons::CERT_RELOAD_INVALID,
            );
        }
        Err(e) => {
            tracing::warn!(
                component = "grpc_client",
                error = %e,
                "conf_sync client cert reload rejected; keeping previous channel"
            );
            edgion_common::observe::system_metrics::mark_system_error(
                edgion_common::observe::system_metrics::components::CONF_SYNC_CLIENT,
                edgion_common::observe::system_metrics::reasons::CERT_RELOAD_INVALID,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal in-test CA/leaf generator (copied verbatim from edgion-common's
    // `validate_identity` tests — edgion-gateway cannot reach that test module).
    fn gen_material(_dir: &std::path::Path) -> (String, String, String) {
        use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair};
        let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut cp = CertificateParams::default();
        cp.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        cp.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "test-ca");
            dn
        };
        let ca_cert = cp.self_signed(&ca_key).unwrap();
        let issuer = Issuer::from_ca_cert_pem(&ca_cert.pem(), ca_key).unwrap();
        let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut lp = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        lp.not_before = time::OffsetDateTime::now_utc() - time::Duration::minutes(5);
        lp.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(30);
        let leaf = lp.signed_by(&leaf_key, &issuer).unwrap();
        (leaf.pem(), leaf_key.serialize_pem(), ca_cert.pem())
    }

    #[tokio::test]
    async fn reload_decision_validates_before_applying() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key, ca) = gen_material(dir.path());
        std::fs::write(dir.path().join("client.crt"), &cert).unwrap();
        std::fs::write(dir.path().join("client.key"), &key).unwrap();
        std::fs::write(dir.path().join("ca.crt"), &ca).unwrap();
        let paths = CertPaths {
            cert: dir.path().join("client.crt"),
            key: dir.path().join("client.key"),
            ca: dir.path().join("ca.crt"),
        };
        assert!(read_and_validate(&paths).await.is_ok());
        std::fs::write(dir.path().join("client.crt"), "garbage").unwrap();
        assert!(read_and_validate(&paths).await.is_err());
    }

    #[test]
    fn restat_detects_mtime_or_len_change() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, "aaa").unwrap();
        let s1 = stat_signature(std::slice::from_ref(&p)).unwrap();
        std::fs::write(&p, "aaaa").unwrap();
        let s2 = stat_signature(std::slice::from_ref(&p)).unwrap();
        assert_ne!(s1, s2);
    }
}
