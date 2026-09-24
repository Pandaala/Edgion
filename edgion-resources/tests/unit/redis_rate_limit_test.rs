use super::*;

#[test]
fn policy_and_call_boundaries() {
    let max_window = Duration::from_millis(MAX_RATE_LIMIT_WINDOW_MS);
    assert_eq!(
        validate_rate_limit_policy(1_000_000, max_window),
        Ok(i64::from(i32::MAX))
    );
    assert!(validate_rate_limit_policy(1_000_001, max_window).is_err());
    assert!(validate_rate_limit_policy(0, max_window).is_err());
    for window in [
        Duration::ZERO,
        Duration::from_millis(99),
        max_window + Duration::from_millis(1),
        Duration::MAX,
    ] {
        assert!(validate_rate_limit_policy(1, window).is_err());
    }
    assert!(validate_rate_limit_policy(1, Duration::from_micros(100_001)).is_err());
    assert_eq!(validate_rate_limit_policy(1, Duration::from_millis(100)), Ok(100));
    assert_eq!(
        validate_rate_limit_call(1_000_000, MAX_SAFE_LUA_INTEGER as i64),
        Ok(1_000_000)
    );
    for cost in [i64::MIN, -1, 0, 1_000_001, i64::MAX] {
        assert!(validate_rate_limit_call(cost, 0).is_err());
    }
    assert!(validate_rate_limit_call(1, -1).is_err());
    assert!(validate_rate_limit_call(1, MAX_SAFE_LUA_INTEGER as i64 + 1).is_err());
}

#[test]
fn maximum_derived_values_stay_exact_and_representable() {
    let units = u64::from(MAX_RATE_LIMIT_UNITS);
    let product = units.checked_mul(MAX_RATE_LIMIT_WINDOW_MS).unwrap();
    let ttl_ms = MAX_RATE_LIMIT_WINDOW_MS.checked_mul(2).unwrap();
    let rate_plus_cost = units.checked_add(units).unwrap();
    let local_capacity = units.checked_mul(1_000).unwrap();

    assert!(product < (1 << 51));
    assert!(product < MAX_SAFE_LUA_INTEGER);
    assert!(ttl_ms < u64::try_from(i64::MAX).unwrap());
    assert!(rate_plus_cost < MAX_SAFE_LUA_INTEGER);
    assert_eq!(local_capacity, 1_000_000_000);
}
