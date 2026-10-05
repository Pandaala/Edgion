//! conf_sync certificate expiry safety net (observability only).
//!
//! Reports the certificate currently in service on each side of
//! the conf_sync channel and (a) exports its `notAfter` as a Prometheus gauge
//! so an alert rule can compute time-to-expiry server-side, and (b) logs
//! WARN/ERROR once the remaining lifetime crosses the thresholds below. The
//! Controller samples hourly; Gateway scrapes supply installed validity directly.
//!
//! This module takes no action on expiry — rotation, plus an emergency
//! rotation net, now live in the controller
//! (`edgion-controller/src/services/conf_sync_ca/rotation.rs`; see
//! `docs/superpowers/specs/2026-07-12-conf-sync-cert-rotation-design.md` for
//! the full design). This module is the observability cross-check layered on
//! top of that rotation pipeline: it catches a silently-stalled rotation from
//! the outside (e.g. the reconciler wedged, a leader-election gap, a file
//! watcher that stopped firing) by watching the certificate that is actually
//! in service, independent of whatever machinery is supposed to be renewing
//! it. Alert rule shape:
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
/// Hourly: cheap (one live identity read + parse), and a 30-day warning window makes
/// sub-hour reaction time pointless.
const CHECK_INTERVAL: Duration = Duration::from_secs(3600);

/// Where the in-service certificate comes from.
///
/// `File` is re-read on every check (kubelet refreshes Secret mounts in
/// place, so the file's content can change under a running process).
///
/// `PartialEq`/`Eq` are only exercised by test assertions on the `Pem`/`File`
/// variants; comparing `PemFn` by function-pointer identity is not relied on
/// for any runtime decision, so the derive's pointer-identity comparison for
/// that variant is intentionally allowed here.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(unpredictable_function_pointer_comparisons)]
pub enum CertSource {
    /// A static certificate snapshot held for the process lifetime. Only
    /// correct when the underlying identity never changes in place (static
    /// snapshots, tests) — anything that can be hot-rotated or hot-reloaded
    /// should use `PemFn` or `File` instead, or this source goes stale and
    /// raises false expiry alarms after the first rotation/reload.
    Pem(String),
    /// A certificate on disk. This observes file contents, not installation;
    /// never use it for a hot-swappable Channel's in-service expiry.
    File(PathBuf),
    /// Reads the certificate currently in service from live process state on
    /// every check (e.g. the controller's hot-swappable server identity);
    /// `None` = no identity currently in service.
    PemFn(fn() -> Option<String>),
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

/// Numeric ordering used to detect escalation vs. recovery between two
/// severities (`Ok < Warning < Critical`).
fn severity_rank(s: ExpirySeverity) -> u8 {
    match s {
        ExpirySeverity::Ok => 0,
        ExpirySeverity::Warning => 1,
        ExpirySeverity::Critical => 2,
    }
}

/// What `check_and_report` should log for a severity change on the success
/// path. A `None` return means the severity is unchanged since the last
/// check and nothing should be logged (this is what turns "log every hourly
/// tick while in band" into "log once per transition").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeverityLogAction {
    /// Severity got worse (`Ok -> Warning`, `Warning -> Critical`, or a
    /// first observation that already starts in `Warning`/`Critical`).
    Escalate(ExpirySeverity),
    /// Severity got better (`Critical -> Warning`, `Critical -> Ok`, or
    /// `Warning -> Ok`) — logged at INFO as a recovery.
    Recover(ExpirySeverity),
}

/// Pure transition function: given the previously observed severity (`None`
/// if this is the first check since the watch started) and the current
/// severity, decide whether `check_and_report` should log and at what
/// action. Kept side-effect-free so it can be unit-tested as a state
/// machine without going through logging or metrics.
pub fn transition(prev: Option<ExpirySeverity>, current: ExpirySeverity) -> Option<SeverityLogAction> {
    let prev_rank = prev.map(severity_rank).unwrap_or(0);
    let current_rank = severity_rank(current);
    match current_rank.cmp(&prev_rank) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => Some(SeverityLogAction::Escalate(current)),
        std::cmp::Ordering::Less => Some(SeverityLogAction::Recover(current)),
    }
}

/// What `check_and_report` should log when `load_validity` fails, based on
/// how many *consecutive* failures have already happened since the last
/// success (or since the watch started).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureLogAction {
    /// The first failure since spawn/last success — under auto-CA cold
    /// start the certificate file may simply not exist yet, so this is
    /// logged at WARN rather than ERROR to avoid a misleading alarm on
    /// every clean process start.
    WarnColdStart,
    /// A second or later consecutive failure — no longer "maybe cold
    /// start", this is a real, persistent problem.
    Error,
}

/// Pure decision function for [`FailureLogAction`], factored out for testing
/// independent of logging/state plumbing.
pub fn failure_transition(consecutive_failures_before_this_one: u32) -> FailureLogAction {
    if consecutive_failures_before_this_one == 0 {
        FailureLogAction::WarnColdStart
    } else {
        FailureLogAction::Error
    }
}

/// Mutable state the background loop carries across checks so
/// `check_and_report` can tell a severity *transition* from a repeat, and a
/// first (possibly cold-start) load failure from a persistent one.
#[derive(Debug, Clone, Default)]
pub struct ExpiryWatchState {
    last_severity: Option<ExpirySeverity>,
    consecutive_failures: u32,
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
        CertSource::PemFn(f) => match f() {
            Some(pem) => parse_leaf_validity(&pem),
            None => Err("no certificate currently in service".to_string()),
        },
    }
}

/// One check: load the cert, export the expiry gauge, and log on severity
/// *transitions* (not on every hourly tick — see [`transition`]).
///
/// Failures to read/parse the cert never panic — the safety net must never
/// take down the process it watches. The first such failure since spawn/last
/// success logs at WARN (auto-CA cold start: the cert file may not exist yet
/// for up to a minute after process start, which is normal); consecutive
/// failures beyond that log at ERROR. See [`failure_transition`].
pub fn check_and_report(side: &'static str, source: &CertSource, now: i64, state: &mut ExpiryWatchState) {
    let validity = match load_validity(source) {
        Ok(v) => v,
        Err(e) => {
            let action = failure_transition(state.consecutive_failures);
            state.consecutive_failures += 1;
            match action {
                FailureLogAction::WarnColdStart => {
                    tracing::warn!(
                        component = "cert_expiry_watch",
                        side = side,
                        error = %e,
                        "conf_sync certificate could not be read/parsed for expiry check; \
                         may be a cold start (e.g. auto-CA identity not provisioned yet) — will retry hourly"
                    );
                }
                FailureLogAction::Error => {
                    tracing::error!(
                        component = "cert_expiry_watch",
                        side = side,
                        error = %e,
                        consecutive_failures = state.consecutive_failures,
                        "conf_sync certificate could not be read/parsed for expiry check"
                    );
                }
            }
            // Deliberately do NOT touch the gauge here: it keeps exporting
            // the last successfully observed `notAfter`, so a stale value
            // keeps the server-side expiry alert armed instead of going
            // blank while the read itself is failing.
            return;
        }
    };
    report_validity(side, validity, now, state);
}

/// Export a preprocessed installed identity and report severity transitions.
/// Gateway scrapes call this without certificate I/O or parsing; the Controller
/// watcher uses the same reporting path after loading its live identity.
pub fn report_validity(side: &'static str, validity: LeafValidity, now: i64, state: &mut ExpiryWatchState) {
    state.consecutive_failures = 0;

    gauge!(CERT_EXPIRY_METRIC, "side" => side).set(validity.not_after as f64);

    let remaining = validity.remaining_seconds(now);
    let remaining_days = remaining / 86_400;
    let current = classify_remaining(remaining);
    let action = transition(state.last_severity, current);
    state.last_severity = Some(current);

    match action {
        None => {}
        Some(SeverityLogAction::Escalate(ExpirySeverity::Warning)) => {
            tracing::warn!(
                component = "cert_expiry_watch",
                side = side,
                remaining_days = remaining_days,
                not_after = validity.not_after,
                "conf_sync certificate expires soon; rotation or restart needed before expiry"
            );
        }
        Some(SeverityLogAction::Escalate(ExpirySeverity::Critical)) => {
            tracing::error!(
                component = "cert_expiry_watch",
                side = side,
                remaining_days = remaining_days,
                not_after = validity.not_after,
                "conf_sync certificate is expired or about to expire; conf_sync will fail at the next TLS handshake"
            );
        }
        Some(SeverityLogAction::Escalate(ExpirySeverity::Ok)) => {
            // Unreachable: `transition` never escalates *to* Ok (rank 0 can't
            // be greater than another rank). No-op if it ever happened.
        }
        Some(SeverityLogAction::Recover(severity)) => {
            tracing::info!(
                component = "cert_expiry_watch",
                side = side,
                remaining_days = remaining_days,
                not_after = validity.not_after,
                recovered_to = ?severity,
                "conf_sync certificate expiry severity recovered"
            );
        }
    }
}

/// Spawn the hourly background watch (never exits; aborts with the runtime).
pub fn spawn_expiry_watch(side: &'static str, source: CertSource) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(CHECK_INTERVAL);
        let mut state = ExpiryWatchState::default();
        loop {
            interval.tick().await;
            let now = chrono::Utc::now().timestamp();
            check_and_report(side, &source, now, &mut state);
        }
    });
}

#[cfg(test)]
#[path = "../../tests/unit/conf_sync/cert_expiry_watch_test.rs"]
mod tests;
