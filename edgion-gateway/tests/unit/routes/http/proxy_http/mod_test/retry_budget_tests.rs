use super::retry_budget_allows_next_attempt;

#[test]
fn zero_retries_stops_after_the_initial_attempt() {
    assert!(!retry_budget_allows_next_attempt(1, 0));
}

#[test]
fn two_retries_allow_three_total_attempts() {
    assert!(retry_budget_allows_next_attempt(1, 2));
    assert!(retry_budget_allows_next_attempt(2, 2));
    assert!(!retry_budget_allows_next_attempt(3, 2));
}

#[test]
fn route_schema_maximum_does_not_overflow() {
    let retry_attempts = i32::MAX as u32;
    assert!(retry_budget_allows_next_attempt(retry_attempts, retry_attempts));
    assert!(!retry_budget_allows_next_attempt(retry_attempts + 1, retry_attempts));
}
