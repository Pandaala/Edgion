//! Process-wide GeoIP database provider and background updater.

pub mod config;
pub mod info;
pub mod provider;
pub mod updater;

#[cfg(feature = "geoip")]
use std::path::Path;

#[cfg(not(feature = "geoip"))]
pub(crate) const GEOIP_NOT_COMPILED: &str =
    "GeoIP support is not compiled into this gateway build (cargo feature `geoip`)";

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
#[cfg(feature = "geoip")]
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
    #[cfg(not(feature = "geoip"))]
    {
        crate::observe::metrics::set_geoip_db_loaded(false);
        if cfg.db_path.is_some() || !cfg.update_url.as_deref().unwrap_or("").is_empty() {
            tracing::error!("{GEOIP_NOT_COMPILED}; geo lookups will report Unavailable");
        }
    }
    #[cfg(feature = "geoip")]
    init_enabled(cfg);
}

#[cfg(feature = "geoip")]
fn init_enabled(cfg: &GeoIpConfig) {
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
#[path = "../../tests/unit/geoip/mod_test.rs"]
mod tests;
