//! FileSystem-specific configuration types
//!
//! This module contains configuration types specific to the FileSystem backend:
//! - `FileSystemConfig`: Configuration for file-based configuration center

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::core::controller::conf_mgr::conf_center::common::EndpointMode;

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
/// endpoint_mode: endpoint_slice
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSystemConfig {
    /// Directory containing configuration YAML files
    pub conf_dir: PathBuf,

    /// Endpoint discovery mode
    ///
    /// In file system mode, this determines which endpoint resource type to use:
    /// - Endpoint: Use Endpoints resources
    /// - EndpointSlice: Use EndpointSlice resources (recommended)
    /// - Auto: Same as EndpointSlice in file system mode
    #[serde(default)]
    pub endpoint_mode: EndpointMode,
}

impl Default for FileSystemConfig {
    fn default() -> Self {
        Self {
            conf_dir: PathBuf::from("conf"),
            endpoint_mode: EndpointMode::default(),
        }
    }
}

impl FileSystemConfig {
    /// Create a new FileSystemConfig with the given directory
    pub fn new(conf_dir: PathBuf) -> Self {
        Self {
            conf_dir,
            endpoint_mode: EndpointMode::default(),
        }
    }

    /// Create with custom endpoint mode
    pub fn with_endpoint_mode(mut self, mode: EndpointMode) -> Self {
        self.endpoint_mode = mode;
        self
    }

    /// Get the configuration directory
    pub fn conf_dir(&self) -> &PathBuf {
        &self.conf_dir
    }

    /// Get the endpoint mode
    pub fn endpoint_mode(&self) -> EndpointMode {
        self.endpoint_mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_system_config_default() {
        let config = FileSystemConfig::default();
        assert_eq!(config.conf_dir, PathBuf::from("conf"));
        assert!(config.endpoint_mode.is_auto());
    }

    #[test]
    fn test_file_system_config_new() {
        let config = FileSystemConfig::new(PathBuf::from("/etc/edgion/conf"));
        assert_eq!(config.conf_dir, PathBuf::from("/etc/edgion/conf"));
    }

    #[test]
    fn test_file_system_config_builder() {
        let config =
            FileSystemConfig::new(PathBuf::from("/etc/edgion/conf")).with_endpoint_mode(EndpointMode::EndpointSlice);
        assert_eq!(config.endpoint_mode, EndpointMode::EndpointSlice);
    }

    #[test]
    fn test_file_system_config_serialize() {
        let config = FileSystemConfig {
            conf_dir: PathBuf::from("/etc/edgion/conf"),
            endpoint_mode: EndpointMode::EndpointSlice,
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("conf_dir:"));
        assert!(yaml.contains("endpoint_mode: endpoint_slice"));
    }

    #[test]
    fn test_file_system_config_deserialize() {
        let yaml = r#"
conf_dir: /etc/edgion/conf
endpoint_mode: endpoint
"#;
        let config: FileSystemConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.conf_dir, PathBuf::from("/etc/edgion/conf"));
        assert!(config.endpoint_mode.is_endpoint());
    }
}
