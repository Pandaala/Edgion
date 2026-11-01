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
    let cfg: KubernetesConfig = yaml_serde::from_str(yaml).expect("parse");
    assert_eq!(cfg.cache_ready_deadline_secs, 90);
}
