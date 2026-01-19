//! ConfCenter configuration
//!
//! Defines the configuration enum for different configuration center backends.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the Configuration Center
///
/// Supports two backends:
/// - FileSystem: Local YAML files with optional file watching
/// - Kubernetes: K8s API with resource watching via Controller
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfCenterConfig {
    /// File system based configuration center
    FileSystem {
        /// Directory containing configuration YAML files
        conf_dir: PathBuf,
        /// Enable file watching for hot-reload (default: false)
        #[serde(default)]
        watch_enabled: bool,
    },
    /// Kubernetes based configuration center
    Kubernetes {
        /// Namespaces to watch. Empty means all namespaces.
        #[serde(default)]
        watch_namespaces: Vec<String>,
        /// Label selector for filtering resources
        #[serde(default)]
        label_selector: Option<String>,
        /// Gateway class name this controller manages
        gateway_class: String,
        /// Metadata filter configuration for reducing resource memory usage
        #[serde(default)]
        metadata_filter: MetadataFilterConfig,
    },
}

/// Metadata filter configuration for reducing K8s resource size in memory
///
/// When loading resources from Kubernetes, certain metadata fields can be
/// removed to reduce memory usage. These fields are typically not needed
/// for the controller's operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataFilterConfig {
    /// Annotations to remove from resources (blacklist)
    /// Default includes kubectl last-applied-configuration and helm metadata
    #[serde(default = "default_blocked_annotations")]
    pub blocked_annotations: Vec<String>,
    /// Whether to remove managedFields from resources
    /// managedFields can be large and is not needed for most operations
    #[serde(default = "default_remove_managed_fields")]
    pub remove_managed_fields: bool,
}

impl Default for MetadataFilterConfig {
    fn default() -> Self {
        Self {
            blocked_annotations: default_blocked_annotations(),
            remove_managed_fields: default_remove_managed_fields(),
        }
    }
}

fn default_blocked_annotations() -> Vec<String> {
    vec![
        "kubectl.kubernetes.io/last-applied-configuration".to_string(),
        "meta.helm.sh/release-name".to_string(),
        "meta.helm.sh/release-namespace".to_string(),
    ]
}

fn default_remove_managed_fields() -> bool {
    true
}

impl Default for ConfCenterConfig {
    fn default() -> Self {
        Self::FileSystem {
            conf_dir: PathBuf::from("conf"),
            watch_enabled: false,
        }
    }
}

impl ConfCenterConfig {
    /// Check if running in Kubernetes mode
    pub fn is_k8s_mode(&self) -> bool {
        matches!(self, Self::Kubernetes { .. })
    }

    /// Get the configuration directory (FileSystem mode only)
    pub fn conf_dir(&self) -> Option<&PathBuf> {
        match self {
            Self::FileSystem { conf_dir, .. } => Some(conf_dir),
            Self::Kubernetes { .. } => None,
        }
    }

    /// Get watch namespaces (Kubernetes mode only)
    pub fn watch_namespaces(&self) -> &[String] {
        match self {
            Self::FileSystem { .. } => &[],
            Self::Kubernetes { watch_namespaces, .. } => watch_namespaces,
        }
    }

    /// Get label selector (Kubernetes mode only)
    pub fn label_selector(&self) -> Option<&str> {
        match self {
            Self::FileSystem { .. } => None,
            Self::Kubernetes { label_selector, .. } => label_selector.as_deref(),
        }
    }

    /// Get gateway class name (Kubernetes mode only)
    pub fn gateway_class(&self) -> Option<&str> {
        match self {
            Self::FileSystem { .. } => None,
            Self::Kubernetes { gateway_class, .. } => Some(gateway_class),
        }
    }

    /// Check if file watching is enabled (FileSystem mode only)
    pub fn watch_enabled(&self) -> bool {
        match self {
            Self::FileSystem { watch_enabled, .. } => *watch_enabled,
            Self::Kubernetes { .. } => false,
        }
    }

    /// Get metadata filter configuration (Kubernetes mode only)
    pub fn metadata_filter(&self) -> Option<&MetadataFilterConfig> {
        match self {
            Self::FileSystem { .. } => None,
            Self::Kubernetes { metadata_filter, .. } => Some(metadata_filter),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_system_config() {
        let config = ConfCenterConfig::FileSystem {
            conf_dir: PathBuf::from("/etc/edgion/conf"),
            watch_enabled: true,
        };

        assert!(!config.is_k8s_mode());
        assert_eq!(config.conf_dir(), Some(&PathBuf::from("/etc/edgion/conf")));
        assert!(config.watch_enabled());
        assert!(config.metadata_filter().is_none());
    }

    #[test]
    fn test_kubernetes_config() {
        let config = ConfCenterConfig::Kubernetes {
            watch_namespaces: vec!["default".to_string(), "prod".to_string()],
            label_selector: Some("app=edgion".to_string()),
            gateway_class: "edgion".to_string(),
            metadata_filter: MetadataFilterConfig::default(),
        };

        assert!(config.is_k8s_mode());
        assert_eq!(config.watch_namespaces(), &["default", "prod"]);
        assert_eq!(config.label_selector(), Some("app=edgion"));
        assert_eq!(config.gateway_class(), Some("edgion"));

        // Test metadata filter
        let filter = config.metadata_filter().unwrap();
        assert!(filter.remove_managed_fields);
        assert!(filter
            .blocked_annotations
            .contains(&"kubectl.kubernetes.io/last-applied-configuration".to_string()));
    }

    #[test]
    fn test_deserialize_file_system() {
        let yaml = r#"
type: file_system
conf_dir: /etc/edgion/conf
watch_enabled: true
"#;
        let config: ConfCenterConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.is_k8s_mode());
    }

    #[test]
    fn test_deserialize_kubernetes() {
        let yaml = r#"
type: kubernetes
watch_namespaces:
  - default
  - prod
label_selector: app=edgion
gateway_class: edgion
"#;
        let config: ConfCenterConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.is_k8s_mode());
        // Default metadata filter should be applied
        let filter = config.metadata_filter().unwrap();
        assert!(filter.remove_managed_fields);
    }

    #[test]
    fn test_deserialize_kubernetes_with_metadata_filter() {
        let yaml = r#"
type: kubernetes
gateway_class: edgion
metadata_filter:
  blocked_annotations:
    - "custom.annotation/to-remove"
  remove_managed_fields: false
"#;
        let config: ConfCenterConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.is_k8s_mode());
        let filter = config.metadata_filter().unwrap();
        assert!(!filter.remove_managed_fields);
        assert_eq!(filter.blocked_annotations.len(), 1);
        assert_eq!(filter.blocked_annotations[0], "custom.annotation/to-remove");
    }

    #[test]
    fn test_metadata_filter_config_default() {
        let filter = MetadataFilterConfig::default();
        assert!(filter.remove_managed_fields);
        assert_eq!(filter.blocked_annotations.len(), 3);
        assert!(filter
            .blocked_annotations
            .contains(&"kubectl.kubernetes.io/last-applied-configuration".to_string()));
        assert!(filter
            .blocked_annotations
            .contains(&"meta.helm.sh/release-name".to_string()));
        assert!(filter
            .blocked_annotations
            .contains(&"meta.helm.sh/release-namespace".to_string()));
    }
}
