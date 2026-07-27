use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{Role, Verb};
use edgion_resources::resource::is_kind_cluster_scoped;
use edgion_resources::ResourceKind;

const SCHEMA_VERSION: u8 = 1;
const NON_RESOURCE_KIND: &str = "NonResource";

const RESOURCE_KINDS: [ResourceKind; 21] = [
    ResourceKind::GatewayClass,
    ResourceKind::EdgionGatewayConfig,
    ResourceKind::Gateway,
    ResourceKind::HTTPRoute,
    ResourceKind::Service,
    ResourceKind::EndpointSlice,
    ResourceKind::EdgionTls,
    ResourceKind::Secret,
    ResourceKind::EdgionPlugins,
    ResourceKind::GRPCRoute,
    ResourceKind::TCPRoute,
    ResourceKind::UDPRoute,
    ResourceKind::EdgionConfigData,
    ResourceKind::TLSRoute,
    ResourceKind::LinkSys,
    ResourceKind::EdgionStreamPlugins,
    ResourceKind::ReferenceGrant,
    ResourceKind::BackendTLSPolicy,
    ResourceKind::EdgionAcme,
    ResourceKind::ConfigMap,
    ResourceKind::EdgionBackendTrafficPolicy,
];

/// Compile-time guard that `RESOURCE_KINDS` still covers every `ResourceKind`.
///
/// The catalog is what `GET /api/v1/access` reports to Center. A kind missing
/// from it is silently absent from the snapshot — Center's UI would simply not
/// offer the resource, with no test failure and no runtime error. The array's
/// length is a fixed literal, so a cardinality assertion in a test cannot catch
/// this; only an exhaustive match can. Mirrors
/// `ResourceKind::_compile_time_sync_check` in `edgion-resources`.
///
/// If you add a `ResourceKind` variant, the compiler fails here until you also
/// decide whether it belongs in `RESOURCE_KINDS`. `Unspecified` is deliberately
/// excluded: it is a sentinel, not an addressable resource.
const fn _resource_kinds_catalog_is_exhaustive(kind: ResourceKind) {
    match kind {
        ResourceKind::Unspecified => {}
        ResourceKind::GatewayClass
        | ResourceKind::EdgionGatewayConfig
        | ResourceKind::Gateway
        | ResourceKind::HTTPRoute
        | ResourceKind::Service
        | ResourceKind::EndpointSlice
        | ResourceKind::EdgionTls
        | ResourceKind::Secret
        | ResourceKind::EdgionPlugins
        | ResourceKind::GRPCRoute
        | ResourceKind::TCPRoute
        | ResourceKind::UDPRoute
        | ResourceKind::EdgionConfigData
        | ResourceKind::TLSRoute
        | ResourceKind::LinkSys
        | ResourceKind::EdgionStreamPlugins
        | ResourceKind::ReferenceGrant
        | ResourceKind::BackendTLSPolicy
        | ResourceKind::EdgionAcme
        | ResourceKind::ConfigMap
        | ResourceKind::EdgionBackendTrafficPolicy => {}
    }
}

const RESOURCE_VERBS: [Verb; 7] = [
    Verb::Get,
    Verb::List,
    Verb::ListKeys,
    Verb::Watch,
    Verb::Create,
    Verb::Update,
    Verb::Delete,
];

const OPERATIONS: [(&str, Verb, &str); 9] = [
    ("regionRoute.list", Verb::List, "RegionRoute"),
    ("regionRoute.failover", Verb::Failover, "RegionRoute"),
    ("acme.trigger", Verb::AcmeTrigger, ResourceKind::Service.as_str()),
    ("confSync.rotate", Verb::ConfSyncRotate, NON_RESOURCE_KIND),
    ("reload", Verb::Reload, NON_RESOURCE_KIND),
    ("serverInfo", Verb::ServerInfo, NON_RESOURCE_KIND),
    ("diagnostics", Verb::Diagnostics, NON_RESOURCE_KIND),
    ("wipeAll", Verb::WipeAll, NON_RESOURCE_KIND),
    ("debug", Verb::Debug, NON_RESOURCE_KIND),
];

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccessSnapshot {
    pub schema_version: u8,
    pub revision: String,
    pub resources: Vec<ResourceAccess>,
    pub operations: Vec<OperationAccess>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ResourceAccess {
    pub kind: &'static str,
    pub scope: &'static str,
    pub verbs: Vec<&'static str>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct OperationAccess {
    pub name: &'static str,
    pub allowed: bool,
}

#[derive(Debug)]
pub struct AccessSnapshotError(serde_json::Error);

impl std::fmt::Display for AccessSnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to serialize the effective access snapshot: {}", self.0)
    }
}

impl std::error::Error for AccessSnapshotError {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RevisionInput<'a> {
    schema_version: u8,
    resources: &'a [ResourceAccess],
    operations: &'a [OperationAccess],
}

pub(super) fn snapshot(role: &Role) -> Result<AccessSnapshot, AccessSnapshotError> {
    let Role::Restricted(policy) = role;

    let resources = RESOURCE_KINDS
        .iter()
        .map(|kind| ResourceAccess {
            kind: kind.as_str(),
            scope: if is_kind_cluster_scoped(*kind) {
                "cluster"
            } else {
                "namespaced"
            },
            verbs: RESOURCE_VERBS
                .iter()
                .filter(|verb| policy.permits(**verb, kind.as_str()))
                .map(Verb::as_str)
                .collect(),
        })
        .collect::<Vec<_>>();

    let operations = OPERATIONS
        .iter()
        .map(|(name, verb, kind)| OperationAccess {
            name,
            allowed: policy.permits(*verb, kind),
        })
        .collect::<Vec<_>>();

    let canonical = serde_json::to_vec(&RevisionInput {
        schema_version: SCHEMA_VERSION,
        resources: &resources,
        operations: &operations,
    })
    .map_err(AccessSnapshotError)?;
    let revision = format!("sha256:{}", hex::encode(Sha256::digest(canonical)));

    Ok(AccessSnapshot {
        schema_version: SCHEMA_VERSION,
        revision,
        resources,
        operations,
    })
}

#[cfg(test)]
mod tests {
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
        assert_eq!(access.resources.len(), 21);
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
}
