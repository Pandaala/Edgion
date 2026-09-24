use edgion_resources::resources::common::parse_byte_size;
use edgion_resources::resources::http_route::MirrorTuning;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

/// Startup-only limits shared by every mirror plugin generation in one process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RequestMirrorGlobalConfig {
    /// Maximum simultaneously active mirror tasks across all plugin instances.
    pub max_concurrent: usize,
    /// Retained mirror request-body budget, not a limit on total process RSS.
    pub max_buffered_bytes: String,
}

impl Default for RequestMirrorGlobalConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 1024,
            max_buffered_bytes: "64MiB".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRequestMirrorGlobalConfig {
    pub max_concurrent: usize,
    pub max_buffered_bytes: usize,
}

impl RequestMirrorGlobalConfig {
    /// Resolve and validate before installing the process budget at startup.
    pub fn resolve(&self) -> Result<ResolvedRequestMirrorGlobalConfig, String> {
        if !(1..=MirrorTuning::MAX_CONCURRENT).contains(&self.max_concurrent) {
            return Err(format!(
                "request_mirror.max_concurrent must be between 1 and {}",
                MirrorTuning::MAX_CONCURRENT
            ));
        }
        let bytes = parse_byte_size(&self.max_buffered_bytes)
            .filter(|bytes| (1..=MirrorTuning::MAX_BUFFERED_BYTES as u64).contains(bytes))
            .ok_or_else(|| {
                "request_mirror.max_buffered_bytes must be a byte size between 1 byte and 1GiB".to_string()
            })?;
        let max_buffered_bytes = usize::try_from(bytes)
            .map_err(|_| "request_mirror.max_buffered_bytes exceeds this platform's capacity".to_string())?;
        if max_buffered_bytes > Semaphore::MAX_PERMITS {
            return Err("request_mirror.max_buffered_bytes exceeds this platform's semaphore capacity".to_string());
        }
        Ok(ResolvedRequestMirrorGlobalConfig {
            max_concurrent: self.max_concurrent,
            max_buffered_bytes,
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/request_mirror/config_test.rs"]
mod tests;
