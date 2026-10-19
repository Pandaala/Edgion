use super::*;

fn upstream_info() -> UpstreamInfo {
    UpstreamInfo {
        ip: None,
        port: None,
        status: None,
        ct: None,
        ht: None,
        bt: None,
        et: None,
        wpt: None,
        response_body_size: None,
        start_time: Instant::now(),
        err: Vec::new(),
        lb_backend_addr: None,
        lb_backend: None,
        lb_outlier_completion: None,
        service_key: None,
        lb_policy: None,
        tls: None,
        lb_committed: None,
    }
}

#[test]
fn repeated_response_body_size_is_idempotent_when_unchanged() {
    let mut upstream = upstream_info();

    upstream.set_response_body_size(17);
    upstream.set_response_body_size(17);

    assert_eq!(upstream.response_body_size, Some(17));
    assert!(upstream.err.is_empty());
}

#[test]
fn changed_response_body_size_keeps_double_set_diagnostic() {
    let mut upstream = upstream_info();

    upstream.set_response_body_size(17);
    upstream.set_response_body_size(23);

    assert_eq!(upstream.response_body_size, Some(23));
    assert_eq!(upstream.err, ["response_body_size already set"]);
    assert!(matches!(upstream.err[0], std::borrow::Cow::Borrowed(_)));
}

#[test]
fn attempt_error_storage_preserves_serialization_and_owned_lifetime() {
    use std::borrow::Cow;
    let mut upstream = upstream_info();
    assert!(serde_json::to_value(&upstream).unwrap().get("err").is_none());
    upstream.err.push("ConnectError".into());
    let dynamic = String::from("dns resolution failed: test failure");
    let original_ptr = dynamic.as_ptr();
    upstream.err.push(dynamic.into());
    assert!(matches!(upstream.err[0], Cow::Borrowed("ConnectError")));
    assert!(matches!(&upstream.err[1], Cow::Owned(value) if value.as_ptr() == original_ptr));
    assert_eq!(
        serde_json::to_value(&upstream).unwrap()["err"],
        serde_json::json!(["ConnectError", "dns resolution failed: test failure"])
    );
}
