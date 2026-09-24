use super::*;
use std::thread;

#[test]
fn generates_monotonic_versions() {
    let first = next_resource_version();
    let second = next_resource_version();
    assert!(second > first);
}

#[test]
fn concurrent_calls_are_unique_and_ordered() {
    let handles: Vec<_> = (0..64).map(|_| thread::spawn(next_resource_version)).collect();

    let mut values: Vec<u64> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread panicked"))
        .collect();

    values.sort_unstable();
    values.dedup();
    assert_eq!(values.len(), 64);

    for window in values.windows(2) {
        assert!(window[1] > window[0]);
    }
}

#[test]
fn bump_resource_version_floor_raises_floor() {
    let baseline = next_resource_version();
    let target = baseline + 1_000_000;
    bump_resource_version_floor(target);
    let next = next_resource_version();
    assert!(
        next > target,
        "next_resource_version() should be strictly greater than the bumped floor (next={}, target={})",
        next,
        target
    );
}

#[test]
fn bump_resource_version_floor_no_regression() {
    let baseline = next_resource_version();
    let high = baseline + 2_000_000;
    let low = baseline + 1_000_000;
    bump_resource_version_floor(high);
    bump_resource_version_floor(low);
    let next = next_resource_version();
    assert!(
        next > high,
        "a lower bump must not roll the floor back (next={}, high={})",
        next,
        high
    );
}
