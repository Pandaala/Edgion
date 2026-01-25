//! Kubernetes-specific configuration types
//!
//! This module contains configuration types specific to the Kubernetes backend:
//! - `LeaderElectionConfig`: Configuration for leader election in HA deployments
//! - `MetadataFilterConfig`: Configuration for filtering K8s resource metadata

use serde::{Deserialize, Serialize};

/// Leader election configuration for HA deployments
///
/// In K8s mode, leader election is always enabled to ensure only one
/// controller instance is active at a time when running multiple replicas.
///
/// Uses Kubernetes Lease objects for leader election, similar to
/// controller-runtime's leader election implementation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaderElectionConfig {
    /// Lease resource name for leader election
    #[serde(default = "default_lease_name")]
    pub lease_name: String,

    /// Namespace where the Lease resource will be created
    /// Defaults to the namespace from POD_NAMESPACE env var or "default"
    #[serde(default = "default_lease_namespace")]
    pub lease_namespace: String,

    /// Lease duration in seconds (how long the lease is valid)
    /// The leader must renew before this duration expires
    #[serde(default = "default_lease_duration_secs")]
    pub lease_duration_secs: i32,

    /// Renew period in seconds (how often the leader renews the lease)
    /// Should be less than lease_duration_secs
    #[serde(default = "default_renew_period_secs")]
    pub renew_period_secs: u64,

    /// Retry period in seconds (how often non-leaders try to acquire)
    #[serde(default = "default_retry_period_secs")]
    pub retry_period_secs: u64,
}

impl Default for LeaderElectionConfig {
    fn default() -> Self {
        Self {
            lease_name: default_lease_name(),
            lease_namespace: default_lease_namespace(),
            lease_duration_secs: default_lease_duration_secs(),
            renew_period_secs: default_renew_period_secs(),
            retry_period_secs: default_retry_period_secs(),
        }
    }
}

fn default_lease_name() -> String {
    "edgion-controller-leader".to_string()
}

fn default_lease_namespace() -> String {
    // Try to get namespace from environment (set by K8s Downward API)
    std::env::var("POD_NAMESPACE").unwrap_or_else(|_| "default".to_string())
}

fn default_lease_duration_secs() -> i32 {
    15
}

fn default_renew_period_secs() -> u64 {
    10
}

fn default_retry_period_secs() -> u64 {
    2
}

/// Metadata filter configuration for reducing K8s resource size in memory
///
/// When loading resources from Kubernetes, certain metadata fields can be
/// removed to reduce memory usage. These fields are typically not needed
/// for the controller's operation.
///
/// ## Default Blocked Annotations
///
/// - `kubectl.kubernetes.io/last-applied-configuration`: Large, stores full resource
/// - `meta.helm.sh/release-name`: Helm metadata
/// - `meta.helm.sh/release-namespace`: Helm metadata
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_leader_election_config_default() {
        let config = LeaderElectionConfig::default();
        assert_eq!(config.lease_name, "edgion-controller-leader");
        assert_eq!(config.lease_duration_secs, 15);
        assert_eq!(config.renew_period_secs, 10);
        assert_eq!(config.retry_period_secs, 2);
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

    #[test]
    fn test_leader_election_config_serialize() {
        let config = LeaderElectionConfig {
            lease_name: "my-lease".to_string(),
            lease_namespace: "my-namespace".to_string(),
            lease_duration_secs: 30,
            renew_period_secs: 20,
            retry_period_secs: 5,
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("lease_name: my-lease"));
        assert!(yaml.contains("lease_namespace: my-namespace"));
    }

    #[test]
    fn test_metadata_filter_config_deserialize() {
        let yaml = r#"
blocked_annotations:
  - "custom.annotation/to-remove"
remove_managed_fields: false
"#;
        let filter: MetadataFilterConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!filter.remove_managed_fields);
        assert_eq!(filter.blocked_annotations.len(), 1);
        assert_eq!(filter.blocked_annotations[0], "custom.annotation/to-remove");
    }
}
