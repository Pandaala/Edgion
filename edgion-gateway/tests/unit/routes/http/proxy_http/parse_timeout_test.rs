use super::*;
use edgion_resources::resources::edgion_gateway_config::{EdgionGatewayConfigSpec, HttpTimeout};

#[test]
fn keepalive_seconds_preserve_zero_and_ceil_fractional_values() {
    let cases = [
        (Duration::ZERO, 0),
        (Duration::from_millis(1), 1),
        (Duration::from_millis(500), 1),
        (Duration::from_millis(999), 1),
        (Duration::from_millis(1000), 1),
        (Duration::from_secs(1), 1),
        (Duration::from_millis(1001), 2),
        (Duration::from_secs(1) + Duration::from_millis(1), 2),
        (Duration::new(u64::MAX, 999_999_999), u64::MAX),
    ];

    for (duration, expected) in cases {
        assert_eq!(keepalive_timeout_seconds(duration), expected, "duration={duration:?}");
    }
}

#[test]
fn configured_fractional_keepalive_is_ceiled_without_rounding_other_timeouts() {
    let client = ParsedClientTimeout::from_config(&ClientTimeout {
        read_timeout: "1500ms".to_string(),
        write_timeout: "2500ms".to_string(),
        keepalive_timeout: "500ms".to_string(),
    });

    assert_eq!(client.read_timeout, Duration::from_millis(1500));
    assert_eq!(client.write_timeout, Duration::from_millis(2500));
    assert_eq!(client.keepalive_timeout, 1);
}

#[test]
fn parsed_timeouts_from_config_preserves_defaults() {
    let config = EdgionGatewayConfig::new("test", EdgionGatewayConfigSpec::default());

    let parsed = ParsedTimeouts::from_config(&config);

    assert_eq!(parsed.client.read_timeout, Duration::from_secs(60));
    assert_eq!(parsed.client.write_timeout, Duration::from_secs(60));
    assert_eq!(parsed.client.keepalive_timeout, 75);
    assert_eq!(parsed.backend.connect_timeout, Duration::from_secs(5));
    assert_eq!(parsed.backend.request_timeout, Duration::from_secs(60));
    assert_eq!(parsed.backend.idle_timeout, Duration::from_secs(300));
}

#[test]
fn parsed_timeouts_from_config_keeps_invalid_fallbacks() {
    let spec = EdgionGatewayConfigSpec {
        current_status: None,
        http_timeout: Some(HttpTimeout {
            client: ClientTimeout {
                read_timeout: "invalid".to_string(),
                write_timeout: "invalid".to_string(),
                keepalive_timeout: "invalid".to_string(),
            },
            backend: BackendTimeout {
                default_connect_timeout: "invalid".to_string(),
                default_request_timeout: "invalid".to_string(),
                default_idle_timeout: "invalid".to_string(),
            },
        }),
        ..Default::default()
    };
    let config = EdgionGatewayConfig::new("test", spec);

    let parsed = ParsedTimeouts::from_config(&config);

    assert_eq!(parsed.client.read_timeout, Duration::from_secs(60));
    assert_eq!(parsed.client.write_timeout, Duration::from_secs(60));
    assert_eq!(parsed.client.keepalive_timeout, 75);
    assert_eq!(parsed.backend.connect_timeout, Duration::from_secs(5));
    assert_eq!(parsed.backend.request_timeout, Duration::from_secs(60));
    assert_eq!(parsed.backend.idle_timeout, Duration::from_secs(300));
}
