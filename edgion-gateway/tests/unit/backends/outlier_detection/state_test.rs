use super::*;
use std::sync::{Arc, Barrier};

fn config() -> OutlierDetectionResolved {
    OutlierDetectionResolved {
        consecutive_errors: 1,
        consecutive_gateway_errors: 1,
        consecutive_local_origin_failures: 1,
        ejection_seconds: 1,
        max_ejection_seconds: 3,
        max_ejection_percent: 100,
    }
}

#[test]
fn exactly_one_half_open_probe_is_admitted() {
    let state = PassiveEjectionState::new(Some(config()));
    state.open_for(Duration::ZERO);
    let barrier = Arc::new(Barrier::new(8));
    let admitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let state = Arc::clone(&state);
            let barrier = Arc::clone(&barrier);
            let admitted = Arc::clone(&admitted);
            std::thread::spawn(move || {
                barrier.wait();
                if let EjectionAdmission::Probe(probe) = state.admit() {
                    admitted.fetch_add(1, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(20));
                    drop(probe);
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(admitted.load(Ordering::Relaxed), 1);
}

#[test]
fn dropped_probe_releases_without_verdict() {
    let state = PassiveEjectionState::new(Some(config()));
    state.open_for(Duration::ZERO);
    let EjectionAdmission::Probe(probe) = state.admit() else {
        panic!("first probe must be admitted");
    };
    drop(probe);
    assert!(matches!(state.admit(), EjectionAdmission::Probe(_)));
}

#[test]
fn probe_success_closes_and_failure_reopens_with_backoff() {
    let state = PassiveEjectionState::new(Some(config()));
    assert_eq!(
        state.record_failure(FailureClass::Gateway, true),
        FailureTransition::Opened
    );
    assert_eq!(state.num_ejections(), 1);
    state.expire_deadlines_for_test();
    let EjectionAdmission::Probe(probe) = state.admit() else {
        panic!("probe must be admitted after the first window");
    };
    probe.failure(FailureClass::Gateway);
    assert_eq!(state.num_ejections(), 2);
    assert!(matches!(state.admit(), EjectionAdmission::Rejected));

    state.expire_deadlines_for_test();
    let EjectionAdmission::Probe(probe) = state.admit() else {
        panic!("probe must be admitted after backoff");
    };
    probe.success();
    assert!(matches!(state.admit(), EjectionAdmission::Closed));
}

#[test]
fn disabled_thresholds_still_support_explicit_cooldown() {
    let state = PassiveEjectionState::new(None);
    assert_eq!(
        state.record_failure(FailureClass::Gateway, true),
        FailureTransition::None
    );
    assert!(matches!(state.admit(), EjectionAdmission::Closed));
    state.open_for(Duration::from_secs(1));
    assert!(matches!(state.admit(), EjectionAdmission::Rejected));
}

#[test]
fn normal_success_preserves_an_explicit_cooldown() {
    let state = PassiveEjectionState::new(Some(config()));
    state.open_for(Duration::from_secs(60));

    assert!(!state.record_success(), "no failure ejection was cleared");
    assert!(matches!(state.admit(), EjectionAdmission::Rejected));
}

#[test]
fn normal_success_does_not_invalidate_an_explicit_half_open_probe() {
    let state = PassiveEjectionState::new(Some(config()));
    state.open_for(Duration::ZERO);
    let EjectionAdmission::Probe(probe) = state.admit() else {
        panic!("expired explicit cooldown must admit one probe");
    };

    assert!(!state.record_success(), "no failure ejection was cleared");
    assert!(matches!(state.admit(), EjectionAdmission::Rejected));

    probe.success();
    assert!(matches!(state.admit(), EjectionAdmission::Closed));
}

#[test]
fn normal_success_still_clears_a_failure_ejection() {
    let state = PassiveEjectionState::new(Some(config()));
    assert_eq!(
        state.record_failure(FailureClass::Gateway, true),
        FailureTransition::Opened
    );

    assert!(state.record_success());
    assert!(matches!(state.admit(), EjectionAdmission::Closed));
}

#[test]
fn stale_failure_candidate_cannot_reopen_after_success() {
    let state = PassiveEjectionState::new(Some(config()));
    let candidate = state
        .record_failure_candidate(FailureClass::Gateway)
        .expect("threshold-one failure must become a candidate");

    assert!(!state.record_success());
    assert_eq!(state.finish_failure(candidate, true), FailureTransition::None);
    assert!(matches!(state.admit(), EjectionAdmission::Closed));
}
