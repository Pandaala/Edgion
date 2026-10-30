use super::*;
use crate::authz::{Authorizer, Decision, Verb};
use crate::config::FedAllowRule;

fn az() -> Authorizer {
    Authorizer::new(default_center_policy())
}

fn allows(az: &Authorizer, verb: Verb, kind: &str) -> bool {
    az.authorize(&az.center_role(), verb, kind) == Decision::Allow
}

// -- read permissions --

#[test]
fn default_allows_get_httproute() {
    assert!(allows(&az(), Verb::Get, "HTTPRoute"));
}

#[test]
fn default_allows_list_gateway() {
    assert!(allows(&az(), Verb::List, "Gateway"));
}

#[test]
fn default_allows_watch_edgionconfigdata() {
    assert!(allows(&az(), Verb::Watch, "EdgionConfigData"));
}

#[test]
fn default_allows_get_edgiontls() {
    assert!(allows(&az(), Verb::Get, "EdgionTls"));
}

#[test]
fn default_allows_listkeys_httproute() {
    assert!(allows(&az(), Verb::ListKeys, "HTTPRoute"));
}

// -- write on EdgionConfigData --

#[test]
fn default_allows_create_edgionconfigdata() {
    assert!(allows(&az(), Verb::Create, "EdgionConfigData"));
}

#[test]
fn default_allows_update_edgionconfigdata() {
    assert!(allows(&az(), Verb::Update, "EdgionConfigData"));
}

#[test]
fn default_allows_delete_edgionconfigdata() {
    assert!(allows(&az(), Verb::Delete, "EdgionConfigData"));
}

// -- RegionRoute: only list and failover are allowed --
//
// RegionRoute is a synthetic classifier label (not a ResourceKind variant).
// The effective read endpoint GET /api/v1/region-routes/effective classifies
// as (list, "RegionRoute"), so the default grants `list`.
// `get` is intentionally NOT granted: no classifier route emits
// (get, "RegionRoute"), so allowing it would be a dead permission.

#[test]
fn default_denies_get_regionroute() {
    // No classifier route emits (get, "RegionRoute"); the default must not
    // grant a dead permission that widens the boundary for future routes.
    assert!(!allows(&az(), Verb::Get, "RegionRoute"));
}

#[test]
fn default_allows_list_regionroute() {
    assert!(allows(&az(), Verb::List, "RegionRoute"));
}

#[test]
fn default_allows_failover_regionroute() {
    assert!(allows(&az(), Verb::Failover, "RegionRoute"));
}

// -- denials --

#[test]
fn default_denies_get_secret() {
    assert!(!allows(&az(), Verb::Get, "Secret"));
}

#[test]
fn default_denies_list_secret() {
    assert!(!allows(&az(), Verb::List, "Secret"));
}

#[test]
fn default_denies_create_httproute() {
    // write is EdgionConfigData-only; HTTPRoute must be denied for create.
    assert!(!allows(&az(), Verb::Create, "HTTPRoute"));
}

#[test]
fn default_denies_delete_gateway() {
    assert!(!allows(&az(), Verb::Delete, "Gateway"));
}

#[test]
fn default_allows_server_info() {
    // server-info is read-only and non-sensitive, allowed on the NonResource route.
    assert!(allows(&az(), Verb::ServerInfo, NON_RESOURCE_KIND));
}

#[test]
fn default_denies_server_info_on_resource_kind() {
    // The allowance is scoped to NonResource; it must not leak to resource kinds.
    assert!(!allows(&az(), Verb::ServerInfo, "Gateway"));
}

#[test]
fn default_denies_diagnostics() {
    assert!(!allows(&az(), Verb::Diagnostics, "*"));
}

#[test]
fn default_denies_wipe_all() {
    assert!(!allows(&az(), Verb::WipeAll, "*"));
}

// -- count guard: read kinds == total variants - 2 (Unspecified + Secret) --

#[test]
fn default_policy_read_kinds_count() {
    // ResourceKind has 23 named variants (including Unspecified and Secret).
    // The read set must exclude exactly Unspecified and Secret, so
    // expected = 23 - 2 = 21.
    //
    // If a new ResourceKind variant is added and DEFAULT_READ_KINDS is not
    // updated, this test will fail with a count mismatch.
    let total_variants: usize = 23; // update when adding a new ResourceKind variant
    let expected = total_variants - 2; // minus Unspecified and Secret
    assert_eq!(
        DEFAULT_READ_KINDS.len(),
        expected,
        "DEFAULT_READ_KINDS must contain every ResourceKind except Unspecified and Secret; \
             update default_policy.rs when adding a new ResourceKind variant \
             (total_variants={}, expected read-kinds={})",
        total_variants,
        expected
    );
}

// -- exhaustiveness gate cross-check: gate verdict must match slice membership --
//
// This test enumerates EVERY current ResourceKind variant and asserts that
// `is_default_readable(variant)` agrees with whether the variant's string
// appears in DEFAULT_READ_KINDS.  If the gate and the slice ever diverge
// (e.g. a variant is added to DEFAULT_READ_KINDS but `is_default_readable`
// returns false, or vice-versa), this test catches it immediately.

#[test]
fn default_policy_gate_vs_slice() {
    // All current variants listed explicitly so that this test itself fails to
    // compile when a new variant is added (via the exhaustive match in
    // `is_default_readable`).
    let all_variants = [
        ResourceKind::Unspecified,
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
        ResourceKind::EdgionBackend,
    ];

    for variant in &all_variants {
        let gate_says = is_default_readable(*variant);
        let in_slice = DEFAULT_READ_KINDS.contains(&variant.as_str());
        assert_eq!(
            gate_says, in_slice,
            "gate/slice mismatch for {:?}: is_default_readable={}, in DEFAULT_READ_KINDS={}; \
                 keep the gate and the slice in sync",
            variant, gate_says, in_slice
        );
    }
}

#[test]
fn default_policy_secret_not_in_read_kinds() {
    assert!(
        !DEFAULT_READ_KINDS.contains(&ResourceKind::Secret.as_str()),
        "Secret must NOT appear in DEFAULT_READ_KINDS"
    );
}

#[test]
fn default_policy_representatives_in_read_kinds() {
    for kind in &[
        ResourceKind::HTTPRoute.as_str(),
        ResourceKind::Gateway.as_str(),
        ResourceKind::EdgionTls.as_str(),
        ResourceKind::EdgionConfigData.as_str(),
    ] {
        assert!(
            DEFAULT_READ_KINDS.contains(kind),
            "expected {} in DEFAULT_READ_KINDS",
            kind
        );
    }
}

// -- explicit rbac override still works (deny-all config = deny everything) --

#[test]
fn explicit_empty_rbac_overrides_to_deny_all() {
    // An operator-supplied empty allow list must produce deny-all,
    // NOT the built-in default.
    let deny_all_cfg = FedRbacConfig { allow: vec![] };
    let policy = center_policy_from(Some(&deny_all_cfg));
    let az = Authorizer::new(policy);
    assert!(
        !allows(&az, Verb::Get, "HTTPRoute"),
        "explicit empty rbac must deny-all (center_policy_from Some(empty) != default)"
    );
}

#[test]
fn explicit_rbac_with_rules_overrides() {
    // An operator supplying an explicit rule gets exactly that.
    let cfg = FedRbacConfig {
        allow: vec![FedAllowRule {
            verbs: vec!["get".to_string()],
            kinds: vec!["Gateway".to_string()],
        }],
    };
    let policy = center_policy_from(Some(&cfg));
    let az = Authorizer::new(policy);
    assert!(
        allows(&az, Verb::Get, "Gateway"),
        "explicit rule must allow get/Gateway"
    );
    assert!(
        !allows(&az, Verb::Get, "HTTPRoute"),
        "explicit rule must not grant get/HTTPRoute"
    );
}

#[test]
fn none_rbac_gives_default_policy() {
    // center_policy_from(None) must give the built-in default.
    let policy = center_policy_from(None);
    let az = Authorizer::new(policy);
    assert!(allows(&az, Verb::Get, "HTTPRoute"), "None rbac must use default policy");
    assert!(
        !allows(&az, Verb::Get, "Secret"),
        "None rbac: Secret must still be denied"
    );
}
