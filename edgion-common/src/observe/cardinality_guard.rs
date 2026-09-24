//! Runtime cardinality guard: a `metrics::Recorder` wrapper that caps the number
//! of distinct series (name + labels) registered against the inner recorder.
//!
//! Rationale and coverage boundary: design spec
//! docs/superpowers/specs/2026-07-20-metrics-observability-hardening-design.md §4.4.
//! Covers only metrics that flow through the recorder; `resource_abnormal` is
//! hand-rendered and capped separately. Counts lifetime-distinct keys.

use dashmap::DashSet;
use metrics::{Counter, Gauge, Histogram, Key, KeyName, Level, Metadata, Recorder, SharedString, Unit};
use std::sync::atomic::{AtomicBool, Ordering};

pub const SERIES_REJECTED_TOTAL: &str = "edgion_metrics_series_rejected_total";
pub const CARDINALITY_LIMIT_ACTIVE: &str = "edgion_metrics_cardinality_limit_active";

/// Behavior when the cap is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardinalityMode {
    /// Reject new series past the cap (return a no-op handle). Default.
    #[default]
    Enforce,
    /// Keep registering, but count rejections and raise the alarm gauge.
    Observe,
}

/// A `metrics::Recorder` wrapper that bounds the number of distinct series.
///
/// The two self-metrics (`SERIES_REJECTED_TOTAL`, `CARDINALITY_LIMIT_ACTIVE`)
/// are inserted into `seen` up front, so they permanently occupy 2 of `cap`
/// slots — the effective user budget is `cap - 2`. In `CardinalityMode::Observe`,
/// `seen` keeps growing past `cap` by design: this mode is monitoring-only (it
/// never rejects a series), so there is no memory bound on `seen` in that mode.
pub struct CardinalityGuard<R> {
    inner: R,
    seen: DashSet<Key>,
    cap: usize,
    mode: CardinalityMode,
    breached: AtomicBool,
    rejected: Counter,
    limit_active: Gauge,
}

impl<R: Recorder> CardinalityGuard<R> {
    pub fn new(inner: R, cap: usize, mode: CardinalityMode) -> Self {
        let md = Metadata::new(module_path!(), Level::INFO, Some(module_path!()));
        let rejected_key = Key::from_static_name(SERIES_REJECTED_TOTAL);
        let limit_key = Key::from_static_name(CARDINALITY_LIMIT_ACTIVE);
        let rejected = inner.register_counter(&rejected_key, &md);
        let limit_active = inner.register_gauge(&limit_key, &md);
        limit_active.set(0.0);
        let seen = DashSet::new();
        seen.insert(rejected_key);
        seen.insert(limit_key);
        Self {
            inner,
            seen,
            cap,
            mode,
            breached: AtomicBool::new(false),
            rejected,
            limit_active,
        }
    }

    /// Number of distinct keys seen so far (includes the two self-metrics).
    pub fn series_count(&self) -> usize {
        self.seen.len()
    }

    /// Returns true if `key` may be registered against the inner recorder.
    #[inline]
    fn admit(&self, key: &Key) -> bool {
        if self.seen.contains(key) {
            return true; // already counted; hot fast-path (read-only)
        }
        // Deliberate soft cap: this check-then-insert is not atomic, so
        // concurrent first-registrations of distinct keys can race between
        // the len() check and the insert and let `seen` overshoot `cap` by a
        // few entries. Acceptable for a 10k safety net — exactness isn't the
        // goal, bounding runaway cardinality is.
        if self.seen.len() < self.cap {
            self.seen.insert(key.clone());
            return true;
        }
        self.on_reject();
        match self.mode {
            CardinalityMode::Enforce => false,
            CardinalityMode::Observe => {
                self.seen.insert(key.clone());
                true
            }
        }
    }

    #[cold]
    fn on_reject(&self) {
        // Counts over-cap registration *events*, not distinct series. In enforce
        // mode a rejected key is never admitted into `seen`, so every subsequent
        // emission of that key re-enters here and increments again (a hot bad
        // label inflates this counter); in observe mode the key is admitted after
        // the first reject, so it increments once per distinct over-cap series.
        self.rejected.increment(1);
        if !self.breached.swap(true, Ordering::Relaxed) {
            self.limit_active.set(1.0);
            tracing::error!(
                target: "edgion_metrics",
                cap = self.cap,
                mode = ?self.mode,
                "metrics cardinality cap reached; new series are being rejected (enforce) or flagged (observe)"
            );
        }
    }
}

impl<R: Recorder> Recorder for CardinalityGuard<R> {
    fn describe_counter(&self, k: KeyName, u: Option<Unit>, d: SharedString) {
        self.inner.describe_counter(k, u, d);
    }
    fn describe_gauge(&self, k: KeyName, u: Option<Unit>, d: SharedString) {
        self.inner.describe_gauge(k, u, d);
    }
    fn describe_histogram(&self, k: KeyName, u: Option<Unit>, d: SharedString) {
        self.inner.describe_histogram(k, u, d);
    }
    fn register_counter(&self, key: &Key, md: &Metadata<'_>) -> Counter {
        if self.admit(key) {
            self.inner.register_counter(key, md)
        } else {
            Counter::noop()
        }
    }
    fn register_gauge(&self, key: &Key, md: &Metadata<'_>) -> Gauge {
        if self.admit(key) {
            self.inner.register_gauge(key, md)
        } else {
            Gauge::noop()
        }
    }
    fn register_histogram(&self, key: &Key, md: &Metadata<'_>) -> Histogram {
        if self.admit(key) {
            self.inner.register_histogram(key, md)
        } else {
            Histogram::noop()
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/observe/cardinality_guard_test.rs"]
mod tests;
