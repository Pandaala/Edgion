//! Access log module

mod backfill;
mod entry;
mod log_ext;
mod logger;

pub use backfill::{run_backfill, BackfillSummary};
pub use entry::AccessLogEntry;
pub use log_ext::{collect, LogExt};
pub use logger::AccessLogger;

use anyhow::{anyhow, Result};
use edgion_resources::output::StringOutput;
use std::sync::{Arc, OnceLock};

use crate::observe::logs::{create_async_logger, LogType};

static ACCESS_LOGGER: OnceLock<Arc<AccessLogger>> = OnceLock::new();

/// Snapshot of the access-log config the backfill endpoint needs: the ES primary
/// key and the resolved LocalFile fallback path. Captured at `init_access_logger`
/// so the API handler need not re-read gateway config.
#[derive(Clone, Default)]
pub struct AccessLogBackfillConfig {
    /// "namespace/name" of the ES primary, when `output` is ES. None otherwise.
    pub es_key: Option<String>,
    /// Resolved (work_dir-anchored) fallback LocalFile base path, when `fallback`
    /// is a LocalFile with a non-empty path. None otherwise.
    pub fallback_base_path: Option<std::path::PathBuf>,
}

static BACKFILL_CONFIG: OnceLock<AccessLogBackfillConfig> = OnceLock::new();

fn derive_backfill_config(config: &edgion_resources::LogConfig) -> AccessLogBackfillConfig {
    let es_key = match &config.output {
        StringOutput::Es { link_sys_ref } => link_sys_ref.key().ok(),
        _ => None,
    };
    let fallback_base_path = match &config.fallback {
        Some(StringOutput::LocalFile(cfg)) if !cfg.path.is_empty() => {
            Some(edgion_resources::work_dir().resolve(&cfg.path))
        }
        _ => None,
    };
    AccessLogBackfillConfig {
        es_key,
        fallback_base_path,
    }
}

/// Read the backfill config snapshot (None before init).
pub fn get_backfill_config() -> Option<&'static AccessLogBackfillConfig> {
    BACKFILL_CONFIG.get()
}

/// Initialize global AccessLogger from configuration.
///
/// Always sets `ACCESS_LOGGER` exactly once, even when access logging is
/// disabled by configuration (`enabled = false` or empty output path). In
/// the disabled case a no-op `AccessLogger` with zero senders is installed,
/// so downstream callers (e.g. startup listener configuration) can rely on
/// `get_access_logger_unchecked()` being safe after this returns.
pub async fn init_access_logger(config: &edgion_resources::LogConfig) -> Result<()> {
    // Capture what the backfill endpoint needs before installing the logger.
    let _ = BACKFILL_CONFIG.set(derive_backfill_config(config));

    let logger = match create_async_logger(config, LogType::Access).await? {
        Some(logger) => {
            tracing::info!("Global AccessLogger initialized");
            logger
        }
        None => {
            tracing::info!("AccessLogger disabled, installing no-op logger");
            Arc::new(AccessLogger::new())
        }
    };
    ACCESS_LOGGER
        .set(logger)
        .map_err(|_| anyhow!("AccessLogger already initialized"))?;
    Ok(())
}

/// Get global AccessLogger instance.
/// Returns `None` only if called before `init_access_logger`.
pub fn get_access_logger() -> Option<&'static Arc<AccessLogger>> {
    ACCESS_LOGGER.get()
}

/// Get global AccessLogger instance.
///
/// Safe to call any time after `init_access_logger` has returned `Ok`, which
/// is guaranteed by the gateway startup sequence before any listener wiring
/// or request processing begins. When access logging is disabled by config,
/// the returned logger is a no-op (zero senders) and `send` silently drops
/// without touching the `access_log_dropped` metric.
///
/// Panics if `init_access_logger` has not been called yet (programmer error).
pub fn get_access_logger_unchecked() -> &'static Arc<AccessLogger> {
    ACCESS_LOGGER.get().expect("AccessLogger not initialized")
}

#[cfg(test)]
#[path = "../../../tests/unit/observe/access_log/mod_test.rs"]
mod tests;
