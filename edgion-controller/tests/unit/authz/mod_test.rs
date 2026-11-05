use super::*;
use crate::config::{FedAllowRule, FedRbacConfig};

fn policy(rules: Vec<(&[&str], &[&str])>) -> CenterPolicy {
    CenterPolicy::from_config(&FedRbacConfig {
        allow: rules
            .into_iter()
            .map(|(v, k)| FedAllowRule {
                verbs: v.iter().map(|s| s.to_string()).collect(),
                kinds: k.iter().map(|s| s.to_string()).collect(),
            })
            .collect(),
    })
}

#[test]
fn empty_policy_denies_all() {
    let az = Authorizer::new(CenterPolicy::deny_all());
    assert!(!az.authorize(&az.center_role(), Verb::Get, "EdgionConfigData").allowed());
}

#[test]
fn center_policy_matches_exact_and_wildcard() {
    let az = Authorizer::new(policy(vec![
        (&["get", "list"], &["EdgionConfigData"]),
        (&["server-info"], &["*"]),
    ]));
    assert!(az.authorize(&az.center_role(), Verb::Get, "EdgionConfigData").allowed());
    assert!(!az
        .authorize(&az.center_role(), Verb::Delete, "EdgionConfigData")
        .allowed());
    assert!(az
        .authorize(&az.center_role(), Verb::ServerInfo, "AnythingHere")
        .allowed());
    assert!(!az.authorize(&az.center_role(), Verb::Get, "Secret").allowed());
}

#[test]
fn policy_hot_swaps() {
    let az = Authorizer::new(CenterPolicy::deny_all());
    assert!(!az.authorize(&az.center_role(), Verb::Get, "EdgionConfigData").allowed());
    az.swap_policy(policy(vec![(&["get"], &["EdgionConfigData"])]));
    assert!(az.authorize(&az.center_role(), Verb::Get, "EdgionConfigData").allowed());
}

#[test]
fn center_policy_verb_is_case_insensitive() {
    let az = Authorizer::new(policy(vec![(&["GET", "List"], &["EdgionConfigData"])]));
    assert!(az.authorize(&az.center_role(), Verb::Get, "EdgionConfigData").allowed());
    assert!(az
        .authorize(&az.center_role(), Verb::List, "EdgionConfigData")
        .allowed());
}

#[test]
fn verb_roundtrips_string() {
    assert_eq!(Verb::Get.as_str(), "get");
    assert_eq!(Verb::from_str("failover"), Some(Verb::Failover));
    assert_eq!(Verb::ConfSyncRotate.as_str(), "conf-sync-rotate");
    assert_eq!(Verb::from_str("conf-sync-rotate"), Some(Verb::ConfSyncRotate));
    assert_eq!(Verb::from_str("nonsense"), None);
    assert_eq!(Verb::from_str("access-self"), None);
}

#[test]
fn access_self_is_never_policy_granted_even_by_wildcards() {
    let az = Authorizer::new(admin_wildcard_policy());
    assert!(!az
        .authorize(&az.center_role(), Verb::AccessSelf, "NonResource")
        .allowed());
}

#[test]
fn restricted_role_uses_its_own_policy() {
    let p = CenterPolicy::from_config(&FedRbacConfig {
        allow: vec![FedAllowRule {
            verbs: vec!["get".into()],
            kinds: vec!["HTTPRoute".into()],
        }],
    });
    let role = Role::Restricted(std::sync::Arc::new(p));
    let az = Authorizer::new(CenterPolicy::deny_all());
    assert!(az.authorize(&role, Verb::Get, "HTTPRoute").allowed());
    assert!(!az.authorize(&role, Verb::Delete, "HTTPRoute").allowed());
}

#[test]
fn cli_lookup_resolves_by_sha256() {
    let az = Authorizer::new(CenterPolicy::deny_all());
    let p = CenterPolicy::from_config(&FedRbacConfig {
        allow: vec![FedAllowRule {
            verbs: vec!["get".into()],
            kinds: vec!["EdgionConfigData".into()],
        }],
    });
    let mut m = std::collections::HashMap::new();
    let hash = crate::authz::sha256_hex("tok-abc");
    m.insert(
        hash,
        CliTokenEntry {
            name: "bot".into(),
            policy: std::sync::Arc::new(p),
        },
    );
    az.apply(CenterPolicy::deny_all(), m);
    let (role, name) = az.cli_lookup("tok-abc").expect("hit");
    assert_eq!(name, "bot");
    assert!(az.authorize(&role, Verb::Get, "EdgionConfigData").allowed());
    assert!(az.cli_lookup("wrong").is_none());
    assert!(az.cli_tokens_present());
}

#[test]
fn center_role_snapshots_policy() {
    let az = Authorizer::new(CenterPolicy::deny_all());
    let old_role = az.center_role();
    assert!(!az.authorize(&old_role, Verb::Get, "EdgionConfigData").allowed());
    az.swap_policy(CenterPolicy::from_config(&FedRbacConfig {
        allow: vec![FedAllowRule {
            verbs: vec!["get".into()],
            kinds: vec!["EdgionConfigData".into()],
        }],
    }));
    assert!(az.authorize(&az.center_role(), Verb::Get, "EdgionConfigData").allowed());
    let old_snapshot = old_role.access_snapshot().unwrap();
    let new_snapshot = az.center_role().access_snapshot().unwrap();
    assert_ne!(old_snapshot.revision, new_snapshot.revision);
    assert!(old_snapshot.resources.iter().all(|resource| resource.verbs.is_empty()));
}

// --- Local admin token (Istio-style) ---

#[test]
fn admin_wildcard_policy_allows_everything() {
    let p = admin_wildcard_policy();
    let role = Role::Restricted(Arc::new(p));
    let az = Authorizer::new(CenterPolicy::deny_all());
    assert!(az.authorize(&role, Verb::Delete, "Secret").allowed());
    assert!(az.authorize(&role, Verb::WipeAll, "AnythingHere").allowed());
}

#[test]
fn build_admin_token_entry_rejects_blank() {
    assert!(build_admin_token_entry(None).is_none());
    assert!(build_admin_token_entry(Some("")).is_none());
    assert!(build_admin_token_entry(Some("   ")).is_none());
    let (hash, entry) = build_admin_token_entry(Some("  tok-xyz  ")).expect("non-blank token");
    // Whitespace is trimmed before hashing.
    assert_eq!(hash, sha256_hex("tok-xyz"));
    assert_eq!(entry.name, ADMIN_TOKEN_NAME);
}

#[test]
fn admin_token_is_merged_with_scoped_tokens() {
    let az = Authorizer::new(CenterPolicy::deny_all());
    az.set_admin_token(build_admin_token_entry(Some("admin-secret")));

    // Startup apply with an empty config map: admin token must be live.
    az.apply(CenterPolicy::deny_all(), HashMap::new());
    let (role, name) = az
        .cli_lookup("admin-secret")
        .expect("admin token present after startup");
    assert_eq!(name, ADMIN_TOKEN_NAME);
    assert!(az.authorize(&role, Verb::Delete, "Secret").allowed());

    // Configuration assembly rebuilds the map from config (no admin token) — it must
    // still be re-merged automatically.
    let mut configured_map = HashMap::new();
    configured_map.insert(
        sha256_hex("center-bot"),
        CliTokenEntry {
            name: "bot".into(),
            policy: Arc::new(CenterPolicy::deny_all()),
        },
    );
    az.apply(CenterPolicy::deny_all(), configured_map);
    assert!(
        az.cli_lookup("admin-secret").is_some(),
        "admin token must remain available alongside scoped tokens"
    );
    assert!(az.cli_lookup("center-bot").is_some(), "center token also present");
}

#[test]
fn no_admin_token_means_no_extra_entry() {
    let az = Authorizer::new(CenterPolicy::deny_all());
    az.set_admin_token(build_admin_token_entry(Some("   "))); // blank → None
    az.apply(CenterPolicy::deny_all(), HashMap::new());
    assert!(!az.cli_tokens_present());
}
