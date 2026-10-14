//! Component benchmark of the production facade. This makes no gateway QPS claim.
use edgion_common::observe::{
    cardinality_guard::CardinalityMode,
    metrics_api::{RecorderConfig, TestRecorder},
    metrics_facade::{dynamic_counter, dynamic_histogram, fixed_counter},
};
use metrics::Recorder;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    },
    time::Instant,
};
struct CountAlloc;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountAlloc = CountAlloc;
const N: usize = 1_000_000;
fn mean(mut operation: impl FnMut()) -> f64 {
    let start = Instant::now();
    for _ in 0..N {
        operation();
    }
    start.elapsed().as_nanos() as f64 / N as f64
}
#[derive(serde::Serialize)]
struct Percentiles {
    count: usize,
    p50: u64,
    p95: u64,
    p99: u64,
    p999: u64,
    max: u64,
}
fn percentiles(mut values: Vec<u64>) -> Percentiles {
    values.sort_unstable();
    let n = values.len();
    let at = |numerator: usize, denominator: usize| {
        if n == 0 {
            0
        } else {
            values[((n - 1) * numerator / denominator).min(n - 1)]
        }
    };
    Percentiles {
        count: n,
        p50: at(50, 100),
        p95: at(95, 100),
        p99: at(99, 100),
        p999: at(999, 1000),
        max: values.last().copied().unwrap_or(0),
    }
}
#[derive(serde::Serialize)]
struct WriterTiming {
    elapsed_ns: u64,
    overlap_ns: u64,
    latency: Percentiles,
}
#[derive(serde::Serialize)]
struct Trial {
    label: String,
    repeat: usize,
    writers: usize,
    operations: usize,
    wall_ns: u64,
    mean_thread_ns: f64,
    latency: Percentiles,
    overlap_latency: Percentiles,
    outside_latency: Percentiles,
    scrapes: usize,
    scrape_total_ns: u64,
    scrape_min_ns: u64,
    scrape_max_ns: u64,
    first_scrape_ns: Option<u64>,
    last_scrape_ns: Option<u64>,
    writer_timing: Vec<WriterTiming>,
}
fn concurrent(
    label: &str,
    repeat: usize,
    writer_count: usize,
    with_scrapes: bool,
    operation: impl Fn() + Send + Sync + 'static,
    scrape: impl Fn() -> String + Send + 'static,
) -> Trial {
    let operation = Arc::new(operation);
    let barrier = Arc::new(Barrier::new(writer_count + 1 + usize::from(with_scrapes)));
    let origin = Instant::now();
    let mut workers = Vec::new();
    for _ in 0..writer_count {
        let op = operation.clone();
        let ready = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let mut samples = Vec::with_capacity(1000);
            ready.wait();
            let first = Instant::now();
            for index in 0..100_000 {
                let start = Instant::now();
                op();
                if index % 100 == 0 {
                    samples.push((start, Instant::now()));
                }
            }
            (first, Instant::now(), samples)
        }));
    }
    let render = with_scrapes.then(|| {
        let ready = barrier.clone();
        std::thread::spawn(move || {
            let mut intervals = Vec::with_capacity(10);
            ready.wait();
            for _ in 0..10 {
                let start = Instant::now();
                black_box(scrape());
                intervals.push((start, Instant::now()));
            }
            intervals
        })
    });
    barrier.wait();
    let traces: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    let intervals = render.map(|thread| thread.join().unwrap()).unwrap_or_default();
    let mut all = Vec::new();
    let mut overlap = Vec::new();
    let mut outside = Vec::new();
    let mut writer_timing = Vec::new();
    let mut elapsed_sum = 0u64;
    let first = traces.iter().map(|trace| trace.0).min().unwrap();
    let last = traces.iter().map(|trace| trace.1).max().unwrap();
    for (begin, end, samples) in traces {
        let elapsed = end.duration_since(begin).as_nanos() as u64;
        elapsed_sum += elapsed;
        let overlap_ns = intervals
            .iter()
            .map(|(start, finish)| {
                let low = begin.max(*start);
                let high = end.min(*finish);
                if high > low {
                    high.duration_since(low).as_nanos() as u64
                } else {
                    0
                }
            })
            .sum();
        let mut own = Vec::new();
        for (start, end) in samples {
            let duration = end.duration_since(start).as_nanos() as u64;
            own.push(duration);
            all.push(duration);
            if intervals.iter().any(|(low, high)| end > *low && start < *high) {
                overlap.push(duration);
            } else {
                outside.push(duration);
            }
        }
        writer_timing.push(WriterTiming {
            elapsed_ns: elapsed,
            overlap_ns,
            latency: percentiles(own),
        });
    }
    let render_times: Vec<_> = intervals
        .iter()
        .map(|(start, end)| end.duration_since(*start).as_nanos() as u64)
        .collect();
    Trial {
        label: label.into(),
        repeat,
        writers: writer_count,
        operations: writer_count * 100_000,
        wall_ns: last.duration_since(first).as_nanos() as u64,
        mean_thread_ns: elapsed_sum as f64 / (writer_count * 100_000) as f64,
        latency: percentiles(all),
        overlap_latency: percentiles(overlap),
        outside_latency: percentiles(outside),
        scrapes: intervals.len(),
        scrape_total_ns: render_times.iter().sum(),
        scrape_min_ns: render_times.iter().copied().min().unwrap_or(0),
        scrape_max_ns: render_times.iter().copied().max().unwrap_or(0),
        first_scrape_ns: intervals
            .first()
            .map(|entry| entry.0.duration_since(origin).as_nanos() as u64),
        last_scrape_ns: intervals
            .last()
            .map(|entry| entry.1.duration_since(origin).as_nanos() as u64),
        writer_timing,
    }
}
fn hot_key(name: &'static str) -> metrics::Key {
    metrics::Key::from_parts(
        name,
        vec![
            metrics::Label::new("route_name", "hot"),
            metrics::Label::new("protocol", "http"),
            metrics::Label::new("status", "2xx"),
        ],
    )
}
type CounterOperation = Arc<dyn Fn() + Send + Sync>;
type RenderOperation = Arc<dyn Fn() -> String + Send + Sync>;
fn disabled_counter_fixture(production: bool) -> (CounterOperation, RenderOperation) {
    if production {
        let store = Arc::new(
            TestRecorder::new(RecorderConfig {
                service: "bench",
                idle_timeout_secs: 600,
                histogram_buckets: &[],
                cardinality_cap: 10000,
                cardinality_mode: CardinalityMode::Enforce,
            })
            .unwrap(),
        );
        let counter=store.with(||{
            fixed_counter!("edgion_ctx_created_total").increment(0);
            let counter=dynamic_counter!("edgion_backend_requests_total","route_name"=>"hot","protocol"=>"http","status"=>"2xx");counter.increment(1);
            for index in 0..9996{dynamic_counter!("edgion_lb_degrade_selections_total","service"=>format!("route-{index}")).increment(1);}
            counter
        });
        assert_eq!(store.stats().admission_count, 10000);
        black_box(store.render());
        (
            Arc::new(move || counter.increment(black_box(1))),
            Arc::new(move || store.render()),
        )
    } else {
        let recorder = Arc::new(
            PrometheusBuilder::new()
                .add_global_label("service", "bench")
                .build_recorder(),
        );
        let metadata = metrics::Metadata::new("metrics_lifetime_bench", metrics::Level::INFO, None);
        recorder
            .register_counter(
                &metrics::Key::from_name("edgion_metrics_series_rejected_total"),
                &metadata,
            )
            .increment(0);
        recorder
            .register_gauge(
                &metrics::Key::from_name("edgion_metrics_cardinality_limit_active"),
                &metadata,
            )
            .set(0.);
        recorder
            .register_counter(&metrics::Key::from_name("edgion_ctx_created_total"), &metadata)
            .increment(0);
        let counter = recorder.register_counter(&hot_key("edgion_backend_requests_total"), &metadata);
        counter.increment(1);
        for index in 0..9996 {
            recorder
                .register_counter(
                    &metrics::Key::from_parts(
                        "edgion_lb_degrade_selections_total",
                        vec![metrics::Label::new("service", format!("route-{index}"))],
                    ),
                    &metadata,
                )
                .increment(1);
        }
        black_box(recorder.handle().render());
        (
            Arc::new(move || counter.increment(black_box(1))),
            Arc::new(move || recorder.handle().render()),
        )
    }
}
fn main() {
    let mut failures = Vec::new();
    let mut trials = Vec::new();
    let bounds = vec![1., 2., 5., 10., 20., 50., 100., 200., 500., 1000.];
    let matcher = Matcher::Full("edgion_gateway_request_duration_ms".into());
    let overrides = [(matcher.clone(), bounds.clone())];
    let raw = Arc::new(
        PrometheusBuilder::new()
            .add_global_label("service", "bench")
            .set_buckets_for_metric(matcher, &bounds)
            .unwrap()
            .build_recorder(),
    );
    let metadata = metrics::Metadata::new("metrics_lifetime_bench", metrics::Level::INFO, None);
    let raw_counter = raw.register_counter(&hot_key("edgion_backend_requests_total"), &metadata);
    let raw_hist = raw.register_histogram(&hot_key("edgion_gateway_request_duration_ms"), &metadata);
    raw.register_counter(
        &metrics::Key::from_name("edgion_metrics_series_rejected_total"),
        &metadata,
    )
    .increment(0);
    raw.register_gauge(
        &metrics::Key::from_name("edgion_metrics_cardinality_limit_active"),
        &metadata,
    )
    .set(0.);
    let raw_fixed = raw.register_counter(&metrics::Key::from_name("edgion_ctx_created_total"), &metadata);
    raw_fixed.increment(0);
    for index in 0..9995 {
        raw.register_counter(
            &metrics::Key::from_parts(
                "edgion_lb_degrade_selections_total",
                vec![metrics::Label::new("service", format!("route-{index}"))],
            ),
            &metadata,
        )
        .increment(1);
    }
    let production = Arc::new(
        TestRecorder::new(RecorderConfig {
            service: "bench",
            idle_timeout_secs: 600,
            histogram_buckets: &overrides,
            cardinality_cap: 10000,
            cardinality_mode: CardinalityMode::Enforce,
        })
        .unwrap(),
    );
    let (counter,hist,fixed)=production.with(||{
        let counter=dynamic_counter!("edgion_backend_requests_total","route_name"=>"hot","protocol"=>"http","status"=>"2xx");
        let hist=dynamic_histogram!("edgion_gateway_request_duration_ms","route_name"=>"hot","protocol"=>"http","status"=>"2xx");
        let fixed=fixed_counter!("edgion_ctx_created_total");counter.increment(1);hist.record(5.);
        for index in 0..9995{dynamic_counter!("edgion_lb_degrade_selections_total","service"=>format!("route-{index}")).increment(1);}
        (counter,hist,fixed)
    });
    assert_eq!(production.stats().admission_count, 10000);
    let rejected = production.with(|| dynamic_counter!("edgion_lb_degrade_selections_total","service"=>"rejected"));
    rejected.increment(1);
    for repeat in 0..3 {
        let before_fixed = ALLOCATIONS.load(Ordering::Relaxed);
        let raw_fixed_ns = mean(|| raw_fixed.increment(black_box(1)));
        let raw_counter_ns = mean(|| raw_counter.increment(black_box(1)));
        let fixed_ns = mean(|| fixed.increment(black_box(1)));
        let fixed_allocations = ALLOCATIONS.load(Ordering::Relaxed) - before_fixed;
        if fixed_allocations != 0 {
            failures.push(format!("repeat{repeat}: fixed allocations{fixed_allocations}"));
        }
        let before_rejected = ALLOCATIONS.load(Ordering::Relaxed);
        let rejected_ns = mean(|| rejected.increment(black_box(1)));
        let rejected_allocations = ALLOCATIONS.load(Ordering::Relaxed) - before_rejected;
        if rejected_allocations != 0 {
            failures.push(format!("repeat{repeat}: rejected allocations{rejected_allocations}"));
        }
        let raw_route_ns = mean(|| {
            raw_counter.increment(black_box(1));
            raw_hist.record(black_box(5.));
        });
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        let dynamic_ns = mean(|| counter.increment(black_box(1)));
        let route_ns = mean(|| {
            counter.increment(black_box(1));
            hist.record(black_box(5.));
        });
        let warm_allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
        if warm_allocations != 0 {
            failures.push(format!("repeat{repeat}: dynamic allocations{warm_allocations}"));
        }
        let c = counter.clone();
        let h = hist.clone();
        let store = production.clone();
        let production_trial = concurrent(
            "production-paired",
            repeat,
            8,
            true,
            move || {
                c.increment(black_box(1));
                h.record(black_box(5.));
            },
            move || store.render(),
        );
        let c = raw_counter.clone();
        let h = raw_hist.clone();
        let r = raw.clone();
        let raw_trial = concurrent(
            "raw-paired",
            repeat,
            8,
            true,
            move || {
                c.increment(black_box(1));
                h.record(black_box(5.));
            },
            move || r.handle().render(),
        );
        let production_p99 = production_trial.latency.p99;
        let raw_p99 = raw_trial.latency.p99;
        if production_trial.overlap_latency.count == 0 {
            failures.push(format!("repeat{repeat}: paired concurrent evidence missing overlap"));
        }
        trials.push(production_trial);
        trials.push(raw_trial);
        println!("repeat={repeat} fixed_raw_ns={raw_fixed_ns:.2} fixed_facade_ns={fixed_ns:.2} raw_counter_ns={raw_counter_ns:.2} raw_route_ns={raw_route_ns:.2} dynamic_counter_ns={dynamic_ns:.2} route_facade_ns={route_ns:.2} production_p99_ns={production_p99} raw_p99_ns={raw_p99} rejected_ns={rejected_ns:.2} fixed_allocations={fixed_allocations} rejected_allocations={rejected_allocations} warm_allocations={warm_allocations}");
        if fixed_ns > raw_fixed_ns * 1.1 + 5. {
            failures.push(format!("repeat{repeat}: fixed mean gate"));
        }
        if dynamic_ns > 1000. || route_ns > 1000. {
            failures.push(format!("repeat{repeat}: single-writer mean gate"));
        }
        if production_p99 > 20_000 {
            failures.push(format!("repeat{repeat}: pairedp99 {production_p99}>20000"));
        }
    }
    let mut summary_trials = Vec::new();
    let summaries = TestRecorder::new(RecorderConfig {
        service: "bench",
        idle_timeout_secs: 600,
        histogram_buckets: &[],
        cardinality_cap: 100,
        cardinality_mode: CardinalityMode::Enforce,
    })
    .unwrap();
    for repeat in 0..3 {
        let (estimated_bytes, p99) = summaries.dense_summary_snapshot_probe();
        println!(
            "summary_repeat={repeat} dense_merged_estimated_bytes={estimated_bytes} production_snapshot_p99_ns={p99}"
        );
        summary_trials.push(serde_json::json!({"repeat":repeat,"dense_merged_estimated_bytes":estimated_bytes,"production_snapshot_p99_ns":p99}));
        if estimated_bytes < 500_000 {
            failures.push(format!("summaryrepeat{repeat}: incomplete dense fixture"));
        }
        if p99 > 500_000 {
            failures.push(format!("summaryrepeat{repeat}: snapshotp99{p99}>500000"));
        }
    }
    let mut counter_mean_allocations = Vec::new();
    for repeat in 0..3 {
        for writers in [1, 2, 4, 8] {
            for with_scrapes in [false, true] {
                for production in if repeat % 2 == 0 { [false, true] } else { [true, false] } {
                    let (operation, render) = disabled_counter_fixture(production);
                    let before = ALLOCATIONS.load(Ordering::Relaxed);
                    let single_mean = mean(|| operation());
                    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
                    counter_mean_allocations.push(serde_json::json!({"repeat":repeat,"production":production,"writers":writers,"with_scrapes":with_scrapes,"single_mean_ns":single_mean,"warm_allocations":allocations}));
                    if allocations != 0 {
                        failures.push(format!("disabled Counter allocations {allocations}"));
                    }
                    let trial = concurrent(
                        if production {
                            "production-disabled-counter"
                        } else {
                            "raw-disabled-counter"
                        },
                        repeat,
                        writers,
                        with_scrapes,
                        move || operation(),
                        move || render(),
                    );
                    println!("diagnostic repeat={repeat} production={production} writers={writers} scrapes={} p99_ns={} overlap_samples={} overlap_p99_ns={} mean_thread_ns={:.2}",trial.scrapes,trial.latency.p99,trial.overlap_latency.count,trial.overlap_latency.p99,trial.mean_thread_ns);
                    trials.push(trial);
                }
            }
        }
    }
    let counter_over20:Vec<_>=trials.iter().filter(|trial|trial.label=="production-disabled-counter"&&trial.latency.p99>20_000).map(|trial|serde_json::json!({"repeat":trial.repeat,"writers":trial.writers,"scrapes":trial.scrapes,"p99_ns":trial.latency.p99})).collect();
    let receipt = serde_json::json!({"platform":std::env::consts::OS,"arch":std::env::consts::ARCH,"logical_parallelism":std::thread::available_parallelism().map(|count|count.get()).unwrap_or(0),"summary_trials":summary_trials,"original_gate_failures":failures,"counter_only_over20us_diagnostic":counter_over20,"counter_only_acceptance":"pending independent assessment; no standalone reviewed numeric gate","counter_mean_allocations":counter_mean_allocations,"trials":trials});
    if let Some(path) = std::env::args().nth(1) {
        std::fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    }
    println!(
        "original_gate_failures={:?} counter_diagnostic_over20_count={}",
        failures,
        counter_over20.len()
    );
    assert!(
        failures.is_empty(),
        "original performance gates failed; complete receipts retained"
    );
}
