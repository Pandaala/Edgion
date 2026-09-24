use super::*;

/// Acquire the lock for an identity via `try_lock_owned` (no wall-clock timing).
/// `None` means "currently held" (contended); `Some(_)` means "acquired".
fn try_acquire(kind: &str, ns: &str, name: &str) -> Option<OwnedMutexGuard<()>> {
    let key = format!("{}/{}/{}", kind, ns, name);
    RESOURCE_WRITE_LOCKS
        .entry(key)
        .or_default()
        .clone()
        .try_lock_owned()
        .ok()
}

#[tokio::test]
async fn same_identity_shares_one_lock_distinct_identity_does_not() {
    // Use unique identities so this test is insensitive to entries left in the
    // process-global map by other tests, and deterministic regardless of the
    // test runtime flavor (no wall-clock timeouts).
    let g1 = lock_resource("EdgionConfigData", "ns-rl-test", "pm").await;

    // Same kind/ns/name must serialize: while g1 is held, a second acquisition
    // for the same identity must fail (None).
    assert!(
        try_acquire("EdgionConfigData", "ns-rl-test", "pm").is_none(),
        "second lock on the same identity must be contended while the first is held"
    );

    // A different name, namespace, or kind must NOT contend (acquires immediately).
    assert!(
        try_acquire("EdgionConfigData", "ns-rl-test", "other").is_some(),
        "distinct name must not contend"
    );
    assert!(
        try_acquire("EdgionConfigData", "other-ns-rl-test", "pm").is_some(),
        "distinct namespace must not contend"
    );
    assert!(
        try_acquire("Service", "ns-rl-test", "pm").is_some(),
        "distinct kind must not contend"
    );

    drop(g1);
    // Once released, the identity is acquirable again.
    assert!(
        try_acquire("EdgionConfigData", "ns-rl-test", "pm").is_some(),
        "lock must be reacquirable after release"
    );
}
