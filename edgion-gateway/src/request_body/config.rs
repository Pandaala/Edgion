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
    pub max_buffered_size_bytes: u64,
    pub spill_to_disk: bool,
    pub storage_operation_timeout: Duration,
}

impl ResolvedRequestBodyConfig {
    /// Capacity of a new capture owner. Existing owners retain their own bound.
    pub fn capture_limit(self) -> u64 {
        let cap = self.max_buffered_size_bytes.min(self.max_body_size_bytes);
        if self.spill_to_disk {
            cap
        } else {
            cap.min(self.memory_buffer_bytes as u64)
        }
    }

    pub fn resolve(config: &RequestBodyConfig) -> Self {
        let max_body_size_bytes = parse_positive(&config.max_body_size).unwrap_or(DEFAULT_MAX_BODY_SIZE_BYTES);
        let max_buffered_size_bytes = config
            .storage
            .max_buffered_size
            .as_deref()
            .and_then(parse_positive)
            .unwrap_or(max_body_size_bytes)
            .min(max_body_size_bytes);
        let memory_buffer_bytes = parse_positive(&config.storage.memory_limit)
            .and_then(|bytes| usize::try_from(bytes).ok())
            .unwrap_or(DEFAULT_MEMORY_BUFFER_BYTES)
            .min(usize::try_from(max_buffered_size_bytes).unwrap_or(usize::MAX));
        let storage_operation_timeout = edgion_resources::duration::parse_duration(&config.storage.operation_timeout)
            .ok()
            .filter(|timeout| !timeout.is_zero())
            .unwrap_or(DEFAULT_STORAGE_OPERATION_TIMEOUT);

        Self {
            enabled: config.enabled,
            memory_buffer_bytes: memory_buffer_bytes.min(usize::try_from(max_body_size_bytes).unwrap_or(usize::MAX)),
            max_body_size_bytes,
            max_buffered_size_bytes,
            spill_to_disk: config.storage.spill_to_disk,
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
            max_buffered_size_bytes: DEFAULT_MAX_BODY_SIZE_BYTES,
            spill_to_disk: true,
            storage_operation_timeout: DEFAULT_STORAGE_OPERATION_TIMEOUT,
        }
    }
}

fn parse_positive(value: &str) -> Option<u64> {
    parse_byte_size(value).filter(|bytes| *bytes > 0)
}

#[cfg(test)]
#[path = "../../tests/unit/request_body/config_test.rs"]
mod tests;
