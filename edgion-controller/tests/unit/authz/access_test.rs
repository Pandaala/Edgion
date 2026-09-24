use super::*;
use crate::authz::CenterPolicy;
use crate::config::{FedAllowRule, FedRbacConfig};
use std::sync::Arc;

fn role(rules: Vec<FedAllowRule>) -> Role {
    Role::Restricted(Arc::new(CenterPolicy::from_config(&FedRbacConfig { allow: rules })))
}

#[test]
fn deny_all_has_fixed_cardinality_and_canonical_serialization() {
    let access = snapshot(&role(Vec::new())).unwrap();
    assert_eq!(access.resources.len(), 22);
    assert!(access.resources.iter().all(|row| row.verbs.is_empty()));
    assert_eq!(access.operations.len(), 9);
    assert!(access.operations.iter().all(|operation| !operation.allowed));
    assert_eq!(access.resources[0].kind, "GatewayClass");
    assert_eq!(access.resources[0].scope, "cluster");
    assert_eq!(access.resources[2].scope, "namespaced");

    let json = serde_json::to_string(&access).unwrap();
    assert!(json.starts_with("{\"schemaVersion\":1,\"revision\":\"sha256:"));
    assert_eq!(snapshot(&role(Vec::new())).unwrap().revision, access.revision);
}

#[test]
fn explicit_write_only_policy_is_expanded_without_read_access() {
    let snapshot = snapshot(&role(vec![FedAllowRule {
        verbs: vec!["create".into(), "update".into()],
        kinds: vec!["HTTPRoute".into()],
    }]))
    .unwrap();
    let route = snapshot.resources.iter().find(|row| row.kind == "HTTPRoute").unwrap();
    assert_eq!(route.verbs, vec!["create", "update"]);
    assert!(snapshot
        .resources
        .iter()
        .filter(|row| row.kind != "HTTPRoute")
        .all(|row| row.verbs.is_empty()));
}

#[test]
fn wildcards_expand_only_bounded_resources_verbs_and_operations() {
    let snapshot = snapshot(&role(vec![FedAllowRule {
        verbs: vec!["*".into()],
        kinds: vec!["*".into()],
    }]))
    .unwrap();
    assert!(snapshot.resources.iter().all(|row| row.verbs.len() == 7));
    assert!(snapshot.operations.iter().all(|operation| operation.allowed));
}

#[test]
fn default_center_policy_expands_expected_secret_and_operation_boundaries() {
    let role = Role::Restricted(Arc::new(crate::fed_sync::default_policy::default_center_policy()));
    let snapshot = snapshot(&role).unwrap();
    let secret = snapshot.resources.iter().find(|row| row.kind == "Secret").unwrap();
    let config_data = snapshot
        .resources
        .iter()
        .find(|row| row.kind == "EdgionConfigData")
        .unwrap();
    assert!(secret.verbs.is_empty());
    assert_eq!(
        config_data.verbs,
        vec!["get", "list", "list-keys", "watch", "create", "update", "delete"]
    );
    assert!(
        snapshot
            .operations
            .iter()
            .find(|operation| operation.name == "regionRoute.list")
            .unwrap()
            .allowed
    );
    assert!(
        snapshot
            .operations
            .iter()
            .find(|operation| operation.name == "serverInfo")
            .unwrap()
            .allowed
    );
    // The default Center policy grants `reload` on the non-resource route
    // so the Center's reload action works without operator configuration.
    assert!(
        snapshot
            .operations
            .iter()
            .find(|operation| operation.name == "reload")
            .unwrap()
            .allowed
    );
}
