use super::*;

fn target(name: &str) -> DynUpstreamTarget {
    DynUpstreamTarget {
        name: name.to_string(),
        namespace: None,
    }
}

fn valid_config() -> CanaryConfig {
    CanaryConfig {
        enable: true,
        active_profile: "stable".to_string(),
        profiles: HashMap::from([
            ("stable".to_string(), target("app-v1")),
            ("canary".to_string(), target("app-v2")),
        ]),
        active_profile_ref: None,
        on_invalid: default_upstream_reject(),
    }
}

#[test]
fn valid_config_passes() {
    assert!(valid_config().check_fields().is_none());
}

#[test]
fn empty_profiles_is_rejected() {
    let mut cfg = valid_config();
    cfg.profiles.clear();
    let err = cfg.check_fields().unwrap();
    assert!(err.contains("at least one profile"));
}

#[test]
fn unknown_active_profile_is_rejected() {
    let mut cfg = valid_config();
    cfg.active_profile = "ghost".to_string();
    let err = cfg.check_fields().unwrap();
    assert!(err.contains("activeProfile") && err.contains("ghost"));
}

#[test]
fn resolve_profile_none_uses_active_profile() {
    let cfg = valid_config();
    let resolved = cfg.resolve_profile(None).unwrap();
    assert_eq!(resolved.name, "app-v1");
}

#[test]
fn resolve_profile_some_known_name_resolves_that_profile() {
    let cfg = valid_config();
    let resolved = cfg.resolve_profile(Some("canary")).unwrap();
    assert_eq!(resolved.name, "app-v2");
}

#[test]
fn resolve_profile_unknown_name_returns_none() {
    let cfg = valid_config();
    assert!(cfg.resolve_profile(Some("ghost")).is_none());
}

#[test]
fn validation_error_delegates_to_check_fields() {
    let mut cfg = valid_config();
    cfg.profiles.clear();
    assert_eq!(PluginValidation::validation_error(&cfg), cfg.check_fields());
}
