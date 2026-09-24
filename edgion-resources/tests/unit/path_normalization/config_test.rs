use super::*;

#[test]
fn default_values() {
    let cfg = PathNormalizationConfig::default();
    assert!(!cfg.reject_escaped_slashes);
    assert!(!cfg.reject_invalid_percent_encoding);
    assert!(!cfg.preserve_path_parameters);
    assert!(cfg.forward_normalized_path);
}

#[test]
fn deserialize_from_yaml() {
    let yaml_str = r#"
            reject_escaped_slashes: true
            reject_invalid_percent_encoding: true
        "#;
    let cfg: PathNormalizationConfig = serde_yaml::from_str(yaml_str).unwrap();
    assert!(cfg.reject_escaped_slashes);
    assert!(cfg.reject_invalid_percent_encoding);
}

#[test]
fn deserialize_partial() {
    // Only one field set, the other defaults
    let yaml_str = r#"reject_escaped_slashes: true"#;
    let cfg: PathNormalizationConfig = serde_yaml::from_str(yaml_str).unwrap();
    assert!(cfg.reject_escaped_slashes);
    assert!(!cfg.reject_invalid_percent_encoding);
}

#[test]
fn preserve_path_parameters_default_off() {
    let cfg = PathNormalizationConfig::default();
    assert!(!cfg.preserve_path_parameters);
}

#[test]
fn deserialize_preserve_path_parameters() {
    let yaml_str = r#"preserve_path_parameters: true"#;
    let cfg: PathNormalizationConfig = serde_yaml::from_str(yaml_str).unwrap();
    assert!(cfg.preserve_path_parameters);
}

#[test]
fn forward_normalized_path_default_true() {
    let cfg = PathNormalizationConfig::default();
    assert!(cfg.forward_normalized_path);
}

#[test]
fn deserialize_forward_normalized_path_false() {
    let yaml_str = r#"forward_normalized_path: false"#;
    let cfg: PathNormalizationConfig = serde_yaml::from_str(yaml_str).unwrap();
    assert!(!cfg.forward_normalized_path);
}
