use edgion_resources::resources::edgion_plugins::plugin_configs::{JwtAlgorithm, JwtIssuerGroup};
use edgion_resources::resources::gateway::SecretObjectReference;

fn make_group_with_overrides(
    name: &str,
    algorithm: Option<JwtAlgorithm>,
    audiences: Option<Vec<String>>,
) -> JwtIssuerGroup {
    JwtIssuerGroup {
        name: name.into(),
        description: None,
        issuers: vec!["https://x.example".into()],
        secret_refs: vec![SecretObjectReference {
            group: None,
            kind: None,
            name: "s1".into(),
            namespace: None,
        }],
        algorithm,
        audiences,
    }
}

// Tiny pure helper that mirrors the snapshot-merge core decision in
// `resolve_group` without requiring fetch_secret/HandlerContext.
fn snapshot_merge(
    config_algorithm: JwtAlgorithm,
    config_audiences: Option<Vec<String>>,
    group: &JwtIssuerGroup,
) -> (JwtAlgorithm, Option<Vec<String>>) {
    (
        group.algorithm.unwrap_or(config_algorithm),
        group.audiences.clone().or(config_audiences),
    )
}

#[test]
fn snapshot_uses_group_algorithm_when_set() {
    let group = make_group_with_overrides("g", Some(JwtAlgorithm::RS256), None);
    let (alg, _) = snapshot_merge(JwtAlgorithm::HS256, None, &group);
    assert_eq!(alg, JwtAlgorithm::RS256, "group override must win");
}

#[test]
fn snapshot_falls_back_to_plugin_algorithm() {
    let group = make_group_with_overrides("g", None, None);
    let (alg, _) = snapshot_merge(JwtAlgorithm::HS384, None, &group);
    assert_eq!(alg, JwtAlgorithm::HS384, "plugin-level fallback");
}

#[test]
fn snapshot_preserves_explicit_empty_audiences_override() {
    // Group sets `Some(vec![])` (= "no aud check for this group").
    // Must NOT fall back to plugin-level audiences.
    let group = make_group_with_overrides("g", None, Some(vec![]));
    let plugin_auds = Some(vec!["plugin-aud".to_string()]);
    let (_, auds) = snapshot_merge(JwtAlgorithm::HS256, plugin_auds, &group);
    assert_eq!(
        auds,
        Some(vec![]),
        "Some(vec![]) must be preserved as explicit no-aud override"
    );
}
