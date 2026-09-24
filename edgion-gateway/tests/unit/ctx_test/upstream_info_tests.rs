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
}
