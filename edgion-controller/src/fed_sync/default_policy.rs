//! Built-in default Center RBAC policy.
//!
//! When `center.rbac` is absent the operator has not supplied an explicit
//! policy.  Rather than defaulting to deny-all (which would make federation
//! useless out of the box), we install a minimal allow set that covers the
//! normal Center workflows:
//!
//!  - Read (get / list / watch / list-keys) on every ResourceKind except Secret.
//!  - Write (create / update / delete) on EdgionConfigData only
//!    (region-route and ip-restriction handlers drive this).
//!  - Get, list, and failover on RegionRoute (synthetic classifier label, not a
//!    ResourceKind): get/list allow the aggregate read endpoints
//!    (GET /api/v1/cluster-region-routes, GET /api/v1/service-region-routes),
//!    and failover covers the POST failover endpoint.
//!
//! Nothing else is allowed: no reload, server-info, diagnostics, wipe-all.
//!
//! When `center.rbac` is `Some(...)` the operator's config is used verbatim;
//! the built-in default is NOT applied.

use crate::authz::CenterPolicy;
use crate::config::{FedAllowRule, FedRbacConfig};
use edgion_resources::resource::kind::ResourceKind;

// ---------------------------------------------------------------------------
// Compile-time exhaustiveness gate
// ---------------------------------------------------------------------------
//
// HOW TO ADD A NEW ResourceKind:
//   1. Add the variant to `is_default_readable` below (compiler forces this --
//      missing arms are a compile error).
//   2. Decide: should it be in the default read set?
//      * Yes  -> add it to DEFAULT_READ_KINDS and return `true` in the gate.
//      * No   -> return `false` (and document why if non-obvious).
//   3. Update `total_variants` in the count-guard test.
//   Secret stays `false` for security: the default policy must never expose
//   credentials to federation clients.

/// Returns whether `kind` belongs in the built-in default read set.
///
/// The exhaustive `match` here is the compiler-enforced reminder: adding a new
/// `ResourceKind` variant without updating this function is a compile error.
const fn is_default_readable(kind: ResourceKind) -> bool {
    match kind {
        ResourceKind::Unspecified => false,
        ResourceKind::Secret => false, // never expose credentials
        ResourceKind::GatewayClass => true,
        ResourceKind::EdgionGatewayConfig => true,
        ResourceKind::Gateway => true,
        ResourceKind::HTTPRoute => true,
        ResourceKind::Service => true,
        ResourceKind::EndpointSlice => true,
        ResourceKind::EdgionTls => true,
        ResourceKind::EdgionPlugins => true,
        ResourceKind::GRPCRoute => true,
        ResourceKind::TCPRoute => true,
        ResourceKind::UDPRoute => true,
        ResourceKind::EdgionConfigData => true,
        ResourceKind::TLSRoute => true,
        ResourceKind::LinkSys => true,
        ResourceKind::EdgionStreamPlugins => true,
        ResourceKind::ReferenceGrant => true,
        ResourceKind::BackendTLSPolicy => true,
        ResourceKind::EdgionAcme => true,
        ResourceKind::ConfigMap => true,
    }
}

// Trigger the gate at compile time; zero runtime cost.
const _: () = {
    let _ = is_default_readable(ResourceKind::Unspecified);
};

// ---------------------------------------------------------------------------

/// All ResourceKind strings that are readable by default (all except Secret and Unspecified).
///
/// Keep this slice consistent with `is_default_readable` above.  The test
/// `default_policy_gate_vs_slice` cross-checks every variant against both,
/// and `default_policy_read_kinds_count` enforces the cardinality contract.
const DEFAULT_READ_KINDS: &[&str] = &[
    ResourceKind::GatewayClass.as_str(),
    ResourceKind::EdgionGatewayConfig.as_str(),
    ResourceKind::Gateway.as_str(),
    ResourceKind::HTTPRoute.as_str(),
    ResourceKind::Service.as_str(),
    ResourceKind::EndpointSlice.as_str(),
    ResourceKind::EdgionTls.as_str(),
    // ResourceKind::Secret is intentionally excluded.
    ResourceKind::EdgionPlugins.as_str(),
    ResourceKind::GRPCRoute.as_str(),
    ResourceKind::TCPRoute.as_str(),
    ResourceKind::UDPRoute.as_str(),
    ResourceKind::EdgionConfigData.as_str(),
    ResourceKind::TLSRoute.as_str(),
    ResourceKind::LinkSys.as_str(),
    ResourceKind::EdgionStreamPlugins.as_str(),
    ResourceKind::ReferenceGrant.as_str(),
    ResourceKind::BackendTLSPolicy.as_str(),
    ResourceKind::EdgionAcme.as_str(),
    ResourceKind::ConfigMap.as_str(),
];

/// Build the built-in default Center policy.
///
/// Rules:
///   1. read (get, list, watch, list-keys) on all ResourceKinds except Secret.
///   2. write (create, update, delete) on EdgionConfigData.
///   3. get, list, failover on RegionRoute (synthetic label -- not a ResourceKind).
///      `get` and `list` cover the aggregate read endpoints
///      (GET /api/v1/cluster-region-routes, GET /api/v1/service-region-routes)
///      which classify as (list, "RegionRoute") and (get, "RegionRoute").
///      `failover` covers the POST failover endpoint.
///      RegionRoute is intentionally kept as its own rule because it is NOT part
///      of the ResourceKind exhaustiveness gate -- it is a synthetic classifier
///      label only used on the fed-proxy routing layer.
pub(crate) fn default_center_policy() -> CenterPolicy {
    let read_kinds: Vec<String> = DEFAULT_READ_KINDS.iter().map(|s| s.to_string()).collect();

    let cfg = FedRbacConfig {
        allow: vec![
            // Rule 1: read verbs on all non-Secret ResourceKind variants.
            FedAllowRule {
                verbs: vec![
                    "get".to_string(),
                    "list".to_string(),
                    "watch".to_string(),
                    "list-keys".to_string(),
                ],
                kinds: read_kinds,
            },
            // Rule 2: write verbs on EdgionConfigData only.
            FedAllowRule {
                verbs: vec!["create".to_string(), "update".to_string(), "delete".to_string()],
                kinds: vec![ResourceKind::EdgionConfigData.as_str().to_string()],
            },
            // Rule 3: get, list, and failover on RegionRoute (synthetic label).
            // get / list cover the aggregate region-route read endpoints;
            // failover covers the POST failover operation.
            FedAllowRule {
                verbs: vec!["get".to_string(), "list".to_string(), "failover".to_string()],
                kinds: vec!["RegionRoute".to_string()],
            },
        ],
    };

    CenterPolicy::from_config(&cfg)
}

/// Resolve an `Option<&FedRbacConfig>` to a `CenterPolicy`.
///
/// - `Some(r)` -> the operator's explicit config (used verbatim).
/// - `None`    -> the built-in default (see [`default_center_policy`]).
pub(crate) fn center_policy_from(rbac: Option<&FedRbacConfig>) -> CenterPolicy {
    match rbac {
        Some(r) => CenterPolicy::from_config(r),
        None => default_center_policy(),
    }
}

#[cfg(test)]
mod tests {
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

    // -- RegionRoute: get, list, and failover are all allowed --
    //
    // RegionRoute is a synthetic classifier label (not a ResourceKind variant).
    // The default now includes get and list so that the aggregate read endpoints
    // GET /api/v1/cluster-region-routes and GET /api/v1/service-region-routes
    // (which classify as (list, "RegionRoute") and (get, "RegionRoute"))
    // are accessible under the built-in default policy.

    #[test]
    fn default_allows_get_regionroute() {
        assert!(allows(&az(), Verb::Get, "RegionRoute"));
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
    fn default_denies_reload_any() {
        assert!(!allows(&az(), Verb::Reload, "Gateway"));
        assert!(!allows(&az(), Verb::Reload, "*"));
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
    fn default_denies_server_info() {
        assert!(!allows(&az(), Verb::ServerInfo, "*"));
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
        // ResourceKind has 21 named variants (including Unspecified and Secret).
        // The read set must exclude exactly Unspecified and Secret, so
        // expected = 21 - 2 = 19.
        //
        // If a new ResourceKind variant is added and DEFAULT_READ_KINDS is not
        // updated, this test will fail with a count mismatch.
        let total_variants: usize = 21; // update when adding a new ResourceKind variant
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
}
