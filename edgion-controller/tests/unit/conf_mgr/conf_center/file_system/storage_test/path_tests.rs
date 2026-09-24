use super::*;

#[test]
fn build_resource_path_namespace_scoped_uses_ns_subdir() {
    let root = Path::new("/conf");
    assert_eq!(
        build_resource_path(root, "Service", Some("default"), "x"),
        Path::new("/conf/default/Service_default_x.yaml")
    );
}

#[test]
fn build_resource_path_cluster_scoped_uses_cluster_subdir() {
    let root = Path::new("/conf");
    assert_eq!(
        build_resource_path(root, "GatewayClass", None, "edgion"),
        Path::new("/conf/cluster/GatewayClass__edgion.yaml")
    );
    assert_eq!(
        build_resource_path(root, "GatewayClass", Some(""), "edgion"),
        Path::new("/conf/cluster/GatewayClass__edgion.yaml")
    );
}

#[test]
fn validate_path_components_accepts_legitimate_identifiers() {
    assert!(validate_path_components("HTTPRoute", Some("default"), "my-route").is_ok());
    assert!(validate_path_components("GatewayClass", None, "edgion").is_ok());
    assert!(validate_path_components("Service", Some(""), "svc").is_ok());
}

#[test]
fn validate_path_components_rejects_traversal_namespace() {
    // A bare ".." namespace would resolve `root.join("..")` one level above the conf root.
    assert!(validate_path_components("HTTPRoute", Some(".."), "x").is_err());
    assert!(validate_path_components("HTTPRoute", Some("a/b"), "x").is_err());
    assert!(validate_path_components("HTTPRoute", Some("a\\b"), "x").is_err());
}

#[test]
fn validate_path_components_rejects_traversal_kind_and_name() {
    assert!(validate_path_components("..", Some("default"), "x").is_err());
    assert!(validate_path_components("HTTPRoute", Some("default"), "..").is_err());
    assert!(validate_path_components("HTTPRoute", Some("default"), "a/b").is_err());
}

#[test]
fn absolutize_path_keeps_absolute_unchanged() {
    let abs = PathBuf::from("/etc/edgion/conf");
    assert_eq!(absolutize_path(abs.clone()), abs);
}

#[test]
fn absolutize_path_makes_relative_absolute() {
    let out = absolutize_path(PathBuf::from("relative/conf"));
    assert!(out.is_absolute(), "relative input must become absolute");
    assert!(out.ends_with("relative/conf"));
}
