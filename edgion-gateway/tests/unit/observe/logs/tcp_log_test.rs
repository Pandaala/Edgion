use super::*;
use crate::routes::tcp::edgion_tcp::TcpStatus;
use std::time::Instant;

fn denied_ctx(reason: Option<&'static str>) -> TcpContext {
    TcpContext {
        listener_port: 9000,
        direct_peer_ip: "127.0.0.1".to_string(),
        direct_peer_port: 50000,
        remote_ip: "127.0.0.1".to_string(),
        remote_port: 50000,
        upstream_addr: None,
        start_time: Instant::now(),
        bytes_sent: 0,
        bytes_received: 0,
        status: TcpStatus::PluginDenied,
        connection_established: false,
        deny_reason: reason,
    }
}

#[test]
fn from_context_propagates_deny_reason_into_json() {
    // A plugin-denied connection (connection_established = false) is still
    // logged, and its bounded deny_reason must reach the JSON entry.
    let ctx = denied_ctx(Some("stream plugin resource not found"));
    let entry = TcpLogEntry::from_context(&ctx);
    assert_eq!(entry.deny_reason, Some("stream plugin resource not found"));
    assert!(
        entry
            .to_json()
            .contains(r#""deny_reason":"stream plugin resource not found""#),
        "deny_reason must be serialized into the TCP log JSON"
    );
}

#[test]
fn from_context_omits_deny_reason_when_none() {
    // Non-denied connections leave deny_reason None; serde must skip it.
    let ctx = denied_ctx(None);
    let entry = TcpLogEntry::from_context(&ctx);
    assert_eq!(entry.deny_reason, None);
    assert!(
        !entry.to_json().contains("deny_reason"),
        "deny_reason must be omitted from JSON when None"
    );
}

#[test]
fn tcp_status_as_str_matches_log_field() {
    // Locks the TCP access log `status` field labels. If a variant is
    // renamed for code clarity, this test forces an explicit update so
    // operators' log pipelines do not break silently.
    let cases: &[(TcpStatus, &str)] = &[
        (TcpStatus::Success, "Success"),
        (TcpStatus::NoRouteMatched, "NoRouteMatched"),
        (TcpStatus::NoRuleAvailable, "NoRuleAvailable"),
        (TcpStatus::PluginDenied, "PluginDenied"),
        (TcpStatus::NoBackendSelected, "NoBackendSelected"),
        (TcpStatus::RefDenied, "RefDenied"),
        (TcpStatus::UnsupportedBackend, "UnsupportedBackend"),
        (TcpStatus::NoBackendResolved, "NoBackendResolved"),
        (TcpStatus::UnsafeUpstreamAddress, "UnsafeUpstreamAddress"),
        (TcpStatus::UpstreamConnectionFailed, "UpstreamConnectionFailed"),
        (TcpStatus::UpstreamReadError, "UpstreamReadError"),
        (TcpStatus::UpstreamWriteError, "UpstreamWriteError"),
        (TcpStatus::DownstreamReadError, "DownstreamReadError"),
        (TcpStatus::DownstreamWriteError, "DownstreamWriteError"),
        (TcpStatus::IdleTimeout, "IdleTimeout"),
    ];
    for (variant, expected) in cases {
        assert_eq!(variant.as_str(), *expected);
    }
}

#[test]
fn direct_peer_is_carried_separately_from_remote_ip() {
    // On a proxy-protocol listener RemoteIp is the logical client while
    // DirectPeerIp stays the physical load balancer.
    // A wiring mistake reading peer_addr() twice would make them equal and
    // no other test would notice.
    let mut ctx = denied_ctx(None);
    ctx.remote_ip = "203.0.113.9".to_string();
    ctx.direct_peer_ip = "10.0.0.7".to_string();

    let entry = TcpLogEntry::from_context(&ctx);
    assert_eq!(entry.remote_ip, "203.0.113.9");
    assert_eq!(entry.direct_peer_ip, "10.0.0.7");

    let json = serde_json::to_string(&entry).unwrap();
    assert!(json.contains("\"direct_peer_ip\":\"10.0.0.7\""), "{json}");
    assert!(json.contains("\"remote_ip\":\"203.0.113.9\""), "{json}");
}

#[test]
fn context_projection_borrows_strings_without_allocating() {
    for upstream in [None, Some("[2001:db8::1]:8443")] {
        let mut ctx = denied_ctx(Some("stream plugin policy denied"));
        ctx.direct_peer_ip = "2001:db8::2".into();
        ctx.remote_ip = "203.0.113.9".into();
        ctx.upstream_addr = upstream.map(str::to_owned);
        ctx.bytes_sent = 123;
        ctx.bytes_received = 456;
        let (entry, allocations) =
            crate::test_allocation_counter::count_allocations(|| TcpLogEntry::from_context(std::hint::black_box(&ctx)));
        let mut expected = serde_json::json!({
            "ts": entry.ts,
            "listener_port": ctx.listener_port,
            "direct_peer_ip": edgion_resources::sensitive::apply_sensitive(&ctx.direct_peer_ip, str::to_owned),
            "direct_peer_port": ctx.direct_peer_port,
            "remote_ip": edgion_resources::sensitive::apply_sensitive(&ctx.remote_ip, str::to_owned),
            "remote_port": ctx.remote_port,
            "duration_ms": entry.duration_ms,
            "bytes_sent": ctx.bytes_sent,
            "bytes_received": ctx.bytes_received,
            "status": ctx.status.as_str(),
            "connection_established": ctx.connection_established,
            "deny_reason": ctx.deny_reason,
        });
        if let Some(upstream) = &ctx.upstream_addr {
            expected["upstream_addr"] = upstream.clone().into();
        }
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&entry.to_json()).unwrap(),
            expected
        );
        assert_eq!(allocations, 0, "context projection must not copy strings");
        assert_eq!(entry.direct_peer_ip.as_ptr(), ctx.direct_peer_ip.as_ptr());
        assert_eq!(entry.remote_ip.as_ptr(), ctx.remote_ip.as_ptr());
        assert_eq!(entry.status.as_ptr(), ctx.status.as_str().as_ptr());
        assert_eq!(
            entry.upstream_addr.as_ref().map(|s| s.as_ptr()),
            ctx.upstream_addr.as_ref().map(|s| s.as_ptr())
        );
    }
}
