use super::*;

#[test]
fn failure_entry_preserves_wire_fields_and_omits_upstream() {
    let entry = UdpLogEntry::failure(9000, "127.0.0.1".to_owned(), 50000, "NoRouteMatched");
    let value = serde_json::to_value(&entry).unwrap();

    assert_eq!(value["listener_port"], 9000);
    assert!(value["direct_peer_ip"].is_string());
    assert_eq!(value["direct_peer_port"], 50000);
    assert_eq!(value["session_duration_ms"], 0);
    assert_eq!(value["status"], "NoRouteMatched");
    assert!(value.get("upstream_addr").is_none());
}

#[test]
fn udp_status_as_str_matches_log_field() {
    use crate::routes::udp::edgion_udp::UdpStatus;
    let cases: &[(UdpStatus, &str)] = &[
        (UdpStatus::NoRouteMatched, "NoRouteMatched"),
        (UdpStatus::NoRuleAvailable, "NoRuleAvailable"),
        (UdpStatus::NoBackendSelected, "NoBackendSelected"),
        (UdpStatus::RefDenied, "RefDenied"),
        (UdpStatus::UnsupportedBackend, "UnsupportedBackend"),
        (UdpStatus::NoBackendResolved, "NoBackendResolved"),
        (UdpStatus::UnsafeUpstreamAddress, "unsafe upstream address"),
        (UdpStatus::SessionLimitReached, "SessionLimitReached"),
        (UdpStatus::UpstreamSocketError, "UpstreamSocketError"),
        (UdpStatus::UpstreamSendError, "UpstreamSendError"),
        (UdpStatus::DownstreamSendError, "DownstreamSendError"),
    ];
    for (variant, expected) in cases {
        assert_eq!(variant.as_str(), *expected);
    }
}

#[test]
fn session_creation_failures_serialize_distinct_fixed_statuses() {
    use crate::routes::udp::edgion_udp::UdpStatus;
    for (status, expected) in [
        (UdpStatus::SessionLimitReached, "SessionLimitReached"),
        (UdpStatus::UpstreamSocketError, "UpstreamSocketError"),
    ] {
        let entry = UdpLogEntry::failure(9000, "192.0.2.1".to_owned(), 50000, status.as_str());
        let value: serde_json::Value = serde_json::from_str(&entry.to_json()).unwrap();
        assert_eq!(value["listener_port"], 9000);
        assert!(value["direct_peer_ip"].is_string());
        assert_eq!(value["direct_peer_port"], 50000);
        assert_eq!(value["status"], expected);
        assert!(value.get("upstream_addr").is_none());
        for field in [
            "session_duration_ms",
            "packets_sent",
            "packets_received",
            "bytes_sent",
            "bytes_received",
        ] {
            assert_eq!(value[field], 0);
        }
    }
}

#[test]
fn session_entry_carries_all_counters_and_omits_status() {
    let entry = UdpLogEntry::new(
        9000,
        "127.0.0.1".to_owned(),
        50000,
        Some("127.0.0.2:8080".to_owned()),
        Instant::now(),
    )
    .with_stats(1, 2, 3, 4);
    let value = serde_json::to_value(&entry).unwrap();

    assert_eq!(value["upstream_addr"], "127.0.0.2:8080");
    assert_eq!(value["packets_sent"], 1);
    assert_eq!(value["packets_received"], 2);
    assert_eq!(value["bytes_sent"], 3);
    assert_eq!(value["bytes_received"], 4);
    assert!(value.get("status").is_none());
}
