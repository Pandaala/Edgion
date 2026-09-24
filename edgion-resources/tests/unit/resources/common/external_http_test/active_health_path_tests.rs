use super::*;

fn connection_with_path(path: &str) -> ExternalHttpConnection {
    ExternalHttpConnection {
        target: WebhookTarget {
            url: Some("https://api.example.com".to_string()),
            ..Default::default()
        },
        health_check: Some(WebhookHealthCheck {
            active: Some(ActiveHealthCheck {
                path: Some(path.to_string()),
                interval_sec: 10,
                timeout_ms: 2_000,
                healthy_threshold: 1,
                unhealthy_threshold: 3,
            }),
            passive: None,
        }),
        ..Default::default()
    }
}

#[test]
fn active_health_path_accepts_root_relative_path_and_query() {
    for path in ["/healthz", "/healthz?ready=1", "/%2F%5C%23?next=%2F"] {
        assert!(
            validate_active_health_path(path).is_ok(),
            "path {path:?} must be accepted"
        );
        assert!(
            validate_connection_target(&connection_with_path(path)).is_none(),
            "path {path:?} must pass connection validation"
        );
    }
}

#[test]
fn active_health_path_rejects_authority_and_fragment_forms() {
    for path in [
        "healthz",
        "?ready=1",
        "https://evil.example/healthz",
        "//evil.example/healthz",
        "///evil.example/healthz",
        r"/\evil.example/healthz",
        r"/healthz\tail",
        "/healthz#details",
    ] {
        let err = validate_connection_target(&connection_with_path(path))
            .unwrap_or_else(|| panic!("path {path:?} must be rejected"));
        assert!(err.contains("healthCheck.active.path"), "got: {err}");
    }
}
