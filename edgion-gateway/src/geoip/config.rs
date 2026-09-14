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
mod tests {
    use super::*;

    #[test]
    fn defaults_leave_geoip_unconfigured_and_safe() {
        let cfg: GeoIpConfig = serde_yaml::from_str("{}").unwrap();
        assert!(cfg.db_path.is_none());
        assert!(cfg.update_url.is_none());
        assert_eq!(cfg.update_interval_sec, 86400);
        assert_eq!(cfg.retry_interval_sec, 600);
        assert_eq!(cfg.max_db_bytes, 268_435_456);
    }

    #[test]
    fn snake_case_keys_parse() {
        let yaml = r#"
db_path: /var/lib/edgion/geoip/db.mmdb
update_url: https://mirror.internal/geo.mmdb
max_db_bytes: 1024
"#;
        let cfg: GeoIpConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.db_path, Some(PathBuf::from("/var/lib/edgion/geoip/db.mmdb")));
        assert_eq!(cfg.max_db_bytes, 1024);
    }

    #[test]
    fn configured_updater_accepts_safe_defaults() {
        let cfg = GeoIpConfig {
            update_url: Some("https://mirror.internal/geo.mmdb".into()),
            ..Default::default()
        };
        assert!(cfg.validate_updater_settings().is_ok());
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn configured_updater_rejects_zero_durations_intervals_and_size_cap() {
        let base = GeoIpConfig {
            update_url: Some("https://mirror.internal/geo.mmdb".into()),
            ..Default::default()
        };
        let cases: &[(&str, fn(&mut GeoIpConfig))] = &[
            ("update_interval_sec", |cfg| cfg.update_interval_sec = 0),
            ("retry_interval_sec", |cfg| cfg.retry_interval_sec = 0),
            ("connect_timeout_sec", |cfg| cfg.connect_timeout_sec = 0),
            ("download_deadline_sec", |cfg| cfg.download_deadline_sec = 0),
            ("stall_timeout_sec", |cfg| cfg.stall_timeout_sec = 0),
            ("max_db_bytes", |cfg| cfg.max_db_bytes = 0),
        ];

        for (field, make_invalid) in cases {
            let mut cfg = base.clone();
            make_invalid(&mut cfg);
            let err = cfg.validate_updater_settings().unwrap_err();
            assert!(err.contains(field), "expected {field} in: {err}");
        }
    }

    #[test]
    fn disabled_updater_leaves_updater_only_values_inert() {
        let cfg = GeoIpConfig {
            update_url: None,
            update_interval_sec: 0,
            retry_interval_sec: 0,
            connect_timeout_sec: 0,
            download_deadline_sec: 0,
            stall_timeout_sec: 0,
            max_db_bytes: 0,
            ..Default::default()
        };
        assert!(cfg.validate_updater_settings().is_ok());

        let cfg = GeoIpConfig {
            update_url: Some("".into()),
            update_interval_sec: 0,
            retry_interval_sec: 0,
            connect_timeout_sec: 0,
            download_deadline_sec: 0,
            stall_timeout_sec: 0,
            max_db_bytes: 0,
            ..Default::default()
        };
        assert!(cfg.validate_updater_settings().is_ok());
    }

    /// A camelCase typo must be a loud error, never a silent all-defaults struct.
    #[test]
    fn camel_case_key_is_rejected() {
        let yaml = "dbPath: /tmp/x.mmdb\n";
        let err = serde_yaml::from_str::<GeoIpConfig>(yaml).unwrap_err();
        assert!(err.to_string().contains("dbPath"), "got: {err}");
    }

    /// Regression test: `update_url` is credential-bearing (may contain a token
    /// for private mirrors) and must be redacted in Debug output. Other fields
    /// are non-sensitive and should print normally. Unlike the previous
    /// version of this test, this no longer depends on a hand-written `Debug`
    /// impl remembering to redact this one field: `GeoIpConfig` derives
    /// `Debug`, and `update_url`'s `RedactedString` type redacts itself — so
    /// this test is really exercising that the field's *type* is
    /// `RedactedString`, not a policy someone could forget to update.
    #[test]
    fn update_url_is_redacted_in_debug() {
        let cfg = GeoIpConfig {
            db_path: Some(PathBuf::from("/var/lib/geoip.mmdb")),
            update_url: Some("https://mirror.internal/db.mmdb?token=secret-token-1234".into()),
            update_auth_header_env: Some("GEOIP_AUTH".to_string()),
            max_db_bytes: 1024,
            ..Default::default()
        };
        let dbg = format!("{:?}", cfg);

        // Credential must never appear in Debug output
        assert!(
            !dbg.contains("secret-token-1234"),
            "update_url credential must be redacted in Debug"
        );
        assert!(
            dbg.contains("[redacted]"),
            "update_url must show [redacted] placeholder"
        );

        // Other fields should be visible
        assert!(dbg.contains("geoip.mmdb"), "db_path should be in Debug");
        assert!(
            dbg.contains("GEOIP_AUTH"),
            "update_auth_header_env (env var name, not secret) should be in Debug"
        );
        assert!(dbg.contains("1024"), "max_db_bytes should be in Debug");
    }
}
