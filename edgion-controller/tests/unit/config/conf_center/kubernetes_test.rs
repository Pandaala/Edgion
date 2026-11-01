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

    let yaml = yaml_serde::to_string(&config).unwrap();
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
    let config: KubernetesConfig = yaml_serde::from_str(yaml).unwrap();
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

    let yaml = yaml_serde::to_string(&config).unwrap();
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
    let filter: MetadataFilterConfig = yaml_serde::from_str(yaml).unwrap();
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
    let config: KubernetesConfig = yaml_serde::from_str(yaml_str).unwrap();
    assert_eq!(config.ha_mode, HaMode::AllServe);

    let yaml_str = r#"
ha_mode: leader-only
"#;
    let config: KubernetesConfig = yaml_serde::from_str(yaml_str).unwrap();
    assert_eq!(config.ha_mode, HaMode::LeaderOnly);

    // Default (no ha_mode specified) should be LeaderOnly
    let yaml_str = "{}";
    let config: KubernetesConfig = yaml_serde::from_str(yaml_str).unwrap();
    assert_eq!(config.ha_mode, HaMode::LeaderOnly);
}

#[test]
fn test_ha_mode_display() {
    assert_eq!(format!("{}", HaMode::LeaderOnly), "leader-only");
    assert_eq!(format!("{}", HaMode::AllServe), "all-serve");
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

    /// Get label selector. Test-only: production reads the field directly.
    #[cfg(test)]
    pub fn label_selector(&self) -> Option<&str> {
        self.label_selector.as_deref()
    }
}
