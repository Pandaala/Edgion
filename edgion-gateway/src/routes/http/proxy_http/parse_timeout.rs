use edgion_resources::resources::edgion_gateway_config::{BackendTimeout, ClientTimeout};
use edgion_resources::EdgionGatewayConfig;
use std::time::Duration;

/// Convert a keepalive duration to Pingora's whole-seconds representation.
///
/// Pingora treats `Some(0)` as an infinite keepalive, so explicit zero must
/// remain zero while every finite fractional duration rounds up. Rounding up
/// also avoids shortening an operator-configured finite bound. Saturation is
/// defensive for the largest representable `Duration` with a fractional part.
fn keepalive_timeout_seconds(duration: Duration) -> u64 {
    if duration.is_zero() {
        return 0;
    }

    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() != 0))
}

/// Pre-parsed timeout configurations for runtime use
#[derive(Debug, Clone, Default)]
pub struct ParsedTimeouts {
    pub client: ParsedClientTimeout,
    pub backend: ParsedBackendTimeout,
}

#[derive(Debug, Clone)]
pub struct ParsedClientTimeout {
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub keepalive_timeout: u64, // keepalive takes seconds as u64
}

#[derive(Debug, Clone)]
pub struct ParsedBackendTimeout {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub idle_timeout: Duration,
}

impl ParsedTimeouts {
    /// Parse timeout configurations from EdgionGatewayConfig
    /// Returns default values if http_timeout is not configured
    pub fn from_config(config: &EdgionGatewayConfig) -> Self {
        match config.spec.http_timeout.as_ref() {
            Some(http_timeout) => Self {
                client: ParsedClientTimeout::from_config(&http_timeout.client),
                backend: ParsedBackendTimeout::from_config(&http_timeout.backend),
            },
            None => Self::default(),
        }
    }
}

impl ParsedClientTimeout {
    fn from_config(config: &ClientTimeout) -> Self {
        use edgion_resources::duration::parse_duration;

        let read_timeout = parse_duration(&config.read_timeout).unwrap_or_else(|e| {
            tracing::warn!(
                "Invalid read_timeout '{}': {}, using default 60s",
                config.read_timeout,
                e
            );
            Duration::from_secs(60)
        });

        let write_timeout = parse_duration(&config.write_timeout).unwrap_or_else(|e| {
            tracing::warn!(
                "Invalid write_timeout '{}': {}, using default 60s",
                config.write_timeout,
                e
            );
            Duration::from_secs(60)
        });

        let keepalive_timeout = parse_duration(&config.keepalive_timeout)
            .map(keepalive_timeout_seconds)
            .unwrap_or_else(|e| {
                tracing::warn!(
                    "Invalid keepalive_timeout '{}': {}, using default 75s",
                    config.keepalive_timeout,
                    e
                );
                75
            });

        Self {
            read_timeout,
            write_timeout,
            keepalive_timeout,
        }
    }
}

impl Default for ParsedClientTimeout {
    fn default() -> Self {
        Self {
            read_timeout: Duration::from_secs(60),
            write_timeout: Duration::from_secs(60),
            keepalive_timeout: 75,
        }
    }
}

impl ParsedBackendTimeout {
    fn from_config(config: &BackendTimeout) -> Self {
        use edgion_resources::duration::parse_duration;

        let connect_timeout = parse_duration(&config.default_connect_timeout).unwrap_or_else(|e| {
            tracing::warn!(
                "Invalid default_connect_timeout '{}': {}, using default 5s",
                config.default_connect_timeout,
                e
            );
            Duration::from_secs(5)
        });

        let request_timeout = parse_duration(&config.default_request_timeout).unwrap_or_else(|e| {
            tracing::warn!(
                "Invalid default_request_timeout '{}': {}, using default 60s",
                config.default_request_timeout,
                e
            );
            Duration::from_secs(60)
        });

        let idle_timeout = parse_duration(&config.default_idle_timeout).unwrap_or_else(|e| {
            tracing::warn!(
                "Invalid default_idle_timeout '{}': {}, using default 300s",
                config.default_idle_timeout,
                e
            );
            Duration::from_secs(300)
        });

        Self {
            connect_timeout,
            request_timeout,
            idle_timeout,
        }
    }
}

impl Default for ParsedBackendTimeout {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(300),
        }
    }
}

#[cfg(test)]
mod tests {
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
                    default_max_retries: None,
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
}
