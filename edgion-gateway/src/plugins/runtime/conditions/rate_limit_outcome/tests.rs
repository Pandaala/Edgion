use super::*;

#[test]
fn frames_isolate_nested_and_repeated_invocations() {
    let mut state = RateLimitOutcomes::default();
    assert!(!state.set(0, RateLimitOutcome::Exceeded));
    let outer = state.begin(2);
    assert!(state.set(0, RateLimitOutcome::Exceeded));
    let inner = state.begin(1);
    assert_eq!(state.get(0), RateLimitOutcome::NotEvaluated);
    assert!(!state.set(1, RateLimitOutcome::WithinLimit));
    assert!(state.set(0, RateLimitOutcome::WithinLimit));
    state.end(inner);
    assert_eq!(state.get(0), RateLimitOutcome::Exceeded);
    assert_eq!(state.get(1), RateLimitOutcome::NotEvaluated);
    state.end(outer);
    let again = state.begin(2);
    assert_eq!(state.get(0), RateLimitOutcome::NotEvaluated);
    state.end(again);
    assert!(state.values.is_empty());
    assert_eq!(state.get(0), RateLimitOutcome::NotEvaluated);
}
