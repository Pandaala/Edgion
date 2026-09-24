use super::*;

#[test]
fn rr_pick_rotates_canonical_service_endpoints_and_handles_empty() {
    let cursor = AtomicUsize::new(0);
    let eps = vec!["10.0.0.1:8443".to_string(), "[2001:db8::1]:9443".to_string()];

    let a = rr_pick(&eps, &cursor, "http");
    let b = rr_pick(&eps, &cursor, "https");
    let c = rr_pick(&eps, &cursor, "http");
    assert_eq!(a.as_deref(), Some("http://10.0.0.1:8443"));
    assert_eq!(b.as_deref(), Some("https://[2001:db8::1]:9443"));
    assert_eq!(c.as_deref(), Some("http://10.0.0.1:8443"));

    let empty: Vec<String> = vec![];
    assert_eq!(rr_pick(&empty, &cursor, "http"), None);
}
