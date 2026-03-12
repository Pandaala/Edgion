//! Configuration Center configuration
//!
//! This module defines the top-level configuration enum for the configuration center,
//! which determines whether to use FileSystem or Kubernetes backend.
//!
//! The actual configuration fields are defined in their respective modules:
//! - `file_system::FileSystemConfig`: FileSystem-specific configuration
//! - `kubernetes::KubernetesConfig`: Kubernetes-specific configuration

use super::common::EndpointMode;
use super::file_system::FileSystemConfig;
use super::kubernetes::KubernetesConfig;
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
    FileSystem(FileSystemConfig),

    /// Kubernetes based configuration center
    Kubernetes(KubernetesConfig),
}

impl Default for ConfCenterConfig {
    fn default() -> Self {
        Self::FileSystem(FileSystemConfig::default())
    }
}

impl ConfCenterConfig {
    /// Check if running in Kubernetes mode
    pub fn is_k8s_mode(&self) -> bool {
        matches!(self, Self::Kubernetes(_))
    }

    /// Get the endpoint mode
    pub fn endpoint_mode(&self) -> EndpointMode {
        match self {
            Self::FileSystem(config) => config.endpoint_mode(),
            Self::Kubernetes(config) => config.endpoint_mode(),
        }
    }

    /// Get FileSystem config (if in FileSystem mode)
    pub fn as_file_system(&self) -> Option<&FileSystemConfig> {
        match self {
            Self::FileSystem(config) => Some(config),
            Self::Kubernetes(_) => None,
        }
    }

    /// Get Kubernetes config (if in Kubernetes mode)
    pub fn as_kubernetes(&self) -> Option<&KubernetesConfig> {
        match self {
            Self::FileSystem(_) => None,
            Self::Kubernetes(config) => Some(config),
        }
    }

    /// Get the configuration directory (FileSystem mode only)
    pub fn conf_dir(&self) -> Option<&PathBuf> {
        self.as_file_system().map(|c| c.conf_dir())
    }
}

/// Create a FileSystem config from path
impl From<PathBuf> for ConfCenterConfig {
    fn from(conf_dir: PathBuf) -> Self {
        Self::FileSystem(FileSystemConfig::new(conf_dir))
    }
}

/// Create a Kubernetes config from gateway class
impl From<&str> for ConfCenterConfig {
    fn from(gateway_class: &str) -> Self {
        Self::Kubernetes(KubernetesConfig::new(gateway_class))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_system_config() {
        let config = ConfCenterConfig::FileSystem(FileSystemConfig {
            conf_dir: PathBuf::from("/etc/edgion/conf"),
            endpoint_mode: EndpointMode::EndpointSlice,
        });

        assert!(!config.is_k8s_mode());
        assert_eq!(config.conf_dir(), Some(&PathBuf::from("/etc/edgion/conf")));
        assert_eq!(config.endpoint_mode(), EndpointMode::EndpointSlice);
        assert!(config.as_file_system().is_some());
        assert!(config.as_kubernetes().is_none());
    }

    #[test]
    fn test_kubernetes_config() {
        let config = ConfCenterConfig::Kubernetes(
            KubernetesConfig::new("edgion")
                .with_watch_namespaces(vec!["default".to_string(), "prod".to_string()])
                .with_label_selector("app=edgion"),
        );

        assert!(config.is_k8s_mode());
        let k8s_config = config.as_kubernetes().unwrap();
        assert_eq!(k8s_config.watch_namespaces(), &["default", "prod"]);
        assert_eq!(k8s_config.label_selector(), Some("app=edgion"));
        assert_eq!(k8s_config.gateway_class(), "edgion");

        // Test metadata filter
        let filter = k8s_config.metadata_filter();
        assert!(filter.remove_managed_fields);
        assert!(filter
            .blocked_annotations
            .contains(&"kubectl.kubernetes.io/last-applied-configuration".to_string()));

        // Test leader election config
        let le = k8s_config.leader_election();
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
        assert_eq!(config.conf_dir(), Some(&PathBuf::from("/etc/edgion/conf")));
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
        let filter = config.as_kubernetes().unwrap().metadata_filter();
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
        let filter = config.as_kubernetes().unwrap().metadata_filter();
        assert!(!filter.remove_managed_fields);
        assert_eq!(filter.blocked_annotations.len(), 1);
        assert_eq!(filter.blocked_annotations[0], "custom.annotation/to-remove");
    }

    #[test]
    fn test_from_path() {
        let config: ConfCenterConfig = PathBuf::from("/etc/edgion/conf").into();
        assert!(!config.is_k8s_mode());
        assert_eq!(config.conf_dir(), Some(&PathBuf::from("/etc/edgion/conf")));
    }

    #[test]
    fn test_from_gateway_class() {
        let config: ConfCenterConfig = "edgion".into();
        assert!(config.is_k8s_mode());
        assert_eq!(config.as_kubernetes().unwrap().gateway_class(), "edgion");
    }
}
