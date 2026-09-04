//! Process-wide GeoIP database provider and background updater.

pub mod config;
pub mod info;
pub mod provider;
pub mod updater;

use std::path::Path;

pub use config::GeoIpConfig;
pub use info::{GeoInfo, GeoLookup};
pub use provider::GeoIpProvider;
#[cfg(test)]
pub use provider::{inject_lookup_fault, LookupFault, LookupFaultGuard, GEO_TEST_LOCK};
pub use updater::GeoIpUpdater;

/// Same-filesystem guard: before spawning the updater, verify the swap
/// directory exists and is writable.
///
/// `updater::tmp_path` places the staging file as a sibling of `db_path`, so by
/// construction they share a parent directory; the assertion below documents
/// that invariant rather than testing anything dynamic. `rename(2)` is only
/// atomic within one filesystem, so if the parent directory cannot be written
/// the hot swap the whole updater design depends on can never succeed — refuse
/// to start rather than spawn a task that spins forever.
fn ensure_swap_directory_ready(db_path: &Path) -> Result<(), String> {
    debug_assert_eq!(
        updater::tmp_path(db_path).parent(),
        db_path.parent(),
        "tmp_path must be a sibling of db_path"
    );

    let dir = db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let meta = std::fs::metadata(dir)
        .map_err(|e| format!("geoip db parent directory {} is not accessible: {e}", dir.display()))?;
    if !meta.is_dir() {
        return Err(format!("geoip db parent path {} is not a directory", dir.display()));
    }

    // A permission-bit read can miss ACLs/ownership edge cases; actually
    // creating and removing a small probe file next to the swap target is the
    // only check that answers the real question ("can the updater rename into
    // this directory").
    let probe = dir.join(format!(".edgion-geoip-write-probe-{}", std::process::id()));
    std::fs::write(&probe, b"probe")
        .map_err(|e| format!("geoip db parent directory {} is not writable: {e}", dir.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Initialize the provider and, when configured, spawn the updater.
///
/// There is no separate master switch: geo lookups are driven entirely by
/// whether a `db_path` is configured, and — at the request level — by whether a
/// `GeoIpLocation` plugin is actually on the chain. An unset `db_path` means no
/// database; an empty `update_url` means no updater task.
///
/// Startup never blocks on the network and never exits on a database problem: a
/// configured-but-missing file WARNs, an unparseable file ERRORs, and the
/// gateway serves on with geo lookups reporting `Unavailable`. Leaving GeoIP
/// unconfigured is a normal, quiet state. The operator's `failOpen` setting
/// (per `GeoIpLocation` plugin instance, at either layer) decides what database
/// unavailability means for that plugin's own traffic decision. There is
/// deliberately no readiness gate here.
pub fn init(cfg: &GeoIpConfig) {
    let provider = GeoIpProvider::global();

    match &cfg.db_path {
        Some(path) => match provider.load_from(path) {
            Ok(()) => {
                crate::observe::metrics::set_geoip_db_loaded(true);
                if let Some(epoch) = provider.build_epoch() {
                    crate::observe::metrics::set_geoip_db_build_epoch(epoch);
                }
            }
            Err(e) if !path.exists() => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "geoip database not present yet; geo lookups will report Unavailable until it appears"
                );
                crate::observe::metrics::set_geoip_db_loaded(false);
            }
            Err(e) => {
                tracing::error!(
                    path = %path.display(),
                    error = %e,
                    "geoip database failed to load; geo lookups will report Unavailable"
                );
                crate::observe::metrics::set_geoip_db_loaded(false);
            }
        },
        None => {
            crate::observe::metrics::set_geoip_db_loaded(false);
        }
    }

    if cfg.update_url.as_deref().unwrap_or("").is_empty() {
        return;
    }

    if let Err(e) = cfg.validate_updater_settings() {
        tracing::error!(error = %e, "geoip updater not started: invalid updater settings");
        return;
    }

    let Some(db_path) = &cfg.db_path else {
        // GeoIpUpdater::new would reject this too, but the message is clearer
        // here since we already know exactly which precondition is missing.
        tracing::error!("geoip updater not started: geoip.db_path is required when update_url is set");
        return;
    };

    if let Err(e) = ensure_swap_directory_ready(db_path) {
        tracing::error!(error = %e, "geoip updater not started: same-filesystem swap guard failed");
        return;
    }

    match GeoIpUpdater::new(cfg.clone(), provider) {
        Ok(u) => u.spawn(),
        Err(e) => {
            tracing::error!(error = %e, "geoip updater not started");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_with_unconfigured_geoip_leaves_provider_unloaded() {
        let _g = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();

        let cfg = GeoIpConfig::default();
        init(&cfg);
        assert!(!GeoIpProvider::global().is_loaded());
    }

    #[test]
    fn init_with_a_missing_db_path_does_not_panic_and_stays_unloaded() {
        let _g = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();

        let cfg = GeoIpConfig {
            db_path: Some("/nonexistent/geo.mmdb".into()),
            ..Default::default()
        };
        init(&cfg);
        assert!(!GeoIpProvider::global().is_loaded());
    }

    /// No `db_path` at all must also not panic, and must not attempt to start
    /// the updater even if `update_url` were set (there is nothing to swap
    /// into).
    #[test]
    fn init_with_no_db_path_and_an_update_url_does_not_panic() {
        let _g = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();

        let cfg = GeoIpConfig {
            db_path: None,
            update_url: Some("https://mirror.internal/geo.mmdb".into()),
            ..Default::default()
        };
        init(&cfg);
        assert!(!GeoIpProvider::global().is_loaded());
    }

    #[test]
    fn swap_directory_guard_accepts_a_writable_existing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("db.mmdb");
        assert!(ensure_swap_directory_ready(&db_path).is_ok());
    }

    #[test]
    fn swap_directory_guard_rejects_a_missing_parent() {
        let db_path = Path::new("/nonexistent/does/not/exist/db.mmdb");
        assert!(ensure_swap_directory_ready(db_path).is_err());
    }

    /// BLOCKER 4 regression: a geoip config with no `db_path` must still
    /// create the `edgion_geoip_db_loaded` gauge (reading 0), not leave the
    /// series absent. An absent series makes the documented alert
    /// `edgion_geoip_db_loaded == 0` match nothing in exactly this
    /// misconfiguration, which is the one case the alert exists to catch.
    #[test]
    fn init_with_no_db_path_still_creates_the_loaded_gauge_at_zero() {
        let _g = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();

        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let cfg = GeoIpConfig {
                db_path: None,
                ..Default::default()
            };
            init(&cfg);
            let rendered = handle.render();
            assert!(
                rendered.contains("edgion_geoip_db_loaded"),
                "gauge must exist even when db_path is unset, got:\n{rendered}"
            );
            assert!(
                rendered.contains("edgion_geoip_db_loaded 0"),
                "gauge must read 0 when db_path is unset, got:\n{rendered}"
            );
        });
    }

    /// `update_url` set but a non-writable parent must fail to spawn
    /// the updater without panicking or loading a database.
    #[test]
    fn init_with_update_url_and_unwritable_parent_does_not_spawn() {
        let _g = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();

        let cfg = GeoIpConfig {
            db_path: Some(std::path::PathBuf::from("/nonexistent/does/not/exist/db.mmdb")),
            update_url: Some("https://mirror.internal/geo.mmdb".into()),
            ..Default::default()
        };
        // Must not panic; the missing db_path already WARNs during load, and
        // the same-filesystem guard then refuses to start the updater.
        init(&cfg);
        assert!(!GeoIpProvider::global().is_loaded());
    }
}
