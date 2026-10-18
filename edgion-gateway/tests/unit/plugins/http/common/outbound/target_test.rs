use super::*;

#[test]
fn static_target_borrows_stored_url_without_allocating() {
    for url in ["", "https://upstream.example:8443/base", "http://[2001:db8::1]:8080"] {
        let target = ResolvedTarget::Static(url.to_owned());
        let ResolvedTarget::Static(stored) = &target else {
            unreachable!();
        };
        let (picked, stats) = crate::test_allocation_counter::count_allocation_stats(|| target.pick_url());
        let picked = picked.unwrap();
        assert!(matches!(picked, Cow::Borrowed(_)));
        assert_eq!(picked, url);
        assert_eq!(picked.as_ptr(), stored.as_ptr());
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
}

#[test]
fn rr_pick_rotates_canonical_service_endpoints_and_handles_empty() {
    let cursor = AtomicUsize::new(0);
    let eps = vec![
        "10.0.0.1:8443".parse::<std::net::SocketAddr>().unwrap(),
        "[2001:db8::1]:9443".parse::<std::net::SocketAddr>().unwrap(),
    ];

    let a = rr_pick(&eps, &cursor, "http");
    let b = rr_pick(&eps, &cursor, "https");
    let c = rr_pick(&eps, &cursor, "http");
    assert_eq!(a.as_deref(), Some("http://10.0.0.1:8443"));
    assert_eq!(b.as_deref(), Some("https://[2001:db8::1]:9443"));
    assert_eq!(c.as_deref(), Some("http://10.0.0.1:8443"));

    let empty: Vec<std::net::SocketAddr> = vec![];
    assert_eq!(rr_pick(&empty, &cursor, "http"), None);
    assert_eq!(cursor.load(std::sync::atomic::Ordering::Relaxed), 3);
}
