//! Runtime cardinality guard: a `metrics::Recorder` wrapper that caps the number
//! of distinct series (name + labels) registered against the inner recorder.
//!
//! Metric discipline: skills/03-coding/observability/01-metrics.md.
//! Covers only metrics that flow through the recorder; `resource_abnormal` is
//! hand-rendered and capped separately. Counts lifetime-distinct keys.

use super::dynamic_metrics::Slot;
use super::metrics_catalog::{definition, validate, MetricClass, MetricType};
use dashmap::DashMap;
use metrics::{Counter, Gauge, Histogram, Key, KeyName, Level, Metadata, Recorder, SharedString, Unit};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
/// The two self metrics permanently count toward the lifetime ledger. Valid
/// fixed tuples are always accepted and counted, even above the soft cap;
/// dynamic cold admission uses the remaining shared headroom. Eviction never
/// refunds history. Concurrent admission preserves the existing soft race.
/// Observe mode and cap zero do not promise a memory bound.
pub struct CardinalityGuard<R> {
    inner: R,
    pub(crate) admission: Arc<Admission>,
}

/// Shared lifetime ledger. Dynamic epoch ownership replaces the old set value;
/// expiration never removes a key or refunds admission.
pub(crate) struct Admission {
    seen: DashMap<Key, Option<Arc<Slot>>>,
    cap: usize,
    mode: CardinalityMode,
    breached: AtomicBool,
    rejected: Counter,
    limit_active: Gauge,
}
impl Admission {
    fn permitted(&self, key: &Key) -> bool {
        if self.seen.contains_key(key) {
            return true;
        }
        if definition(key.name()).is_some_and(|def| def.class == MetricClass::Fixed)
            || self.cap == 0
            || self.seen.len() < self.cap
        {
            return true;
        }
        self.rejected.increment(1);
        if !self.breached.swap(true, Ordering::Relaxed) {
            self.limit_active.set(1.);
        }
        self.mode == CardinalityMode::Observe
    }
    fn admit(&self, key: &Key) -> bool {
        if self.seen.contains_key(key) {
            return true;
        }
        if !self.permitted(key) {
            return false;
        }
        self.seen.entry(key.clone()).or_insert(None);
        true
    }
    pub(crate) fn dynamic_slot(&self, key: &Key, create: impl FnOnce() -> Arc<Slot>) -> Option<Arc<Slot>> {
        if let Some(value) = self.seen.get(key) {
            if let Some(slot) = value.value() {
                return Some(slot.clone());
            }
        }
        if !self.permitted(key) {
            return None;
        }
        let mut entry = self.seen.entry(key.clone()).or_insert(None);
        Some(entry.get_or_insert_with(create).clone())
    }
    pub(crate) fn slots(&self) -> Vec<Arc<Slot>> {
        self.seen.iter().filter_map(|entry| entry.value().clone()).collect()
    }
    pub(crate) fn count(&self) -> usize {
        self.seen.len()
    }
    pub(crate) fn fixed_count(&self) -> usize {
        self.seen
            .iter()
            .filter(|entry| definition(entry.key().name()).is_some_and(|def| def.class == MetricClass::Fixed))
            .count()
    }
}

impl<R: Recorder> CardinalityGuard<R> {
    pub fn new(inner: R, cap: usize, mode: CardinalityMode) -> Self {
        let md = Metadata::new(module_path!(), Level::INFO, Some(module_path!()));
        let rejected_key = Key::from_static_name(SERIES_REJECTED_TOTAL);
        let limit_key = Key::from_static_name(CARDINALITY_LIMIT_ACTIVE);
        let rejected = inner.register_counter(&rejected_key, &md);
        let limit_active = inner.register_gauge(&limit_key, &md);
        limit_active.set(0.0);
        let admission = Arc::new(Admission {
            seen: DashMap::new(),
            cap,
            mode,
            breached: AtomicBool::new(false),
            rejected: rejected.clone(),
            limit_active: limit_active.clone(),
        });
        admission.seen.insert(rejected_key, None);
        admission.seen.insert(limit_key, None);
        Self { inner, admission }
    }

    /// Number of distinct keys seen so far (includes the two self-metrics).
    pub fn series_count(&self) -> usize {
        self.admission.seen.len()
    }

    fn admit(&self, key: &Key, kind: MetricType) -> bool {
        if let Some(def) = definition(key.name()) {
            if !validate(key, kind, def.class) {
                return false;
            }
            if def.class == MetricClass::Dynamic {
                return true;
            }
        }
        self.admission.admit(key)
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
        if self.admit(key, MetricType::Counter) {
            self.inner.register_counter(key, md)
        } else {
            Counter::noop()
        }
    }
    fn register_gauge(&self, key: &Key, md: &Metadata<'_>) -> Gauge {
        if self.admit(key, MetricType::Gauge) {
            self.inner.register_gauge(key, md)
        } else {
            Gauge::noop()
        }
    }
    fn register_histogram(&self, key: &Key, md: &Metadata<'_>) -> Histogram {
        if self.admit(key, MetricType::Histogram) {
            self.inner.register_histogram(key, md)
        } else {
            Histogram::noop()
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/observe/cardinality_guard_test.rs"]
mod tests;
