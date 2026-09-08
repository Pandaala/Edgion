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
mod tests {
    use super::*;

    #[test]
    fn mirror_global_defaults_and_partial_override() {
        let default: RequestMirrorGlobalConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(
            default.resolve().unwrap(),
            ResolvedRequestMirrorGlobalConfig {
                max_concurrent: 1024,
                max_buffered_bytes: 64 * 1024 * 1024,
            }
        );
        let partial: RequestMirrorGlobalConfig = serde_yaml::from_str("max_buffered_bytes: 2MiB").unwrap();
        assert_eq!(partial.resolve().unwrap().max_concurrent, 1024);
        assert_eq!(partial.resolve().unwrap().max_buffered_bytes, 2 * 1024 * 1024);
    }

    #[test]
    fn mirror_global_limits_accept_boundaries_and_reject_unbounded_values() {
        let largest_buffer = MirrorTuning::MAX_BUFFERED_BYTES.min(Semaphore::MAX_PERMITS).to_string();
        for (concurrent, bytes) in [(1, "1"), (MirrorTuning::MAX_CONCURRENT, largest_buffer.as_str())] {
            let config = RequestMirrorGlobalConfig {
                max_concurrent: concurrent,
                max_buffered_bytes: bytes.to_string(),
            };
            let resolved = config.resolve().unwrap();
            assert!(resolved.max_concurrent <= Semaphore::MAX_PERMITS);
            assert!(resolved.max_buffered_bytes <= Semaphore::MAX_PERMITS);
        }
        for concurrent in [0, MirrorTuning::MAX_CONCURRENT + 1, usize::MAX] {
            let config = RequestMirrorGlobalConfig {
                max_concurrent: concurrent,
                ..Default::default()
            };
            assert!(config.resolve().unwrap_err().contains("max_concurrent"));
        }
        for bytes in [
            "0",
            "-1",
            "1GiB1",
            "1073741825",
            "2GiB",
            "NaN",
            "",
            "999999999999999999999GiB",
        ] {
            let config = RequestMirrorGlobalConfig {
                max_buffered_bytes: bytes.to_string(),
                ..Default::default()
            };
            assert!(config.resolve().unwrap_err().contains("max_buffered_bytes"), "{bytes}");
        }
    }

    #[test]
    fn mirror_global_unknown_knob_is_rejected() {
        assert!(serde_yaml::from_str::<RequestMirrorGlobalConfig>("max_concurent: 12").is_err());
    }
}
