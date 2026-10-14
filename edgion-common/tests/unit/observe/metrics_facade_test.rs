use super::*;
use crate::observe::cardinality_guard::CardinalityMode;
use crate::observe::metrics_api::{RecorderConfig, TestRecorder};
use std::time::Duration;
fn recorder(cap: usize) -> TestRecorder {
    TestRecorder::with_clock(RecorderConfig {
        service: "test",
        idle_timeout_secs: 1,
        histogram_buckets: &[],
        cardinality_cap: cap,
        cardinality_mode: CardinalityMode::Enforce,
    })
    .unwrap()
}
#[test]
fn fixed_persists_dynamic_renews_and_cold_absence() {
    let r = recorder(100);
    r.with(|| {
        let fixed = fixed_counter!("edgion_ctx_created_total");
        let dynamic = dynamic_counter!("edgion_lb_degrade_selections_total","service"=>"a");
        fixed.increment(2);
        dynamic.increment(3);
        assert!(r
            .render()
            .contains("edgion_lb_degrade_selections_total{service=\"a\"} 3"));
        r.advance(Duration::from_secs(2));
        let out = r.render();
        assert!(out.contains("edgion_ctx_created_total{service=\"test\"} 2"));
        assert!(!out.contains("edgion_lb_degrade_selections_total"));
        assert!(!out.contains("edgion_ssl_log_dropped_total"));
        dynamic.increment(1);
        assert!(r
            .render()
            .contains("edgion_lb_degrade_selections_total{service=\"a\"} 1"));
    });
}
#[test]
fn fixed_priority_shares_count_and_descriptor_preparation_is_free() {
    let r = recorder(3);
    r.with(|| {
        let a = dynamic_counter!("edgion_lb_degrade_selections_total","service"=>"a");
        let b = dynamic_counter!("edgion_lb_degrade_selections_total","service"=>"b");
        assert_eq!(r.stats().admission_count, 2);
        a.increment(1);
        b.increment(1);
        assert_eq!(r.stats().admission_count, 3);
        fixed_counter!("edgion_ssl_log_dropped_total").increment(1);
        assert_eq!(r.stats().admission_count, 4);
        let out = r.render();
        assert!(out.contains("edgion_ssl_log_dropped_total"));
        assert!(!out.contains("service=\"b\""));
        r.advance(Duration::from_secs(2));
        r.render();
        a.increment(1);
        assert_eq!(r.stats().admission_count, 4);
    });
}
#[test]
fn gauge_overlap_expiry_and_retirement_never_resurrect() {
    let r = recorder(100);
    r.with(|| {
        let key = Key::from_parts(
            "edgion_wasm_shared_data_bytes",
            vec![metrics::Label::new("module", "a")],
        );
        let first = GaugeContribution::new(key.clone());
        let second = GaugeContribution::new(key);
        first.increase(10.);
        second.increase(20.);
        assert!(r
            .render()
            .contains("edgion_wasm_shared_data_bytes{service=\"test\",module=\"a\"} 30"));
        r.advance(Duration::from_secs(2));
        assert!(!r.render().contains("edgion_wasm_shared_data_bytes"));
        first.decrease(5.);
        assert!(r
            .render()
            .contains("edgion_wasm_shared_data_bytes{service=\"test\",module=\"a\"} 25"));
        drop(second);
        assert!(r
            .render()
            .contains("edgion_wasm_shared_data_bytes{service=\"test\",module=\"a\"} 5"));
        r.advance(Duration::from_secs(2));
        r.render();
        drop(first);
        assert!(!r.render().contains("edgion_wasm_shared_data_bytes"));
        assert_eq!(r.stats().gauge_owners, 0);
    });
}

#[test]
fn invalid_preparation_is_explicit_and_does_not_admit() {
    let r = recorder(100);
    r.with(|| {
        let before = r.stats().admission_count;
        assert!(FixedCounter::try_new(Key::from_name("edgion_lb_degrade_selections_total")).is_err());
        assert!(DynamicCounter::try_new(Key::from_name("edgion_ctx_created_total")).is_err());
        assert!(GaugeContribution::try_new(Key::from_name("edgion_ctx_active")).is_err());
        fixed_counter!("edgion_unknown_metric").increment(1);
        assert_eq!(r.stats().admission_count, before);
    });
}

#[test]
fn prepared_dynamic_and_contribution_keep_the_original_runtime() {
    let first = recorder(100);
    let second = recorder(100);
    let (counter, owner) = first.with(|| {
        (
            dynamic_counter!("edgion_wasm_module_released_total","module"=>"runtime-a"),
            GaugeContribution::new(Key::from_parts(
                "edgion_wasm_shared_data_bytes",
                vec![metrics::Label::new("module", "runtime-a")],
            )),
        )
    });
    second.with(|| {
        counter.increment(1);
        owner.increase(7.);
    });
    assert!(first
        .render()
        .contains("edgion_wasm_module_released_total{service=\"test\",module=\"runtime-a\"} 1"));
    assert!(first
        .render()
        .contains("edgion_wasm_shared_data_bytes{service=\"test\",module=\"runtime-a\"} 7"));
    assert_eq!(second.stats().live_epochs, 0);
    std::thread::spawn(move || drop(owner)).join().unwrap();
    assert_eq!(first.stats().gauge_owners, 0);
}
#[test]
fn preinstall_contribution_attaches_its_whole_committed_value_once() {
    let owner = GaugeContribution::new(Key::from_parts(
        "edgion_wasm_shared_data_bytes",
        vec![metrics::Label::new("module", "preinstall")],
    ));
    owner.increase(5.);
    let r = recorder(100);
    r.with(|| {
        owner.increase(2.);
        owner.increase(3.);
    });
    assert!(r
        .render()
        .contains("edgion_wasm_shared_data_bytes{service=\"test\",module=\"preinstall\"} 10"));
    drop(owner);
    assert_eq!(r.stats().gauge_owners, 0);
    assert_eq!(r.stats().attached_gauge_owners, 0);
    assert!(r
        .render()
        .contains("edgion_wasm_shared_data_bytes{service=\"test\",module=\"preinstall\"} 0"));
}
#[test]
fn default_and_empty_bucket_selection_preserve_summary_wire() {
    let r = recorder(100);
    r.with(|| {
        let duration = dynamic_histogram!("edgion_gateway_request_duration_ms","route_name"=>"empty-buckets");
        let wasm = dynamic_histogram!("edgion_wasm_hook_duration_ms","module"=>"summary","phase"=>"request_headers");
        for value in [1., 2., 3.] {
            duration.record(value);
            wasm.record(value);
        }
        let text = r.render();
        for name in ["edgion_gateway_request_duration_ms", "edgion_wasm_hook_duration_ms"] {
            assert!(text.contains(&format!("# TYPE {name} summary")));
            assert!(!text.contains(&format!("{name}_bucket")));
            assert!(text
                .lines()
                .any(|line| line.starts_with(&format!("{name}_count{{")) && line.ends_with(" 3")));
            assert!(text
                .lines()
                .any(|line| line.starts_with(&format!("{name}_sum{{")) && line.ends_with(" 6")));
            assert_eq!(
                text.lines()
                    .filter(|line| line.starts_with(&format!("{name}{{")) && line.contains("quantile="))
                    .count(),
                7
            );
        }
    });
}

#[test]
fn denied_bound_owner_drop_does_not_touch_an_unrelated_recorder() {
    let r = recorder(2);
    let owner = r.with(|| {
        let owner = GaugeContribution::new(Key::from_parts(
            "edgion_wasm_shared_data_bytes",
            vec![metrics::Label::new("module", "denied")],
        ));
        owner.increase(5.);
        assert_eq!(r.stats().gauge_owners, 1);
        assert_eq!(r.stats().attached_gauge_owners, 0);
        owner
    });
    let unrelated = metrics_util::debugging::DebuggingRecorder::new();
    let snapshot = unrelated.snapshotter();
    metrics::with_local_recorder(&unrelated, || drop(owner));
    assert!(
        snapshot.snapshot().into_vec().is_empty(),
        "denied Common owner must not decrement a foreign gauge"
    );
    assert_eq!(r.stats().gauge_owners, 0);
    assert_eq!(r.stats().attached_gauge_owners, 0);
    assert!(!r.render().contains("edgion_wasm_shared_data_bytes"));
}
#[test]
fn rejected_owner_attaches_whole_value_after_another_prechecked_admission_wins() {
    let r = recorder(3);
    r.with(|| {
        let key = Key::from_parts(
            "edgion_wasm_shared_data_bytes",
            vec![metrics::Label::new("module", "racing-admission")],
        );
        let pending = GaugeContribution::new(key.clone());
        let winner = GaugeContribution::new(key.clone());
        let store = crate::observe::metrics_api::dynamic_store().unwrap();
        let prior_permission = store.test_cold_permission(&key);
        dynamic_counter!("edgion_lb_degrade_selections_total","service"=>"fills-cap").increment(1);
        pending.increase(5.);
        assert_eq!(r.stats().attached_gauge_owners, 0);
        assert!(!r.render().contains("edgion_wasm_shared_data_bytes"));
        store.test_commit_prior_permission(&key, prior_permission);
        winner.increase(3.);
        pending.increase(2.);
        assert!(r
            .render()
            .contains("edgion_wasm_shared_data_bytes{service=\"test\",module=\"racing-admission\"} 10"));
        assert_eq!(r.stats().attached_gauge_owners, 2);
        drop(pending);
        assert!(r
            .render()
            .contains("edgion_wasm_shared_data_bytes{service=\"test\",module=\"racing-admission\"} 3"));
        r.advance(Duration::from_secs(2));
        assert!(!r.render().contains("edgion_wasm_shared_data_bytes"));
        drop(winner);
        assert!(!r.render().contains("edgion_wasm_shared_data_bytes"));
        assert_eq!(r.stats().gauge_owners, 0);
        assert_eq!(r.stats().attached_gauge_owners, 0);
        assert_eq!(
            r.stats().admission_count,
            4,
            "the known soft race counts its admitted identity without refunds"
        );
    });
}
#[test]
fn production_dynamic_slots_preserve_cap_zero_and_observe_semantics() {
    for (cap, mode) in [(0, CardinalityMode::Enforce), (2, CardinalityMode::Observe)] {
        let r = TestRecorder::with_clock(RecorderConfig {
            service: "fixture",
            idle_timeout_secs: 1,
            histogram_buckets: &[],
            cardinality_cap: cap,
            cardinality_mode: mode,
        })
        .unwrap();
        r.with(||{
            let counters:Vec<_>=(0..8).map(|index|dynamic_counter!("edgion_lb_degrade_selections_total","service"=>format!("service-{index}"))).collect();
            assert_eq!(r.stats().admission_count,2);
            for counter in &counters{counter.increment(1);}
            assert_eq!(r.stats().admission_count,10);assert_eq!(r.stats().live_epochs,8);
            if mode==CardinalityMode::Observe {assert!(r.render().contains("edgion_metrics_series_rejected_total{service=\"fixture\"} 8"));}
            r.advance(Duration::from_secs(2));r.render();assert_eq!(r.stats().live_epochs,0);assert_eq!(r.stats().admission_count,10);
            for counter in &counters{counter.increment(1);}
            assert_eq!(r.stats().admission_count,10);assert_eq!(r.stats().live_epochs,8);
        });
    }
}
