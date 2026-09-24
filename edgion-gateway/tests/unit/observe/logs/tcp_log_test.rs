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
    let entry = TcpLogEntry::from_context(&denied_ctx(Some("stream plugin resource not found")));
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
    let entry = TcpLogEntry::from_context(&denied_ctx(None));
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
