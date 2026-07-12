//! conf_sync certificate expiry safety net (observability only).
//!
//! Periodically inspects the certificate currently in service on each side of
//! the conf_sync channel and (a) exports its `notAfter` as a Prometheus gauge
//! so an alert rule can compute time-to-expiry server-side, and (b) logs
//! WARN/ERROR once the remaining lifetime crosses the thresholds below.
//!
//! This module takes no action on expiry — rotation is a separate concern
//! (see `tasks/pending/16-conf-sync-cert-hot-rotation.md`). Alert rule shape:
//!
//! ```text
//! edgion_conf_sync_cert_expiry_timestamp_seconds - time() < 30 * 86400
//! ```

use crate::cert_util::{parse_leaf_validity, LeafValidity};
use metrics::gauge;
use std::path::PathBuf;
use std::time::Duration;

/// Gauge: the in-service conf_sync certificate's `notAfter` (Unix seconds).
pub const CERT_EXPIRY_METRIC: &str = "edgion_conf_sync_cert_expiry_timestamp_seconds";

/// `side` label values (bounded cardinality — never pass free-form strings).
pub mod sides {
    /// Controller-side conf_sync server certificate.
    pub const SERVER: &str = "server";
    /// Gateway-side conf_sync client certificate.
    pub const CLIENT: &str = "client";
}

/// Remaining lifetime below which we WARN.
pub const WARN_THRESHOLD: Duration = Duration::from_secs(30 * 86_400);
/// Remaining lifetime below which we ERROR (also covers already-expired).
pub const ERROR_THRESHOLD: Duration = Duration::from_secs(7 * 86_400);

/// How often the background watch re-checks the certificate.
///
/// Hourly: cheap (one file read + parse), and a 30-day warning window makes
/// sub-hour reaction time pointless.
const CHECK_INTERVAL: Duration = Duration::from_secs(3600);

/// Where the in-service certificate comes from.
///
/// `File` is re-read on every check (kubelet refreshes Secret mounts in
/// place, so the file's content can change under a running process).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertSource {
    /// A certificate held in memory for the process lifetime (controller
    /// auto-CA server identity).
    Pem(String),
    /// A certificate on disk (Secret mount or operator-provided path).
    File(PathBuf),
}

/// Severity of the remaining-lifetime check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpirySeverity {
    Ok,
    /// Remaining lifetime is below [`WARN_THRESHOLD`].
    Warning,
    /// Remaining lifetime is below [`ERROR_THRESHOLD`] or already expired.
    Critical,
}

/// Derive a watchable file source from a resolved conf_sync TLS config.
///
/// Returns `None` when there is nothing meaningful to watch: TLS off
/// (`tls == None`), the `skip_tls` plaintext bypass, or an empty cert path.
pub fn file_source_from_tls(tls: Option<&crate::conf_sync::config::ConfSyncTlsConfig>) -> Option<CertSource> {
    let tls = tls?;
    if tls.skip_tls || tls.cert.is_empty() {
        return None;
    }
    Some(CertSource::File(PathBuf::from(&tls.cert)))
}

/// Classify a remaining lifetime (seconds; negative = expired).
pub fn classify_remaining(remaining_seconds: i64) -> ExpirySeverity {
    if remaining_seconds <= ERROR_THRESHOLD.as_secs() as i64 {
        ExpirySeverity::Critical
    } else if remaining_seconds <= WARN_THRESHOLD.as_secs() as i64 {
        ExpirySeverity::Warning
    } else {
        ExpirySeverity::Ok
    }
}

/// Load and parse the current certificate from `source`.
pub fn load_validity(source: &CertSource) -> Result<LeafValidity, String> {
    match source {
        CertSource::Pem(pem) => parse_leaf_validity(pem),
        CertSource::File(path) => {
            let pem = std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read cert file {}: {e}", path.display()))?;
            parse_leaf_validity(&pem)
        }
    }
}

/// One check: load the cert, export the expiry gauge, and log by severity.
///
/// Failures to read/parse the cert are logged at ERROR but do not panic —
/// the safety net must never take down the process it watches.
pub fn check_and_report(side: &'static str, source: &CertSource, now: i64) {
    let validity = match load_validity(source) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                component = "cert_expiry_watch",
                side = side,
                error = %e,
                "conf_sync certificate could not be read/parsed for expiry check"
            );
            return;
        }
    };

    gauge!(CERT_EXPIRY_METRIC, "side" => side).set(validity.not_after as f64);

    let remaining = validity.remaining_seconds(now);
    let remaining_days = remaining / 86_400;
    match classify_remaining(remaining) {
        ExpirySeverity::Ok => {}
        ExpirySeverity::Warning => {
            tracing::warn!(
                component = "cert_expiry_watch",
                side = side,
                remaining_days = remaining_days,
                not_after = validity.not_after,
                "conf_sync certificate expires soon; rotation or restart needed before expiry"
            );
        }
        ExpirySeverity::Critical => {
            tracing::error!(
                component = "cert_expiry_watch",
                side = side,
                remaining_days = remaining_days,
                not_after = validity.not_after,
                "conf_sync certificate is expired or about to expire; conf_sync will fail at the next TLS handshake"
            );
        }
    }
}

/// Spawn the hourly background watch (never exits; aborts with the runtime).
pub fn spawn_expiry_watch(side: &'static str, source: CertSource) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(CHECK_INTERVAL);
        loop {
            interval.tick().await;
            let now = chrono::Utc::now().timestamp();
            check_and_report(side, &source, now);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;

    const DAY: i64 = 86_400;

    fn tls_config(cert: &str, skip_tls: bool) -> crate::conf_sync::config::ConfSyncTlsConfig {
        crate::conf_sync::config::ConfSyncTlsConfig {
            name: None,
            cert: cert.to_string(),
            key: "/certs/tls.key".to_string(),
            ca: "/certs/ca.crt".to_string(),
            skip_tls,
        }
    }

    #[test]
    fn file_source_from_tls_uses_the_cert_path() {
        let tls = tls_config("/certs/tls.crt", false);
        assert_eq!(
            file_source_from_tls(Some(&tls)),
            Some(CertSource::File(PathBuf::from("/certs/tls.crt")))
        );
    }

    #[test]
    fn file_source_from_tls_skips_plaintext_and_empty() {
        assert_eq!(file_source_from_tls(None), None, "TLS off: nothing to watch");
        let skip = tls_config("/certs/tls.crt", true);
        assert_eq!(file_source_from_tls(Some(&skip)), None, "skip_tls bypass: not watched");
        let empty = tls_config("", false);
        assert_eq!(file_source_from_tls(Some(&empty)), None, "empty cert path: not watched");
    }

    #[test]
    fn classify_remaining_maps_thresholds() {
        assert_eq!(classify_remaining(365 * DAY), ExpirySeverity::Ok);
        assert_eq!(classify_remaining(31 * DAY), ExpirySeverity::Ok);
        assert_eq!(classify_remaining(30 * DAY), ExpirySeverity::Warning);
        assert_eq!(classify_remaining(8 * DAY), ExpirySeverity::Warning);
        assert_eq!(classify_remaining(7 * DAY), ExpirySeverity::Critical);
        assert_eq!(classify_remaining(0), ExpirySeverity::Critical);
        assert_eq!(classify_remaining(-DAY), ExpirySeverity::Critical);
    }

    /// A minimal self-signed cert PEM for parse-path tests.
    ///
    /// rcgen's default validity window is a fixed constant, so tests that need
    /// distinguishable certs must pass an explicit `not_after`.
    fn test_cert_pem_expiring(not_after_ts: i64) -> String {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        params.not_after = ::time::OffsetDateTime::from_unix_timestamp(not_after_ts).unwrap();
        params.self_signed(&key).unwrap().pem()
    }

    fn test_cert_pem() -> String {
        test_cert_pem_expiring(1_900_000_000)
    }

    #[test]
    fn load_validity_from_pem_source() {
        let pem = test_cert_pem();
        let v = load_validity(&CertSource::Pem(pem)).unwrap();
        assert!(v.not_after > v.not_before);
    }

    #[test]
    fn load_validity_from_file_source_rereads_disk() {
        let dir = std::env::temp_dir().join(format!("edgion-cert-watch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client.crt");

        std::fs::write(&path, test_cert_pem_expiring(1_900_000_000)).unwrap();
        let first = load_validity(&CertSource::File(path.clone())).unwrap();
        assert_eq!(first.not_after, 1_900_000_000);

        // Overwrite with a different cert — the next load must see the new one
        // (kubelet refreshes Secret mounts in place).
        std::fs::write(&path, test_cert_pem_expiring(1_950_000_000)).unwrap();
        let second = load_validity(&CertSource::File(path.clone())).unwrap();
        assert_eq!(
            second.not_after, 1_950_000_000,
            "File source must re-read from disk on every check"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_validity_reports_missing_file() {
        let err = load_validity(&CertSource::File(PathBuf::from("/nonexistent/edgion-test.crt"))).unwrap_err();
        assert!(
            err.contains("read"),
            "error should mention the read failure, got: {err}"
        );
    }

    #[test]
    fn check_and_report_exports_not_after_gauge_with_side_label() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        let pem = test_cert_pem();
        let expected_not_after = parse_leaf_validity(&pem).unwrap().not_after;

        metrics::with_local_recorder(&recorder, || {
            check_and_report(sides::SERVER, &CertSource::Pem(pem), chrono::Utc::now().timestamp());
        });

        let rendered = handle.render();
        assert!(
            rendered
                .lines()
                .any(|l| l.contains(CERT_EXPIRY_METRIC) && l.contains("side=\"server\"")),
            "expected gauge with side label, got:\n{rendered}"
        );
        assert!(
            rendered.contains(&format!(" {}", expected_not_after)),
            "expected gauge value {expected_not_after}, got:\n{rendered}"
        );
    }

    #[test]
    fn check_and_report_survives_unreadable_source() {
        // Must not panic — the safety net cannot take down the process.
        check_and_report(
            sides::CLIENT,
            &CertSource::File(PathBuf::from("/nonexistent/edgion-test.crt")),
            0,
        );
    }
}
