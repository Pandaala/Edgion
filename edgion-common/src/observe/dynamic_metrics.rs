//! Renewable dynamic metric slots. The lifetime admission ledger owns identity;
//! optional accumulation expires independently. No exporter handle escapes.
use super::cardinality_guard::Admission;
use super::metrics_catalog::{definition, MetricType};
use metrics::{Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key};
use metrics_exporter_prometheus::{
    formatting::{write_help_line, write_metric_line, write_type_line},
    Distribution, DistributionBuilder, LabelSet, Matcher,
};
use metrics_util::parse_quantiles;
use parking_lot::{Mutex, RwLock};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// A narrow classic histogram adapter. Upstream `record_many` places a sample
/// into its first matching configured bound, then computes cumulative prefixes.
/// Keep configured order, duplicates and non-finite values unchanged.
struct AtomicHistogram {
    bounds: Vec<f64>,
    bins: Vec<AtomicU64>,
    count: AtomicU64,
    sum: AtomicU64,
}
impl AtomicHistogram {
    fn new(bounds: Vec<f64>) -> Self {
        let bins = bounds.iter().map(|_| AtomicU64::new(0)).collect();
        Self {
            bounds,
            bins,
            count: AtomicU64::new(0),
            sum: AtomicU64::new(0),
        }
    }
    fn record(&self, value: f64) {
        if let Some(index) = self.bounds.iter().position(|bound| value <= *bound) {
            self.bins[index].fetch_add(1, Ordering::Relaxed);
        }
        self.count.fetch_add(1, Ordering::Relaxed);
        GaugeFn::increment(&self.sum, value);
    }
    fn snapshot(&self) -> ClassicSnapshot {
        let mut cumulative = 0u64;
        let buckets = self
            .bounds
            .iter()
            .zip(&self.bins)
            .map(|(bound, bin)| {
                cumulative = cumulative.wrapping_add(bin.load(Ordering::Relaxed));
                (*bound, cumulative)
            })
            .collect();
        ClassicSnapshot {
            buckets,
            count: self.count.load(Ordering::Relaxed),
            sum: f64::from_bits(self.sum.load(Ordering::Relaxed)),
        }
    }
}

enum Value {
    Counter(AtomicU64),
    Gauge(f64),
    Classic(AtomicHistogram),
    Summary(Mutex<Distribution>),
}
struct Epoch {
    value: Value,
    touched: AtomicU64,
}
#[derive(Default)]
struct SlotState {
    epoch: Option<Epoch>,
    contribution: f64,
    owners: usize,
}

/// Stable admitted identity, not a cached accumulation epoch.
pub(crate) struct Slot {
    pub(crate) key: Key,
    kind: MetricType,
    labels: LabelSet,
    state: RwLock<SlotState>,
}

struct ClassicSnapshot {
    buckets: Vec<(f64, u64)>,
    count: u64,
    sum: f64,
}
enum Snapshot {
    Counter(u64),
    Gauge(f64),
    Classic(ClassicSnapshot),
    Summary(Distribution),
}

/// One process-owned adapter; no operation waits for full scrape formatting.
pub(crate) struct DynamicStore {
    admission: OnceLock<Arc<Admission>>,
    started: Instant,
    timeout_nanos: u64,
    distributions: DistributionBuilder,
    service: &'static str,
    descriptions: RwLock<HashMap<&'static str, String>>,
    owner_scopes: AtomicUsize,
    #[cfg(any(test, feature = "test-util"))]
    clock: Option<Arc<AtomicU64>>,
}
impl DynamicStore {
    pub(crate) fn new(service: &'static str, timeout_secs: u64, buckets: &[(Matcher, Vec<f64>)]) -> Self {
        let overrides = buckets.iter().cloned().collect();
        Self {
            admission: OnceLock::new(),
            started: Instant::now(),
            timeout_nanos: Duration::from_secs(timeout_secs).as_nanos().min(u64::MAX as u128) as u64,
            distributions: DistributionBuilder::new(
                parse_quantiles(&[0., 0.5, 0.9, 0.95, 0.99, 0.999, 1.]),
                None,
                None,
                None,
                Some(overrides),
                None,
            ),
            service,
            descriptions: RwLock::new(HashMap::new()),
            owner_scopes: AtomicUsize::new(0),
            #[cfg(any(test, feature = "test-util"))]
            clock: None,
        }
    }
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn set_clock(&mut self, clock: Option<Arc<AtomicU64>>) {
        self.clock = clock;
    }
    pub(crate) fn attach(&self, admission: Arc<Admission>) {
        let _ = self.admission.set(admission);
    }
    fn now(&self) -> u64 {
        #[cfg(any(test, feature = "test-util"))]
        if let Some(clock) = &self.clock {
            return clock.load(Ordering::Relaxed);
        }
        self.started.elapsed().as_nanos().min(u64::MAX as u128) as u64
    }
    pub(crate) fn describe(&self, name: &str, description: String) {
        if let Some(def) = definition(name) {
            self.descriptions.write().insert(def.name, description);
        }
    }
    fn epoch(&self, kind: MetricType, name: &str, now: u64) -> Epoch {
        let value = match kind {
            MetricType::Counter => Value::Counter(AtomicU64::new(0)),
            MetricType::Gauge => Value::Gauge(0.),
            MetricType::Histogram => match self.distributions.get_distribution(name) {
                Distribution::Histogram(hist) => Value::Classic(AtomicHistogram::new(
                    hist.buckets().iter().map(|(bound, _)| *bound).collect(),
                )),
                summary => Value::Summary(Mutex::new(summary)),
            },
        };
        Epoch {
            value,
            touched: AtomicU64::new(now),
        }
    }
    pub(crate) fn resolve(&self, key: &Key) -> Option<Arc<Slot>> {
        let admission = self.admission.get()?;
        admission.dynamic_slot(key, || {
            // The public helper preserves local service-label override semantics.
            let labels = {
                // Type inference obtains upstream's private IndexMap parameter.
                fn labels(key: &Key, service: &str) -> LabelSet {
                    let globals = std::iter::once(("service".to_string(), service.to_string())).collect();
                    LabelSet::from_key_and_global(key, &globals)
                }
                labels(key, self.service)
            };
            let kind = definition(key.name())
                .map(|def| def.kind)
                .unwrap_or(MetricType::Counter);
            Arc::new(Slot {
                key: key.clone(),
                kind,
                labels,
                state: RwLock::new(SlotState::default()),
            })
        })
    }
    fn mutate(&self, slot: &Slot, operation: impl FnOnce(&Value)) {
        let read = slot.state.read();
        if let Some(epoch) = &read.epoch {
            operation(&epoch.value);
            epoch.touched.fetch_max(self.now(), Ordering::Relaxed);
            return;
        }
        drop(read);
        let mut write = slot.state.write();
        let now = self.now();
        let epoch = write
            .epoch
            .get_or_insert_with(|| self.epoch(slot.kind, slot.key.name(), now));
        operation(&epoch.value);
        epoch.touched.fetch_max(now, Ordering::Relaxed);
    }
    fn set(&self, slot: &Slot, value: f64) {
        let mut write = slot.state.write();
        let now = self.now();
        let epoch = write
            .epoch
            .get_or_insert_with(|| self.epoch(MetricType::Gauge, slot.key.name(), now));
        epoch.value = Value::Gauge(value);
        epoch.touched.store(now, Ordering::Relaxed);
    }
    pub(crate) fn owner_created(&self) {
        self.owner_scopes.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn owner_released(&self) {
        self.owner_scopes.fetch_sub(1, Ordering::Relaxed);
    }
    pub(crate) fn contribution(&self, slot: &Slot, delta: f64, attach: bool, retire: bool, conditional: bool) {
        let mut write = slot.state.write();
        write.contribution += delta;
        if attach {
            write.owners += 1;
        }
        if retire {
            write.owners = write.owners.saturating_sub(1);
        }
        // Retirement never brings a deleted export epoch back to life.
        if (retire || conditional) && write.epoch.is_none() {
            return;
        }
        let aggregate = write.contribution;
        let now = self.now();
        let epoch = write
            .epoch
            .get_or_insert_with(|| self.epoch(MetricType::Gauge, slot.key.name(), now));
        epoch.value = Value::Gauge(aggregate);
        epoch.touched.store(now, Ordering::Relaxed);
    }
    fn expiry_candidate(&self, slot: &Slot, now: u64) -> bool {
        self.timeout_nanos > 0
            && slot
                .state
                .read()
                .epoch
                .as_ref()
                .is_some_and(|epoch| now.saturating_sub(epoch.touched.load(Ordering::Relaxed)) >= self.timeout_nanos)
    }
    fn expire(&self, slot: &Slot, now: u64) {
        if self.expiry_candidate(slot, now) {
            self.retire_expired(slot, now);
        }
    }
    /// Exclusive recheck closes the interval after the read-only age precheck.
    fn retire_expired(&self, slot: &Slot, now: u64) {
        let mut write = slot.state.write();
        let retired = if write
            .epoch
            .as_ref()
            .is_some_and(|epoch| now.saturating_sub(epoch.touched.load(Ordering::Relaxed)) >= self.timeout_nanos)
        {
            write.epoch.take()
        } else {
            None
        };
        drop(write);
        // Dense sketch deallocation belongs outside the operation gate.
        drop(retired);
    }
    fn snapshot(&self, slot: &Slot, now: u64) -> Option<Snapshot> {
        self.expire(slot, now);
        let read = slot.state.read();
        match &read.epoch.as_ref()?.value {
            Value::Counter(counter) => Some(Snapshot::Counter(counter.load(Ordering::Relaxed))),
            Value::Gauge(gauge) => Some(Snapshot::Gauge(*gauge)),
            Value::Summary(summary) => Some(Snapshot::Summary(summary.lock().clone())),
            Value::Classic(_) => {
                drop(read);
                // Exclusive capture keeps count/sum/bins from partially observed samples.
                let write = slot.state.write();
                let Value::Classic(hist) = &write.epoch.as_ref()?.value else {
                    return None;
                };
                Some(Snapshot::Classic(hist.snapshot()))
            }
        }
    }
    pub(crate) fn render(&self) -> String {
        let Some(admission) = self.admission.get() else {
            return String::new();
        };
        let now = self.now();
        let mut families: BTreeMap<&str, Vec<(Arc<Slot>, Snapshot)>> = BTreeMap::new();
        for slot in admission.slots() {
            if let Some(snapshot) = self.snapshot(&slot, now) {
                let name = definition(slot.key.name()).map(|def| def.name).unwrap_or("");
                families.entry(name).or_default().push((slot, snapshot));
            }
        }
        let descriptions = self.descriptions.read();
        let mut out = String::new();
        for (name, values) in families {
            if let Some(description) = descriptions.get(name) {
                write_help_line(&mut out, name, None, None, description);
            }
            let kind = match &values[0].1 {
                Snapshot::Counter(_) => "counter",
                Snapshot::Gauge(_) => "gauge",
                Snapshot::Classic(_) => "histogram",
                Snapshot::Summary(_) => "summary",
            };
            write_type_line(&mut out, name, None, None, kind);
            for (slot, snapshot) in values {
                match snapshot {
                    Snapshot::Counter(value) => {
                        write_metric_line::<&str, _>(&mut out, name, None, &slot.labels, None, value, None)
                    }
                    Snapshot::Gauge(value) => {
                        write_metric_line::<&str, _>(&mut out, name, None, &slot.labels, None, value, None)
                    }
                    Snapshot::Classic(hist) => {
                        for (bound, count) in hist.buckets {
                            write_metric_line(
                                &mut out,
                                name,
                                Some("bucket"),
                                &slot.labels,
                                Some(("le", bound)),
                                count,
                                None,
                            );
                        }
                        write_metric_line(
                            &mut out,
                            name,
                            Some("bucket"),
                            &slot.labels,
                            Some(("le", "+Inf")),
                            hist.count,
                            None,
                        );
                        write_metric_line::<&str, _>(&mut out, name, Some("sum"), &slot.labels, None, hist.sum, None);
                        write_metric_line::<&str, _>(
                            &mut out,
                            name,
                            Some("count"),
                            &slot.labels,
                            None,
                            hist.count,
                            None,
                        );
                    }
                    Snapshot::Summary(Distribution::Summary(summary, quantiles, sum)) => {
                        let values = summary.snapshot(quanta::Instant::now());
                        for quantile in quantiles.iter() {
                            write_metric_line(
                                &mut out,
                                name,
                                None,
                                &slot.labels,
                                Some(("quantile", quantile.value())),
                                values.quantile(quantile.value()).unwrap_or(0.),
                                None,
                            );
                        }
                        write_metric_line::<&str, _>(&mut out, name, Some("sum"), &slot.labels, None, sum, None);
                        write_metric_line::<&str, _>(
                            &mut out,
                            name,
                            Some("count"),
                            &slot.labels,
                            None,
                            summary.count(),
                            None,
                        );
                    }
                    Snapshot::Summary(_) => {}
                }
            }
        }
        out
    }
    pub(crate) fn stats(&self) -> DynamicMetricStats {
        let Some(admission) = self.admission.get() else {
            return DynamicMetricStats::default();
        };
        let now = self.now();
        let mut result = DynamicMetricStats {
            admission_count: admission.count(),
            fixed_initialized_count: admission.fixed_count(),
            gauge_owners: self.owner_scopes.load(Ordering::Relaxed),
            ..Default::default()
        };
        for slot in admission.slots() {
            self.expire(&slot, now);
            let read = slot.state.read();
            result.attached_gauge_owners += read.owners;
            if let Some(epoch) = &read.epoch {
                result.live_epochs += 1;
                if self.timeout_nanos > 0
                    && now.saturating_sub(epoch.touched.load(Ordering::Relaxed)) >= self.timeout_nanos
                {
                    result.overdue_epochs += 1;
                }
            }
            result.key_bytes += slot.key.name().len()
                + slot
                    .key
                    .labels()
                    .map(|label| label.key().len() + label.value().len())
                    .sum::<usize>();
            if result.keys.len() < 100 {
                result.keys.push(slot.key.name().to_string());
            }
        }
        result.keys.sort_unstable();
        result
    }
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn dense_summary_snapshot_probe(&self) -> (usize, u64) {
        let key = Key::from_parts(
            "edgion_wasm_hook_duration_ms",
            vec![
                metrics::Label::new("module", "summary-probe"),
                metrics::Label::new("phase", "request_headers"),
            ],
        );
        let slot = self.resolve(&key).expect("summary probe must be admitted");
        let at = quanta::Instant::now();
        let mut bytes = 0;
        self.mutate(&slot, |value| {
            let Value::Summary(distribution) = value else {
                panic!("probe must use summary");
            };
            let mut distribution = distribution.lock();
            for window in 0..3 {
                let time = at + Duration::from_secs(window * 20);
                for index in 0..32768 {
                    let sample = (index as f64 * 0.0002).exp();
                    distribution.record_samples(&[(sample, time), (-sample, time)]);
                }
            }
            if let Distribution::Summary(summary, _, _) = &*distribution {
                bytes = summary.snapshot(at + Duration::from_secs(40)).estimated_size();
            }
        });
        let mut times = Vec::with_capacity(200);
        for _ in 0..200 {
            let start = Instant::now();
            let snapshot = std::hint::black_box(self.snapshot(&slot, self.now()));
            times.push(start.elapsed().as_nanos() as u64);
            // The measured production snapshot released all slot/inner gates.
            drop(snapshot);
        }
        times.sort_unstable();
        (bytes, times[198])
    }
    pub(crate) fn counter(self: &Arc<Self>, key: &Key) -> Counter {
        Counter::from_arc(Arc::new(Reference {
            store: self.clone(),
            key: key.clone(),
            slot: OnceLock::new(),
        }))
    }
    pub(crate) fn gauge(self: &Arc<Self>, key: &Key) -> Gauge {
        Gauge::from_arc(Arc::new(Reference {
            store: self.clone(),
            key: key.clone(),
            slot: OnceLock::new(),
        }))
    }
    pub(crate) fn histogram(self: &Arc<Self>, key: &Key) -> Histogram {
        Histogram::from_arc(Arc::new(Reference {
            store: self.clone(),
            key: key.clone(),
            slot: OnceLock::new(),
        }))
    }
}
/// Intentional lifetime metadata and TTL history are reported separately from
/// overdue export epochs and current contribution owners.
#[derive(Default, serde::Serialize)]
pub struct DynamicMetricStats {
    pub admission_count: usize,
    pub fixed_initialized_count: usize,
    pub live_epochs: usize,
    pub overdue_epochs: usize,
    pub gauge_owners: usize,
    pub attached_gauge_owners: usize,
    pub key_bytes: usize,
    pub keys: Vec<String>,
}
struct Reference {
    store: Arc<DynamicStore>,
    key: Key,
    slot: OnceLock<Arc<Slot>>,
}
impl Reference {
    fn slot(&self) -> Option<&Arc<Slot>> {
        if let Some(slot) = self.slot.get() {
            return Some(slot);
        }
        let slot = self.store.resolve(&self.key)?;
        let _ = self.slot.set(slot);
        self.slot.get()
    }
}
impl CounterFn for Reference {
    fn increment(&self, value: u64) {
        if let Some(slot) = self.slot() {
            self.store.mutate(slot, |state| {
                if let Value::Counter(counter) = state {
                    counter.fetch_add(value, Ordering::Relaxed);
                }
            });
        }
    }
    fn absolute(&self, value: u64) {
        if let Some(slot) = self.slot() {
            self.store.mutate(slot, |state| {
                if let Value::Counter(counter) = state {
                    counter.fetch_max(value, Ordering::Relaxed);
                }
            });
        }
    }
}
impl GaugeFn for Reference {
    fn set(&self, value: f64) {
        if let Some(slot) = self.slot() {
            self.store.set(slot, value);
        }
    }
    // Public dynamic Gauges expose set only; paired state uses contribution owners.
    fn increment(&self, _value: f64) {}
    fn decrement(&self, _value: f64) {}
}
impl HistogramFn for Reference {
    fn record(&self, value: f64) {
        if let Some(slot) = self.slot() {
            self.store.mutate(slot, |state| match state {
                Value::Classic(hist) => hist.record(value),
                Value::Summary(summary) => {
                    if let Distribution::Summary(hist, _, sum) = &mut *summary.lock() {
                        hist.add(value, quanta::Instant::now());
                        *sum += value;
                    }
                }
                _ => {}
            });
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/observe/dynamic_metrics_test.rs"]
mod tests;
