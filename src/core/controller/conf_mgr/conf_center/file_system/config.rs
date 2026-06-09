//! FileSystem-specific configuration types
//!
//! This module contains configuration types specific to the FileSystem backend:
//! - `FileSystemConfig`: Configuration for file-based configuration center

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// FileSystem configuration center settings
///
/// Used when running in file-based mode where configuration is stored as
/// local YAML files. File watching is always enabled in this mode.
///
/// ## Example (YAML)
///
/// ```yaml
/// type: file_system
/// conf_dir: /etc/edgion/conf
/// poll_interval_ms: 500
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSystemConfig {
    /// Directory containing configuration YAML files
    pub conf_dir: PathBuf,
    /// Polling interval in milliseconds for the file watcher.
    /// When absent the watcher uses its built-in default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_interval_ms: Option<u64>,
}

impl Default for FileSystemConfig {
    fn default() -> Self {
        Self {
            conf_dir: PathBuf::from("conf"),
            poll_interval_ms: None,
        }
    }
}

impl FileSystemConfig {
    /// Create a new FileSystemConfig with the given directory
    pub fn new(conf_dir: PathBuf) -> Self {
        Self {
            conf_dir,
            poll_interval_ms: None,
        }
    }

    /// Get the configuration directory
    pub fn conf_dir(&self) -> &PathBuf {
        &self.conf_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_system_config_default() {
        let config = FileSystemConfig::default();
        assert_eq!(config.conf_dir, PathBuf::from("conf"));
        assert_eq!(config.poll_interval_ms, None);
    }

    #[test]
    fn test_file_system_config_new() {
        let config = FileSystemConfig::new(PathBuf::from("/etc/edgion/conf"));
        assert_eq!(config.conf_dir, PathBuf::from("/etc/edgion/conf"));
    }

    #[test]
    fn test_file_system_config_serialize() {
        let config = FileSystemConfig {
            conf_dir: PathBuf::from("/etc/edgion/conf"),
            poll_interval_ms: None,
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("conf_dir:"));
    }

    #[test]
    fn test_file_system_config_deserialize() {
        let yaml = r#"
conf_dir: /etc/edgion/conf
"#;
        let config: FileSystemConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.conf_dir, PathBuf::from("/etc/edgion/conf"));
    }

    #[test]
    fn test_file_system_config_rejects_unknown_fields() {
        // Stale YAML with endpoint_mode must be rejected
        let yaml = r#"
conf_dir: /etc/edgion/conf
endpoint_mode: endpoint_slice
"#;
        let result: Result<FileSystemConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "endpoint_mode is a removed field and must be rejected");
    }

    #[test]
    fn test_file_system_config_poll_interval_ms_roundtrip() {
        let yaml = "conf_dir: /tmp/conf\npoll_interval_ms: 500\n";
        let config: FileSystemConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.poll_interval_ms, Some(500));

        let serialized = serde_yaml::to_string(&config).unwrap();
        assert!(serialized.contains("poll_interval_ms: 500"));
    }

    #[test]
    fn test_file_system_config_poll_interval_ms_absent() {
        let yaml = "conf_dir: /tmp/conf\n";
        let config: FileSystemConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.poll_interval_ms, None);

        let serialized = serde_yaml::to_string(&config).unwrap();
        assert!(!serialized.contains("poll_interval_ms"));
    }
}
