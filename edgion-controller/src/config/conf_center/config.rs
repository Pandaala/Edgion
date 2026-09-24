//! Configuration Center configuration
//!
//! This module defines the top-level configuration enum for the configuration center,
//! which determines whether to use FileSystem or Kubernetes backend.
//!
//! The actual configuration fields are defined in their respective modules:
//! - `file_system::FileSystemConfig`: FileSystem-specific configuration
//! - `kubernetes::KubernetesConfig`: Kubernetes-specific configuration

use super::etcd::EtcdConfig;
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
/// controller_name: edgion.io/gateway-controller
/// conf_dir: /etc/edgion/conf
/// ```
///
/// ## Example (Kubernetes mode)
///
/// ```yaml
/// type: kubernetes
/// controller_name: edgion.io/gateway-controller
/// watch_namespaces:
///   - default
///   - prod
/// label_selector: app=edgion
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

    /// Etcd based configuration center
    Etcd(EtcdConfig),
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
}

/// Create a FileSystem config from path
impl From<PathBuf> for ConfCenterConfig {
    fn from(conf_dir: PathBuf) -> Self {
        Self::FileSystem(FileSystemConfig::new(conf_dir))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/config/conf_center/config_test.rs"]
mod tests;
