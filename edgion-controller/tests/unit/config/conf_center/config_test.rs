use super::*;

#[test]
fn test_file_system_config() {
    let config = ConfCenterConfig::FileSystem(FileSystemConfig {
        controller_name: None,
        conf_dir: PathBuf::from("/etc/edgion/conf"),
        poll_interval_ms: None,
    });

    assert!(!config.is_k8s_mode());
    assert_eq!(config.conf_dir(), Some(&PathBuf::from("/etc/edgion/conf")));
    assert!(config.as_file_system().is_some());
    assert!(config.as_kubernetes().is_none());
}

#[test]
fn test_kubernetes_config() {
    let config = ConfCenterConfig::Kubernetes(
        KubernetesConfig::default()
            .with_watch_namespaces(vec!["default".to_string(), "prod".to_string()])
            .with_label_selector("app=edgion"),
    );

    assert!(config.is_k8s_mode());
    let k8s_config = config.as_kubernetes().unwrap();
    assert_eq!(k8s_config.watch_namespaces(), &["default", "prod"]);
    assert_eq!(k8s_config.label_selector(), Some("app=edgion"));

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
    let config: ConfCenterConfig = yaml_serde::from_str(yaml).unwrap();
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
"#;
    let config: ConfCenterConfig = yaml_serde::from_str(yaml).unwrap();
    assert!(config.is_k8s_mode());
    // Default metadata filter should be applied
    let filter = config.as_kubernetes().unwrap().metadata_filter();
    assert!(filter.remove_managed_fields);
}

#[test]
fn test_deserialize_kubernetes_with_metadata_filter() {
    let yaml = r#"
type: kubernetes
metadata_filter:
  blocked_annotations:
    - "custom.annotation/to-remove"
  remove_managed_fields: false
"#;
    let config: ConfCenterConfig = yaml_serde::from_str(yaml).unwrap();
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
fn obsolete_gateway_class_is_rejected_through_tagged_configuration() {
    for value in ["edgion", "''", "null", "12"] {
        let error = yaml_serde::from_str::<ConfCenterConfig>(&format!("type: kubernetes\ngateway_class: {value}\n"))
            .unwrap_err();
        assert!(error.to_string().contains("gateway_class has been removed"), "{error}");
    }
    let config: ConfCenterConfig =
        yaml_serde::from_str("type: kubernetes\ncontroller_name: example.com/controller\n").unwrap();
    assert_eq!(
        config.as_kubernetes().unwrap().resolve_controller_name().unwrap(),
        "example.com/controller"
    );
    assert!(!yaml_serde::to_string(&config).unwrap().contains("gateway_class"));
    assert!(
        yaml_serde::from_str::<ConfCenterConfig>("type: kubernetes\nha_mode: all-serve\nha_mode: leader-only\n")
            .is_err()
    );
    assert!(yaml_serde::from_str::<ConfCenterConfig>("type: kubernetes\nunrelated: ignored\n").is_ok());
}

impl ConfCenterConfig {
    /// Get FileSystem config (if in FileSystem mode). Test-only: production
    /// matches the enum directly.
    #[cfg(test)]
    pub fn as_file_system(&self) -> Option<&FileSystemConfig> {
        match self {
            Self::FileSystem(config) => Some(config),
            _ => None,
        }
    }

    /// Get Kubernetes config (if in Kubernetes mode). Test-only: production
    /// matches the enum directly.
    #[cfg(test)]
    pub fn as_kubernetes(&self) -> Option<&KubernetesConfig> {
        match self {
            Self::Kubernetes(config) => Some(config),
            _ => None,
        }
    }

    /// Get the configuration directory (FileSystem mode only). Test-only.
    #[cfg(test)]
    pub fn conf_dir(&self) -> Option<&PathBuf> {
        self.as_file_system().map(|c| c.conf_dir())
    }
}
