use super::*;

#[test]
fn identity_follows_gateway_controller_contract() {
    for name in ["example.com/controller", "example.com/Team/a:1", "a/x"] {
        assert!(validate_controller_name(name).is_ok(), "{name}");
    }
    for name in [
        "",
        " ",
        "controller",
        "Example.com/a",
        "example.com/",
        "a..b/x",
        "-a/x",
        "a/x y",
        "a/x?y",
    ] {
        assert!(validate_controller_name(name).is_err(), "{name}");
    }
    assert!(validate_controller_name(&format!("a/{}", "x".repeat(251))).is_ok());
    assert!(validate_controller_name(&format!("a/{}", "x".repeat(252))).is_err());
}

#[test]
fn automatic_identity_is_stable_and_separates_deployments() {
    let resolve = |ns, deployment| resolve_kubernetes_controller_name(None, Some(ns), Some(deployment)).unwrap();
    assert_eq!(
        resolve("system", "controller-a"),
        "edgion.io/gateway-controller/system/controller-a"
    );
    assert_ne!(resolve("system", "controller-a"), resolve("system", "controller-b"));
    assert_ne!(resolve("system", "controller-a"), resolve("other", "controller-a"));
    assert_eq!(
        resolve_kubernetes_controller_name(Some("example.com/custom"), None, None).unwrap(),
        "example.com/custom"
    );
}

#[test]
fn missing_or_invalid_identity_never_falls_back() {
    assert!(require_controller_name(None).is_err());
    assert!(require_controller_name(Some(" ")).is_err());
    assert!(resolve_kubernetes_controller_name(Some(""), Some("system"), Some("controller")).is_err());
    for (ns, deployment) in [
        (None, Some("a")),
        (Some("a"), None),
        (Some(""), Some("a")),
        (Some("a"), Some("a/b")),
        (Some("a.b"), Some("a")),
    ] {
        assert!(resolve_kubernetes_controller_name(None, ns, deployment).is_err());
    }
    assert!(resolve_kubernetes_controller_name(None, Some("ns"), Some(&"a".repeat(253))).is_err());
    let oversized_deployment = ["a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(40)].join(".");
    assert!(resolve_kubernetes_controller_name(None, Some("ns"), Some(&oversized_deployment)).is_err());
}

#[test]
fn deserialization_preserves_explicit_empty_identity() {
    let omitted: super::super::KubernetesConfig = yaml_serde::from_str("{}").unwrap();
    let empty: super::super::KubernetesConfig = yaml_serde::from_str("controller_name: ''").unwrap();
    assert_eq!(omitted.controller_name, None);
    assert_eq!(empty.controller_name.as_deref(), Some(""));
}
