use super::*;

#[test]
fn frames_isolate_nested_and_repeated_invocations() {
    let mut state = RateLimitLocalOutcomes::default();
    assert!(!state.set(0, RateLimitLocalOutcome::Exceeded));
    let outer = state.begin(2);
    assert!(state.set(0, RateLimitLocalOutcome::Exceeded));
    let inner = state.begin(1);
    assert_eq!(state.get(0), RateLimitLocalOutcome::NotEvaluated);
    assert!(!state.set(1, RateLimitLocalOutcome::WithinLimit));
    assert!(state.set(0, RateLimitLocalOutcome::WithinLimit));
    state.end(inner);
    assert_eq!(state.get(0), RateLimitLocalOutcome::Exceeded);
    assert_eq!(state.get(1), RateLimitLocalOutcome::NotEvaluated);
    state.end(outer);
    let again = state.begin(2);
    assert_eq!(state.get(0), RateLimitLocalOutcome::NotEvaluated);
    state.end(again);
    assert!(state.values.is_empty());
    assert_eq!(state.get(0), RateLimitLocalOutcome::NotEvaluated);
}
