use super::{apply_upstream_authority, finalize_forwarded_headers};
use crate::ctx::EdgionHttpContext;

/// The filter writes the upstream Host from the ctx when an authority was
/// composed, and leaves the request untouched when it was not.
#[test]
fn upstream_host_is_written_from_ctx() {
    let mut req = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
    req.insert_header("Host", "gw.example.com").unwrap();

    let mut ctx = EdgionHttpContext::default();
    ctx.upstream_authority = Some("tenant-a.sandbox.internal".to_string());
    apply_upstream_authority(&mut req, &ctx);
    assert_eq!(req.headers.get("host").unwrap(), "tenant-a.sandbox.internal");

    // No authority => Host untouched.
    let mut req2 = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
    req2.insert_header("Host", "gw.example.com").unwrap();
    let ctx2 = EdgionHttpContext::default();
    apply_upstream_authority(&mut req2, &ctx2);
    assert_eq!(req2.headers.get("host").unwrap(), "gw.example.com");
}

#[test]
fn canonical_forwarded_headers_overwrite_plugin_values() {
    let mut req = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
    req.insert_header("X-Forwarded-For", "spoofed").unwrap();
    req.insert_header("X-Edgion-Remote-IP", "spoofed").unwrap();

    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.x_forwarded_for = Some("198.51.100.8".to_string());
    ctx.request_info.downstream_peer_ip = "10.0.0.5".to_string();
    ctx.request_info.remote_ip = "203.0.113.9".to_string();
    let name = http::HeaderName::from_static("x-edgion-remote-ip");

    finalize_forwarded_headers(&name, &mut req, &ctx);

    assert_eq!(req.headers.get("x-forwarded-for").unwrap(), "198.51.100.8, 10.0.0.5");
    assert_eq!(req.headers.get(&name).unwrap(), "203.0.113.9");
}
