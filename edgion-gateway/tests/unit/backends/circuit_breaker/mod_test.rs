use super::*;
use pingora_load_balancing::Backend;

#[test]
fn permit_releases_on_drop() {
    let b = Arc::new(LbBackend::new(Backend::new("127.0.0.1:1").unwrap()));
    {
        let _p = CbPermit::try_acquire(b.clone(), 1).unwrap();
        assert_eq!(b.cb_inflight(), 1);
        assert!(CbPermit::try_acquire(b.clone(), 1).is_none()); // at cap
    }
    assert_eq!(b.cb_inflight(), 0); // dropped -> released
}
