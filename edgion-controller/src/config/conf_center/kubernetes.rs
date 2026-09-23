//! Kubernetes-specific configuration types
//!
//! This module contains configuration types specific to the Kubernetes backend:
//! - `KubernetesConfig`: Main configuration for Kubernetes backend
//! - `LeaderElectionConfig`: Configuration for leader election in HA deployments
//! - `MetadataFilterConfig`: Configuration for filtering K8s resource metadata

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// HA mode for controller replicas
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HaMode {
    /// Only the leader runs watchers and serves gRPC (default)
    #[default]
    LeaderOnly,
    /// All replicas run watchers and serve gRPC; leader handles status + ACME
    AllServe,
}

impl std::fmt::Display for HaMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HaMode::LeaderOnly => write!(f, "leader-only"),
            HaMode::AllServe => write!(f, "all-serve"),
        }
    }
}

/// Kubernetes configuration center settings
///
/// Used when running in Kubernetes mode where configuration is read from
/// K8s Custom Resources and Services/Endpoints.
///
/// ## Example (YAML)
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
#[derive(Debug, Clone, Serialize)]
pub struct KubernetesConfig {
    /// Namespaces to watch. Empty means all namespaces.
    #[serde(default)]
    pub watch_namespaces: Vec<String>,

    /// Label selector for filtering resources, except GatewayClass reporting.
    #[serde(default)]
    pub label_selector: Option<String>,

    /// Gateway API identity. When omitted, derive it from POD_NAMESPACE and
    /// EDGION_CONTROLLER_DEPLOYMENT_NAME; invalid/missing inputs fail startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_name: Option<String>,

    /// Metadata filter configuration for reducing resource memory usage
    #[serde(default)]
    pub metadata_filter: MetadataFilterConfig,

    /// Leader election configuration (always enabled in K8s mode)
    #[serde(default)]
    pub leader_election: LeaderElectionConfig,

    /// Static address to set on all managed Gateways' status.addresses
    /// when spec.addresses is empty and no matching Service is found.
    /// Useful for conformance testing or single-gateway deployments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_address: Option<String>,

    /// HA mode for multi-replica deployments.
    /// - "leader-only" (default): only the leader runs watchers + gRPC
    /// - "all-serve": all replicas run watchers + gRPC; leader handles status + ACME
    #[serde(default)]
    pub ha_mode: HaMode,

    /// Resource kinds to skip during CRD preflight check and not watch.
    /// Listed kinds will not be probed, not spawned as watchers, and excluded
    /// from processor readiness gating.
    ///
    /// Values are PascalCase kind names matching `ResourceKind::as_str()`.
    /// Example: `["TCPRoute", "UDPRoute", "BackendTLSPolicy"]`
    #[serde(default)]
    pub no_watch_kinds: Vec<String>,

    /// Deadline (seconds) for a started watcher set to reach CachesReady.
    /// Covers the readiness poll (internally budgeted at 30 s) plus the
    /// post-ready tail (WatchObj registration, cross-namespace revalidations,
    /// leader-job start). Restart-bound: a reload re-arms the deadline but
    /// always with the boot-time value. Raise this when a single resource
    /// kind's count approaches the per-kind workqueue capacity (5000) —
    /// a too-small value is a permanent restart loop, which is why values
    /// at or below the 30 s poll budget are rejected at startup.
    #[serde(default = "default_cache_ready_deadline_secs")]
    pub cache_ready_deadline_secs: u64,
}

impl<'de> Deserialize<'de> for KubernetesConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Keep startup's existing unknown/duplicate-field behavior while refusing
        // the removed selector even when its explicitly supplied value is null.
        #[derive(Deserialize)]
        struct Input {
            #[serde(default)]
            watch_namespaces: Vec<String>,

            #[serde(default)]
            label_selector: Option<String>,

            #[serde(default)]
            controller_name: Option<String>,

            #[serde(default)]
            metadata_filter: MetadataFilterConfig,

            #[serde(default)]
            leader_election: LeaderElectionConfig,

            #[serde(default)]
            gateway_address: Option<String>,

            #[serde(default)]
            ha_mode: HaMode,

            #[serde(default)]
            no_watch_kinds: Vec<String>,

            #[serde(default = "default_cache_ready_deadline_secs")]
            cache_ready_deadline_secs: u64,
            #[serde(default, rename = "gateway_class", deserialize_with = "reject_gateway_class")]
            _removed_gateway_class: (),
        }
        fn reject_gateway_class<'de, D>(_: D) -> std::result::Result<(), D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            Err(serde::de::Error::custom(
                "gateway_class has been removed; GatewayClasses are selected by controller_name",
            ))
        }
        let input = Input::deserialize(deserializer)?;
        Ok(Self {
            watch_namespaces: input.watch_namespaces,
            label_selector: input.label_selector,
            controller_name: input.controller_name,
            metadata_filter: input.metadata_filter,
            leader_election: input.leader_election,
            gateway_address: input.gateway_address,
            ha_mode: input.ha_mode,
            no_watch_kinds: input.no_watch_kinds,
            cache_ready_deadline_secs: input.cache_ready_deadline_secs,
        })
    }
}

impl Default for KubernetesConfig {
    fn default() -> Self {
        Self {
            watch_namespaces: Vec::new(),
            label_selector: None,
            controller_name: None,
            metadata_filter: MetadataFilterConfig::default(),
            leader_election: LeaderElectionConfig::default(),
            gateway_address: None,
            ha_mode: HaMode::default(),
            no_watch_kinds: Vec::new(),
            cache_ready_deadline_secs: default_cache_ready_deadline_secs(),
        }
    }
}

impl KubernetesConfig {
    /// Set watch namespaces. Test-only builder seam.
    #[cfg(test)]
    pub fn with_watch_namespaces(mut self, namespaces: Vec<String>) -> Self {
        self.watch_namespaces = namespaces;
        self
    }

    /// Set label selector. Test-only builder seam.
    #[cfg(test)]
    pub fn with_label_selector(mut self, selector: impl Into<String>) -> Self {
        self.label_selector = Some(selector.into());
        self
    }

    /// Set controller name used in Gateway API status reporting. Test-only builder seam.
    #[cfg(test)]
    pub fn with_controller_name(mut self, controller_name: impl Into<String>) -> Self {
        self.controller_name = Some(controller_name.into());
        self
    }

    /// Set metadata filter. Test-only builder seam.
    #[cfg(test)]
    pub fn with_metadata_filter(mut self, filter: MetadataFilterConfig) -> Self {
        self.metadata_filter = filter;
        self
    }

    /// Get watch namespaces
    pub fn watch_namespaces(&self) -> &[String] {
        &self.watch_namespaces
    }

    /// Get label selector. Test-only: production reads the field directly.
    #[cfg(test)]
    pub fn label_selector(&self) -> Option<&str> {
        self.label_selector.as_deref()
    }

    /// Get controller name
    pub fn resolve_controller_name(&self) -> Result<String> {
        super::controller_identity::resolve_kubernetes_controller_name(
            self.controller_name.as_deref(),
            std::env::var("POD_NAMESPACE").ok().as_deref(),
            std::env::var("EDGION_CONTROLLER_DEPLOYMENT_NAME").ok().as_deref(),
        )
    }

    /// Get metadata filter
    pub fn metadata_filter(&self) -> &MetadataFilterConfig {
        &self.metadata_filter
    }

    /// Get leader election config
    pub fn leader_election(&self) -> &LeaderElectionConfig {
        &self.leader_election
    }

    /// Get HA mode
    pub fn ha_mode(&self) -> HaMode {
        self.ha_mode
    }

    /// Fail-fast validation of K8s-flow timing knobs (same fail-fast policy
    /// as the leader-election timing validation in leader_election.rs: a bad
    /// value here risks a permanent restart loop at runtime, which is worse
    /// than a startup error).
    pub fn validate(&self) -> Result<()> {
        const READINESS_POLL_BUDGET_SECS: u64 = 30;
        if self.cache_ready_deadline_secs <= READINESS_POLL_BUDGET_SECS {
            anyhow::bail!(
                "kubernetes conf_center misconfig: cache_ready_deadline_secs ({}) must be > {} \
                 (the internal cache-readiness poll budget); a deadline that cannot even cover \
                 the poll is a designed-in restart loop",
                self.cache_ready_deadline_secs,
                READINESS_POLL_BUDGET_SECS
            );
        }
        Ok(())
    }
}

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

    /// Renew deadline in seconds. Mirrors client-go's `RenewDeadline`: the
    /// leader keeps leadership across transient apiserver errors (5xx,
    /// network) as long as the previous successful renew was within this
    /// window. Must satisfy
    /// `retry_period_secs < renew_deadline_secs < lease_duration_secs`.
    #[serde(default = "default_renew_deadline_secs")]
    pub renew_deadline_secs: u64,

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
            renew_deadline_secs: default_renew_deadline_secs(),
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

fn default_renew_deadline_secs() -> u64 {
    10
}

fn default_retry_period_secs() -> u64 {
    2
}

fn default_cache_ready_deadline_secs() -> u64 {
    90
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
    fn test_kubernetes_config_new() {
        let config = KubernetesConfig::default();
        assert_eq!(config.controller_name, None);
        assert!(config.watch_namespaces.is_empty());
        assert!(config.label_selector.is_none());
    }

    #[test]
    fn test_kubernetes_config_builder() {
        let config = KubernetesConfig::default()
            .with_watch_namespaces(vec!["default".to_string(), "prod".to_string()])
            .with_label_selector("app=edgion")
            .with_controller_name("example.com/controller");

        assert_eq!(config.resolve_controller_name().unwrap(), "example.com/controller");
        assert_eq!(config.watch_namespaces(), &["default", "prod"]);
        assert_eq!(config.label_selector(), Some("app=edgion"));
    }

    #[test]
    fn test_kubernetes_config_serialize() {
        let config = KubernetesConfig::default().with_watch_namespaces(vec!["default".to_string()]);

        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(!yaml.contains("gateway_class"));
        assert!(!yaml.contains("controller_name:"));
        assert!(yaml.contains("watch_namespaces:"));
    }

    #[test]
    fn test_kubernetes_config_deserialize() {
        let yaml = r#"
watch_namespaces:
  - default
  - prod
label_selector: app=edgion
"#;
        let config: KubernetesConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.controller_name, None);
        assert_eq!(config.watch_namespaces, vec!["default", "prod"]);
        assert_eq!(config.label_selector, Some("app=edgion".to_string()));
    }

    #[test]
    fn test_leader_election_config_default() {
        let config = LeaderElectionConfig::default();
        assert_eq!(config.lease_name, "edgion-controller-leader");
        assert_eq!(config.lease_duration_secs, 15);
        assert_eq!(config.renew_deadline_secs, 10);
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
            renew_deadline_secs: 20,
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

    #[test]
    fn test_ha_mode_default() {
        assert_eq!(HaMode::default(), HaMode::LeaderOnly);
    }

    #[test]
    fn test_ha_mode_serde() {
        let yaml_str = r#"
ha_mode: all-serve
"#;
        let config: KubernetesConfig = serde_yaml::from_str(yaml_str).unwrap();
        assert_eq!(config.ha_mode, HaMode::AllServe);

        let yaml_str = r#"
ha_mode: leader-only
"#;
        let config: KubernetesConfig = serde_yaml::from_str(yaml_str).unwrap();
        assert_eq!(config.ha_mode, HaMode::LeaderOnly);

        // Default (no ha_mode specified) should be LeaderOnly
        let yaml_str = "{}";
        let config: KubernetesConfig = serde_yaml::from_str(yaml_str).unwrap();
        assert_eq!(config.ha_mode, HaMode::LeaderOnly);
    }

    #[test]
    fn test_ha_mode_display() {
        assert_eq!(format!("{}", HaMode::LeaderOnly), "leader-only");
        assert_eq!(format!("{}", HaMode::AllServe), "all-serve");
    }
}

#[cfg(test)]
mod cache_ready_deadline_tests {
    use super::*;

    #[test]
    fn default_deadline_is_90_and_valid() {
        let cfg = KubernetesConfig::default();
        assert_eq!(cfg.cache_ready_deadline_secs, 90);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn deadline_at_or_below_poll_budget_rejected() {
        let mut cfg = KubernetesConfig {
            cache_ready_deadline_secs: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
        cfg.cache_ready_deadline_secs = 30;
        assert!(cfg.validate().is_err());
        cfg.cache_ready_deadline_secs = 31;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn deadline_deserializes_with_default() {
        let yaml = "{}";
        let cfg: KubernetesConfig = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(cfg.cache_ready_deadline_secs, 90);
    }
}
