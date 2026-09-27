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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("client.crt");

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
}

fn pemfn_some() -> Option<String> {
    Some(test_cert_pem())
}

fn pemfn_none() -> Option<String> {
    None
}

#[test]
fn load_validity_from_pemfn_source() {
    let v = load_validity(&CertSource::PemFn(pemfn_some)).unwrap();
    assert!(v.not_after > v.not_before);

    let err = load_validity(&CertSource::PemFn(pemfn_none)).unwrap_err();
    assert!(
        err.contains("no certificate currently in service"),
        "expected 'no certificate currently in service' error, got: {err}"
    );
}

#[test]
fn load_validity_reports_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_validity(&CertSource::File(dir.path().join("missing.crt"))).unwrap_err();
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

    let mut state = ExpiryWatchState::default();
    metrics::with_local_recorder(&recorder, || {
        check_and_report(
            sides::SERVER,
            &CertSource::Pem(pem),
            chrono::Utc::now().timestamp(),
            &mut state,
        );
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
    let dir = tempfile::tempdir().unwrap();
    let mut state = ExpiryWatchState {
        last_severity: Some(ExpirySeverity::Critical),
        consecutive_failures: 0,
    };
    check_and_report(
        sides::CLIENT,
        &CertSource::File(dir.path().join("missing.crt")),
        0,
        &mut state,
    );
    assert_eq!(state.consecutive_failures, 1);
    assert_eq!(state.last_severity, Some(ExpirySeverity::Critical));
}

#[test]
fn check_and_report_resets_consecutive_failures_after_a_success() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = PrometheusBuilder::new().build_recorder();
    let mut state = ExpiryWatchState::default();

    metrics::with_local_recorder(&recorder, || {
        check_and_report(
            sides::CLIENT,
            &CertSource::File(dir.path().join("missing.crt")),
            0,
            &mut state,
        );
    });
    assert_eq!(state.consecutive_failures, 1, "first failure should be recorded");

    metrics::with_local_recorder(&recorder, || {
        check_and_report(
            sides::CLIENT,
            &CertSource::Pem(test_cert_pem()),
            chrono::Utc::now().timestamp(),
            &mut state,
        );
    });
    assert_eq!(state.consecutive_failures, 0, "a success must reset the failure streak");
}

// -- Fix 2: log only on severity *transition*, not every hourly tick --

#[test]
fn transition_from_ok_to_warning_escalates_once() {
    assert_eq!(
        transition(Some(ExpirySeverity::Ok), ExpirySeverity::Warning),
        Some(SeverityLogAction::Escalate(ExpirySeverity::Warning))
    );
}

#[test]
fn transition_staying_in_warning_logs_nothing() {
    assert_eq!(transition(Some(ExpirySeverity::Warning), ExpirySeverity::Warning), None);
}

#[test]
fn transition_from_warning_to_critical_escalates() {
    assert_eq!(
        transition(Some(ExpirySeverity::Warning), ExpirySeverity::Critical),
        Some(SeverityLogAction::Escalate(ExpirySeverity::Critical))
    );
}

#[test]
fn transition_from_critical_to_ok_logs_recovery() {
    assert_eq!(
        transition(Some(ExpirySeverity::Critical), ExpirySeverity::Ok),
        Some(SeverityLogAction::Recover(ExpirySeverity::Ok))
    );
}

#[test]
fn transition_from_critical_to_warning_logs_recovery() {
    assert_eq!(
        transition(Some(ExpirySeverity::Critical), ExpirySeverity::Warning),
        Some(SeverityLogAction::Recover(ExpirySeverity::Warning))
    );
}

#[test]
fn transition_first_observation_ok_logs_nothing() {
    // No prior state (freshly spawned watch) is treated as an Ok baseline,
    // so a healthy first check does not log.
    assert_eq!(transition(None, ExpirySeverity::Ok), None);
}

#[test]
fn transition_first_observation_already_in_warning_band_escalates() {
    // A watch that starts up with an already-aging cert should still
    // report it once, even though there's no "previous" state.
    assert_eq!(
        transition(None, ExpirySeverity::Warning),
        Some(SeverityLogAction::Escalate(ExpirySeverity::Warning))
    );
}

#[test]
fn transition_repeated_ok_logs_nothing() {
    assert_eq!(transition(Some(ExpirySeverity::Ok), ExpirySeverity::Ok), None);
}

// -- Fix 3: cold-start-safe failure logging --

#[test]
fn failure_transition_warns_on_the_first_failure() {
    assert_eq!(failure_transition(0), FailureLogAction::WarnColdStart);
}

#[test]
fn failure_transition_errors_on_subsequent_consecutive_failures() {
    assert_eq!(failure_transition(1), FailureLogAction::Error);
    assert_eq!(failure_transition(5), FailureLogAction::Error);
}
