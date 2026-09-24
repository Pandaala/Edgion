use super::*;

#[test]
fn parse_tag_reference() {
    let r = OciImageRef::parse("registry.internal:5000/wasm/header-demo:1.2.0").unwrap();
    assert_eq!(r.registry, "registry.internal:5000");
    assert_eq!(r.repository, "wasm/header-demo");
    assert_eq!(r.tag.as_deref(), Some("1.2.0"));
    assert!(r.digest.is_none());
}

#[test]
fn parse_digest_reference() {
    let d = format!("sha256:{}", "a".repeat(64));
    let r = OciImageRef::parse(&format!("ghcr.io/org/mod@{d}")).unwrap();
    assert_eq!(r.digest.as_deref(), Some(d.as_str()));
    assert!(r.tag.is_none());
}

#[test]
fn parse_localhost_and_bare_repo() {
    assert!(OciImageRef::parse("localhost:5000/demo:1").is_ok());
    let r = OciImageRef::parse("registry.example.com/demo").unwrap();
    assert!(r.tag.is_none() && r.digest.is_none());
}

#[test]
fn parse_rejects_missing_registry_and_bad_digest() {
    assert!(OciImageRef::parse("just-a-repo:tag").is_err());
    assert!(OciImageRef::parse("repo-only").is_err());
    assert!(OciImageRef::parse("reg.io/repo@sha256:short").is_err());
    assert!(OciImageRef::parse("reg.io/repo@md5:abcd").is_err());
    assert!(OciImageRef::parse("reg.io/UPPER/repo:1").is_err());
    assert!(OciImageRef::parse("reg.io/repo:bad tag").is_err());
}

#[test]
fn effective_policy_matrix() {
    let parse = |s: &str| OciImageRef::parse(s).unwrap();
    let spec = |p: Option<OciPullPolicy>| WasmOciSpec {
        image: String::new(),
        pull_policy: p,
        ..Default::default()
    };
    // Digest pins even with an explicit Always.
    let pinned = parse(&format!("r.io/a@sha256:{}", "b".repeat(64)));
    assert_eq!(
        spec(Some(OciPullPolicy::Always)).effective_pull_policy(&pinned),
        OciPullPolicy::IfNotPresent
    );
    // Explicit policy wins for tag refs.
    let tagged = parse("r.io/a:1.0");
    assert_eq!(
        spec(Some(OciPullPolicy::Always)).effective_pull_policy(&tagged),
        OciPullPolicy::Always
    );
    // Unset: latest / no tag => Always; other tags => IfNotPresent.
    assert_eq!(
        spec(None).effective_pull_policy(&parse("r.io/a:latest")),
        OciPullPolicy::Always
    );
    assert_eq!(
        spec(None).effective_pull_policy(&parse("r.io/a")),
        OciPullPolicy::Always
    );
    assert_eq!(spec(None).effective_pull_policy(&tagged), OciPullPolicy::IfNotPresent);
}

#[test]
fn api_host_normalizes_docker_io() {
    assert_eq!(
        OciImageRef::parse("docker.io/org/mod:1").unwrap().api_host(),
        "registry-1.docker.io"
    );
    assert_eq!(OciImageRef::parse("ghcr.io/org/mod:1").unwrap().api_host(), "ghcr.io");
}

#[test]
fn resolved_pull_secret_redacted_under_admin_guard() {
    let spec = WasmOciSpec {
        image: "r.io/a:1".to_string(),
        resolved_pull_secret: Some("{\"auths\":{}}".to_string()),
        ..Default::default()
    };
    let plain = serde_json::to_value(&spec).unwrap();
    assert_eq!(plain["resolvedPullSecret"], "{\"auths\":{}}");
    let _guard = crate::redact::AdminRedactGuard::new();
    let redacted = serde_json::to_value(&spec).unwrap();
    assert_eq!(redacted["resolvedPullSecret"], "[redacted]");
}

#[test]
fn pull_secret_ref_deserializes_camel_case() {
    let spec: WasmOciSpec =
        serde_json::from_str(r#"{"image":"r.io/a:1","pullPolicy":"IfNotPresent","pullSecretRef":{"name":"creds"}}"#)
            .unwrap();
    assert_eq!(spec.pull_policy, Some(OciPullPolicy::IfNotPresent));
    assert_eq!(spec.pull_secret_ref.as_ref().unwrap().name, "creds");
}
