use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;

fn create_test_plugin(rate: &str) -> Box<dyn UpstreamResponseBodyFilter> {
    let config = BandwidthLimitConfig {
        rate: rate.to_string(),
        rate_bytes_per_second: None,
    };
    BandwidthLimit::create(&config)
}

#[test]
fn test_valid_rates_share_static_name_without_rate_text() {
    let megabyte = create_test_plugin("1mb");
    let gigabyte = create_test_plugin("1gb");

    assert_eq!(megabyte.name(), "BandwidthLimit");
    assert_eq!(gigabyte.name(), "BandwidthLimit");
    assert!(!megabyte.name().contains("1mb"));
    assert!(!gigabyte.name().contains("1gb"));
}

#[test]
fn test_bandwidth_limit_none_body() {
    let plugin = create_test_plugin("1mb");
    let body: Option<bytes::Bytes> = None;

    let mut session = MockPluginSession::new();
    let delay = plugin.run_upstream_response_body_filter(&body, false, &mut session);
    assert!(delay.is_none());
}

#[test]
fn test_bandwidth_limit_invalid_rate() {
    let plugin = create_test_plugin("invalid");
    let body = Some(bytes::Bytes::from(vec![0u8; 1024]));

    let mut session = MockPluginSession::new();
    let delay = plugin.run_upstream_response_body_filter(&body, false, &mut session);
    // Invalid rate -> fail-open, no throttling
    assert!(delay.is_none());
}
