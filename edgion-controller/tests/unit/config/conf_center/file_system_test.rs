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
        controller_name: None,
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
