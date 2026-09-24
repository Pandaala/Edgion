use super::*;

#[test]
fn cb_permit_hard_caps_at_max() {
    let b = LbBackend::new(Backend::new("127.0.0.1:1").unwrap());
    assert!(b.try_acquire_cb_permit(2)); // 0 -> 1
    assert!(b.try_acquire_cb_permit(2)); // 1 -> 2
    assert!(!b.try_acquire_cb_permit(2)); // at cap, reject
    assert_eq!(b.cb_inflight(), 2);
    b.release_cb_permit(); // 2 -> 1
    assert!(b.try_acquire_cb_permit(2)); // 1 -> 2 again
}

#[test]
fn cb_permit_release_saturates_at_zero() {
    let b = LbBackend::new(Backend::new("127.0.0.1:1").unwrap());
    b.release_cb_permit(); // stays 0, no wraparound
    assert_eq!(b.cb_inflight(), 0);
}

#[test]
fn stale_wrapper_cannot_decrement_replacement_connection() {
    let counters = Arc::new(BackendRuntimeCounters::default());
    let old = LbBackend {
        inner: Backend::new("127.0.0.1:1").unwrap(),
        counters: Arc::clone(&counters),
        owned_conn_count: AtomicUsize::new(0),
        runtime_identity: None,
        runtime_membership_identity: None,
    };
    let replacement = LbBackend {
        inner: Backend::new("127.0.0.1:1").unwrap(),
        counters,
        owned_conn_count: AtomicUsize::new(0),
        runtime_identity: None,
        runtime_membership_identity: None,
    };

    replacement.inc_conn();
    assert_eq!(replacement.conn(), 1);
    assert_eq!(replacement.owned_conn(), 1);
    assert_eq!(old.owned_conn(), 0);

    old.dec_conn();
    assert_eq!(replacement.conn(), 1);
    assert_eq!(replacement.owned_conn(), 1);

    replacement.dec_conn();
    assert_eq!(replacement.conn(), 0);
    assert_eq!(replacement.owned_conn(), 0);

    replacement.dec_conn();
    assert_eq!(replacement.conn(), 0);
    assert_eq!(replacement.owned_conn(), 0);
}
