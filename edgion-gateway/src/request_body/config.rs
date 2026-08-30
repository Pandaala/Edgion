use edgion_resources::resources::common::parse_byte_size;
use edgion_resources::resources::edgion_gateway_config::RequestBodyConfig;
use std::time::Duration;

pub const DEFAULT_MEMORY_BUFFER_BYTES: usize = 128 * 1024;
pub const DEFAULT_MAX_BODY_SIZE_BYTES: u64 = 32 * 1024 * 1024;
pub const DEFAULT_STORAGE_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Request-body policy resolved when an EdgionGatewayConfig snapshot is built.
///
/// All human-readable sizes are parsed on the configuration update path. Each
/// request copies this compact value from the immutable EGC snapshot into its
/// context and performs only direct field reads afterward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRequestBodyConfig {
    pub enabled: bool,
    pub memory_buffer_bytes: usize,
    pub max_body_size_bytes: u64,
    pub storage_operation_timeout: Duration,
}

impl ResolvedRequestBodyConfig {
    pub fn resolve(config: &RequestBodyConfig) -> Self {
        let default_memory = parse_positive(config.default_memory_buffer_size.as_str())
            .and_then(|bytes| usize::try_from(bytes).ok())
            .unwrap_or(DEFAULT_MEMORY_BUFFER_BYTES);
        let memory_buffer_bytes = config
            .max_memory_buffer_size
            .as_deref()
            .and_then(parse_positive)
            .and_then(|bytes| usize::try_from(bytes).ok())
            .unwrap_or(default_memory);

        let default_body = parse_positive(config.default_max_body_size.as_str()).unwrap_or(DEFAULT_MAX_BODY_SIZE_BYTES);
        let max_body_size_bytes = config
            .max_body_size
            .as_deref()
            .and_then(parse_positive)
            .unwrap_or(default_body);
        let storage_operation_timeout = edgion_resources::duration::parse_duration(&config.storage_operation_timeout)
            .ok()
            .filter(|timeout| !timeout.is_zero())
            .unwrap_or(DEFAULT_STORAGE_OPERATION_TIMEOUT);

        Self {
            enabled: config.enabled,
            memory_buffer_bytes: memory_buffer_bytes.min(usize::try_from(max_body_size_bytes).unwrap_or(usize::MAX)),
            max_body_size_bytes,
            storage_operation_timeout,
        }
    }
}

impl Default for ResolvedRequestBodyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_buffer_bytes: DEFAULT_MEMORY_BUFFER_BYTES,
            max_body_size_bytes: DEFAULT_MAX_BODY_SIZE_BYTES,
            storage_operation_timeout: DEFAULT_STORAGE_OPERATION_TIMEOUT,
        }
    }
}

fn parse_positive(value: &str) -> Option<u64> {
    parse_byte_size(value).filter(|bytes| *bytes > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_preserve_existing_limits() {
        let resolved = ResolvedRequestBodyConfig::resolve(&RequestBodyConfig::default());
        assert_eq!(resolved.memory_buffer_bytes, 128 * 1024);
        assert_eq!(resolved.max_body_size_bytes, 32 * 1024 * 1024);
        assert_eq!(resolved.storage_operation_timeout, Duration::from_secs(5));
    }

    #[test]
    fn business_maxima_override_defaults() {
        let config = RequestBodyConfig {
            max_memory_buffer_size: Some("1MiB".to_string()),
            max_body_size: Some("64MiB".to_string()),
            ..Default::default()
        };
        let resolved = ResolvedRequestBodyConfig::resolve(&config);
        assert_eq!(resolved.memory_buffer_bytes, 1024 * 1024);
        assert_eq!(resolved.max_body_size_bytes, 64 * 1024 * 1024);
    }

    #[test]
    fn disabled_policy_disables_capture() {
        let config = RequestBodyConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!ResolvedRequestBodyConfig::resolve(&config).enabled);
    }

    #[test]
    fn memory_never_exceeds_body_cap() {
        let config = RequestBodyConfig {
            max_memory_buffer_size: Some("2MiB".to_string()),
            max_body_size: Some("1MiB".to_string()),
            ..Default::default()
        };
        let resolved = ResolvedRequestBodyConfig::resolve(&config);
        assert_eq!(resolved.memory_buffer_bytes, 1024 * 1024);
    }

    #[test]
    fn storage_operation_timeout_resolves_and_invalid_values_fail_safe() {
        let configured = RequestBodyConfig {
            storage_operation_timeout: "750ms".to_string(),
            ..Default::default()
        };
        assert_eq!(
            ResolvedRequestBodyConfig::resolve(&configured).storage_operation_timeout,
            Duration::from_millis(750)
        );

        for value in ["0s", "invalid"] {
            let invalid = RequestBodyConfig {
                storage_operation_timeout: value.to_string(),
                ..Default::default()
            };
            assert_eq!(
                ResolvedRequestBodyConfig::resolve(&invalid).storage_operation_timeout,
                DEFAULT_STORAGE_OPERATION_TIMEOUT
            );
        }
    }
}
