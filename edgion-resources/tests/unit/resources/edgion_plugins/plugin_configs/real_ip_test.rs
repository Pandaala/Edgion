use super::*;

fn group(name: &str, cidrs: &[&str]) -> IpGroup {
    IpGroup {
        name: name.to_string(),
        description: None,
        cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn test_default_config() {
    let config = RealIpConfig::default();
    assert_eq!(config.trusted_ips.len(), 0);
    assert_eq!(config.real_ip_header, "X-Forwarded-For");
    assert!(config.recursive);
}

#[test]
fn test_empty_trusted_ips() {
    let mut config = RealIpConfig::default();
    let result = config.validate_and_init();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must not be empty"));
}

#[test]
fn test_empty_real_ip_header() {
    let mut config = RealIpConfig {
        trusted_ips: vec![group("default", &["10.0.0.0/8"])],
        real_ip_header: "".to_string(),
        ..Default::default()
    };
    let result = config.validate_and_init();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("real_ip_header"));
}

#[test]
fn test_invalid_ip() {
    let mut config = RealIpConfig {
        trusted_ips: vec![group("default", &["192.168.1.256"])],
        ..Default::default()
    };
    let result = config.validate_and_init();
    assert!(result.is_err());
    // New error path: validation surfaces the parse error via validate_groups_structure.
    let msg = result.unwrap_err();
    assert!(msg.contains("Invalid IP"), "unexpected message: {msg}");
}

#[test]
fn test_invalid_cidr_prefix() {
    let mut config = RealIpConfig {
        trusted_ips: vec![group("default", &["192.168.1.0/33"])],
        ..Default::default()
    };
    let result = config.validate_and_init();
    assert!(result.is_err());
}

#[test]
fn test_custom_header() {
    let mut config = RealIpConfig {
        trusted_ips: vec![group("default", &["10.0.0.0/8"])],
        real_ip_header: "CF-Connecting-IP".to_string(),
        recursive: false,
        ..Default::default()
    };
    assert!(config.validate_and_init().is_ok());
    assert_eq!(config.real_ip_header, "CF-Connecting-IP");
    assert!(!config.recursive);
}

#[test]
fn test_serialization() {
    let config = RealIpConfig {
        trusted_ips: vec![group("default", &["10.0.0.0/8", "192.168.1.1"])],
        real_ip_header: "X-Real-IP".to_string(),
        recursive: false,
        ..Default::default()
    };

    let json = serde_json::to_value(&config).unwrap();
    let deserialized: RealIpConfig = serde_json::from_value(json).unwrap();

    assert_eq!(deserialized.trusted_ips.len(), 1);
    assert_eq!(deserialized.trusted_ips[0].name, "default");
    assert_eq!(
        deserialized.trusted_ips[0].cidrs,
        vec!["10.0.0.0/8".to_string(), "192.168.1.1".to_string()]
    );
    assert_eq!(deserialized.real_ip_header, "X-Real-IP");
    assert!(!deserialized.recursive);
}
