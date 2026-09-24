use super::safe_default_downstream_response_trailers_representable;

#[test]
fn response_trailer_transport_capability_defaults_fail_closed() {
    assert!(!safe_default_downstream_response_trailers_representable());
}
