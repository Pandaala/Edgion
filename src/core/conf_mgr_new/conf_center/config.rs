//! Configuration Center configuration
//!
//! This module defines the top-level configuration enum for the configuration center,
//! which determines whether to use FileSystem or Kubernetes backend.

use super::common::EndpointMode;
use super::kubernetes::config::{LeaderElectionConfig, MetadataFilterConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the Configuration Center
///
/// Supports two backends:
/// - FileSystem: Local YAML files with file watching (always enabled)
/// - Kubernetes: K8s API with resource watching via Controller
///
/// ## Example (FileSystem mode)
///
/// ```yaml
/// type: file_system
/// conf_dir: /etc/edgion/conf
/// endpoint_mode: endpoint_slice
/// ```
///
/// ## Example (Kubernetes mode)
///
/// ```yaml
/// type: kubernetes
/// gateway_class: edgion
/// watch_namespaces:
///   - default
///   - prod
/// label_selector: app=edgion
/// endpoint_mode: auto
/// leader_election:
///   lease_name: edgion-controller-leader
///   lease_namespace: edgion-system
/// metadata_filter:
///   remove_managed_fields: true
///   blocked_annotations:
///     - kubectl.kubernetes.io/last-applied-configuration
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfCenterConfig {
    /// File system based configuration center
    /// File watching is always enabled in this mode
    FileSystem {
        /// Directory containing configuration YAML files
        conf_dir: PathBuf,

        /// Endpoint discovery mode
        /// In file system mode, this determines which endpoint resource type to use:
        /// - Endpoint: Use Endpoints resources
        /// - EndpointSlice: Use EndpointSlice resources (recommended)
        /// - Auto: Same as EndpointSlice in file system mode
        #[serde(default)]
        endpoint_mode: EndpointMode,
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

        /// Leader election configuration (always enabled in K8s mode)
        #[serde(default)]
        leader_election: LeaderElectionConfig,

        /// Endpoint discovery mode for Kubernetes
        #[serde(default)]
        endpoint_mode: EndpointMode,
    },
}

impl Default for ConfCenterConfig {
    fn default() -> Self {
        Self::FileSystem {
            conf_dir: PathBuf::from("conf"),
            endpoint_mode: EndpointMode::default(),
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

    /// Get the endpoint mode
    pub fn endpoint_mode(&self) -> EndpointMode {
        match self {
            Self::FileSystem { endpoint_mode, .. } => *endpoint_mode,
            Self::Kubernetes { endpoint_mode, .. } => *endpoint_mode,
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

    /// Get metadata filter configuration (Kubernetes mode only)
    pub fn metadata_filter(&self) -> Option<&MetadataFilterConfig> {
        match self {
            Self::FileSystem { .. } => None,
            Self::Kubernetes { metadata_filter, .. } => Some(metadata_filter),
        }
    }

    /// Get leader election configuration (Kubernetes mode only)
    pub fn leader_election(&self) -> Option<&LeaderElectionConfig> {
        match self {
            Self::FileSystem { .. } => None,
            Self::Kubernetes { leader_election, .. } => Some(leader_election),
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
            endpoint_mode: EndpointMode::EndpointSlice,
        };

        assert!(!config.is_k8s_mode());
        assert_eq!(config.conf_dir(), Some(&PathBuf::from("/etc/edgion/conf")));
        assert!(config.metadata_filter().is_none());
        assert_eq!(config.endpoint_mode(), EndpointMode::EndpointSlice);
    }

    #[test]
    fn test_kubernetes_config() {
        let config = ConfCenterConfig::Kubernetes {
            watch_namespaces: vec!["default".to_string(), "prod".to_string()],
            label_selector: Some("app=edgion".to_string()),
            gateway_class: "edgion".to_string(),
            metadata_filter: MetadataFilterConfig::default(),
            leader_election: LeaderElectionConfig::default(),
            endpoint_mode: EndpointMode::default(),
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

        // Test leader election config
        let le = config.leader_election().unwrap();
        assert_eq!(le.lease_name, "edgion-controller-leader");
        assert_eq!(le.lease_duration_secs, 15);
    }

    #[test]
    fn test_deserialize_file_system() {
        let yaml = r#"
type: file_system
conf_dir: /etc/edgion/conf
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
}
