//! GeoIP settings from the gateway's local startup config.
//!
//! This is process-wide, node-local configuration (a filesystem path, an update
//! URL) and deliberately does NOT live in the `EdgionGatewayConfig` CRD: the CRD
//! has multiple named instances but the database, its mmap, and the updater task
//! are one per process. Changing any field here requires a restart.

use crate::util::RedactedString;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// GeoIP configuration. All keys are snake_case, matching `GatewayConfig`.
///
/// `deny_unknown_fields` is load-bearing: without it a camelCase typo would
/// deserialize into an all-defaults struct — `db_path: None`, i.e. no database
/// — with no error, leaving every configured geo rule silently unenforced.
///
/// `Debug` is derived, not hand-written: `update_url` is a `RedactedString`,
/// whose own `Debug` always prints `[redacted]` (see
/// `crate::util::redacted`). A hand-written `Debug` here that redacts one
/// named field is exactly the hazard this inversion fixes — the previous
/// version of this struct had one, and a hand-written `Debug` on
/// `GatewayConfig` (this struct's embedding config, `cli/config.rs`) was the
/// *original* instance of the same hazard.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoIpConfig {
    /// Local MMDB path. May not exist at first start; that is the normal
    /// "database not ready" state.
    #[serde(default)]
    pub db_path: Option<PathBuf>,

    /// Update source. Empty/absent => load the local file only, never spawn the updater.
    /// May carry an operator-embedded credential (query-string token, signed
    /// mirror URL) — `RedactedString` keeps it out of `Debug` output by
    /// construction; see the struct doc.
    #[serde(default)]
    pub update_url: Option<RedactedString>,

    /// Name of an env var whose value is sent verbatim as the `Authorization`
    /// header. Credentials never go in the URL and never in a CLI flag.
    #[serde(default)]
    pub update_auth_header_env: Option<String>,

    #[serde(default = "default_update_interval_sec")]
    pub update_interval_sec: u64,

    /// Steady-state retry backoff. Not used before the first successful load,
    /// which uses a short escalating backoff instead.
    #[serde(default = "default_retry_interval_sec")]
    pub retry_interval_sec: u64,

    #[serde(default = "default_connect_timeout_sec")]
    pub connect_timeout_sec: u64,

    /// Whole-cycle deadline for one download attempt. The reqwest client-level
    /// `total_timeout` stays unset: it would abort every cycle for a large
    /// database on a slow link.
    #[serde(default = "default_download_deadline_sec")]
    pub download_deadline_sec: u64,

    /// Abort a download that makes no progress for this long.
    #[serde(default = "default_stall_timeout_sec")]
    pub stall_timeout_sec: u64,

    /// Hard cap enforced while streaming. Defends against an anomalous response;
    /// a City-tier database is tens of megabytes.
    #[serde(default = "default_max_db_bytes")]
    pub max_db_bytes: u64,
}

fn default_update_interval_sec() -> u64 {
    86_400
}
fn default_retry_interval_sec() -> u64 {
    600
}
fn default_connect_timeout_sec() -> u64 {
    10
}
fn default_download_deadline_sec() -> u64 {
    600
}
fn default_stall_timeout_sec() -> u64 {
    60
}
fn default_max_db_bytes() -> u64 {
    268_435_456
}

impl Default for GeoIpConfig {
    fn default() -> Self {
        Self {
            db_path: None,
            update_url: None,
            update_auth_header_env: None,
            update_interval_sec: default_update_interval_sec(),
            retry_interval_sec: default_retry_interval_sec(),
            connect_timeout_sec: default_connect_timeout_sec(),
            download_deadline_sec: default_download_deadline_sec(),
            stall_timeout_sec: default_stall_timeout_sec(),
            max_db_bytes: default_max_db_bytes(),
        }
    }
}

impl GeoIpConfig {
    /// Validate settings that are meaningful only when the background updater
    /// is configured. An absent or empty URL keeps the updater disabled and
    /// leaves the remaining values inert.
    pub(crate) fn validate_updater_settings(&self) -> Result<(), String> {
        if self.update_url.as_deref().unwrap_or("").is_empty() {
            return Ok(());
        }
        if self.update_interval_sec == 0 {
            return Err("geoip.update_interval_sec must be greater than zero when update_url is set".to_string());
        }
        if self.retry_interval_sec == 0 {
            return Err("geoip.retry_interval_sec must be greater than zero when update_url is set".to_string());
        }
        if self.connect_timeout_sec == 0 {
            return Err("geoip.connect_timeout_sec must be greater than zero when update_url is set".to_string());
        }
        if self.download_deadline_sec == 0 {
            return Err("geoip.download_deadline_sec must be greater than zero when update_url is set".to_string());
        }
        if self.stall_timeout_sec == 0 {
            return Err("geoip.stall_timeout_sec must be greater than zero when update_url is set".to_string());
        }
        if self.max_db_bytes == 0 {
            return Err("geoip.max_db_bytes must be greater than zero when update_url is set".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/geoip/config_test.rs"]
mod tests;
