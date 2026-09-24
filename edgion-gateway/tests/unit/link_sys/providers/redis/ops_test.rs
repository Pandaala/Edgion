use super::*;

#[test]
fn test_generate_lock_value_is_unique() {
    let v1 = generate_lock_value();
    std::thread::sleep(std::time::Duration::from_nanos(1));
    let v2 = generate_lock_value();
    assert_ne!(v1, v2);
}

#[test]
fn test_lock_options_default() {
    let opts = LockOptions::default();
    assert_eq!(opts.ttl, Duration::from_secs(10));
    assert_eq!(opts.max_wait, Duration::from_secs(5));
    assert_eq!(opts.retry_interval, Duration::from_millis(50));
}
