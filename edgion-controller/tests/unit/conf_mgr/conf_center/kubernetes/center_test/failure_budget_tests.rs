use super::*;

#[test]
fn backoff_grows_then_gives_up_after_max() {
    let mut budget = FailureBudget::new();
    let mut last = Duration::ZERO;
    for i in 1..=MAX_CONSECUTIVE_FAILURES {
        match budget.on_failure(false) {
            FailureAction::Retry(d) => {
                assert!(d >= last, "backoff must be non-decreasing (failure {i})");
                last = d;
            }
            FailureAction::GiveUp => panic!("gave up too early at failure {i}"),
        }
    }
    assert!(matches!(budget.on_failure(false), FailureAction::GiveUp));
}

#[test]
fn stable_run_resets_the_counter() {
    let mut budget = FailureBudget::new();
    for _ in 0..MAX_CONSECUTIVE_FAILURES {
        let _ = budget.on_failure(false);
    }
    // One stable run → the next failure counts as the first again.
    assert!(matches!(budget.on_failure(true), FailureAction::Retry(_)));
    assert!(matches!(budget.on_failure(false), FailureAction::Retry(_)));
}

#[test]
fn benign_reset_clears_the_counter() {
    let mut budget = FailureBudget::new();
    for _ in 0..MAX_CONSECUTIVE_FAILURES {
        let _ = budget.on_failure(false);
    }
    budget.reset();
    assert!(matches!(budget.on_failure(false), FailureAction::Retry(_)));
}
