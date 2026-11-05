//! Built-in default Center RBAC policy.
//!
//! When `center.rbac` is absent the operator has not supplied an explicit
//! policy.  Rather than defaulting to deny-all (which would make federation
//! useless out of the box), we install a minimal allow set that covers the
//! normal Center workflows:
//!
//!  - Read (get / list / list-keys) on every ResourceKind except Secret.
//!  - Write (create / update / delete) on EdgionConfigData only
//!    (region-route and ip-restriction handlers drive this).
//!  - List, dedicated watch and failover on RegionRoute (synthetic classifier label, not a
//!    ResourceKind): list allows the effective read endpoint
//!    (GET /api/v1/region-routes/effective), and failover covers the POST
//!    failover endpoint.
//!  - server-info on the NonResource route (GET /api/v1/server-info). It exposes
//!    only a serving generation (server_id), readiness, and mode -- no secrets -- so
//!    it is safe to read by default; Center uses it to observe serving generations.
//!
//! Nothing else is allowed: no diagnostics, wipe-all.
//!
//! When `center.rbac` is `Some(...)` the operator's config is used verbatim;
//! the built-in default is NOT applied.

use crate::api::authz_classifier::NON_RESOURCE_KIND;
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
        ResourceKind::EdgionBackendTrafficPolicy => true,
        ResourceKind::EdgionBackend => true,
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
    ResourceKind::EdgionBackendTrafficPolicy.as_str(),
    ResourceKind::EdgionBackend.as_str(),
];

/// Build the built-in default Center policy.
///
/// Rules:
///   1. read (get, list, list-keys) on all ResourceKinds except Secret.
///   2. write (create, update, delete) on EdgionConfigData.
///   3. list, watch, failover on RegionRoute (synthetic label -- not a ResourceKind).
///      `list` covers the effective read endpoint
///      (GET /api/v1/region-routes/effective), which classifies as
///      (list, "RegionRoute"). `failover` covers the POST failover endpoint.
///      `get` is intentionally NOT granted: no classifier route currently emits
///      (get, "RegionRoute"), so granting it would be a dead permission that
///      widens the default boundary for any future single-resource read route.
///      RegionRoute is intentionally kept as its own rule because it is NOT part
///      of the ResourceKind exhaustiveness gate -- it is a synthetic classifier
///      label used for dedicated business APIs and the Center view Watch.
///   4. server-info on the NonResource route. Read-only and non-sensitive
///      (server_id / ready / mode only); Center reads it to observe serving generations.
pub(crate) fn default_center_policy() -> CenterPolicy {
    let read_kinds: Vec<String> = DEFAULT_READ_KINDS.iter().map(|s| s.to_string()).collect();

    let cfg = FedRbacConfig {
        allow: vec![
            // Rule 1: read verbs on all non-Secret ResourceKind variants.
            FedAllowRule {
                verbs: vec!["get".to_string(), "list".to_string(), "list-keys".to_string()],
                kinds: read_kinds,
            },
            // Rule 2: write verbs on EdgionConfigData only.
            FedAllowRule {
                verbs: vec!["create".to_string(), "update".to_string(), "delete".to_string()],
                kinds: vec![ResourceKind::EdgionConfigData.as_str().to_string()],
            },
            // Rule 3: list, dedicated watch and failover on RegionRoute (synthetic label).
            // list covers the effective region-route read endpoint;
            // failover covers the POST failover operation. `get` is omitted
            // because no classifier route emits (get, "RegionRoute").
            FedAllowRule {
                verbs: vec!["list".to_string(), "watch".to_string(), "failover".to_string()],
                kinds: vec!["RegionRoute".to_string()],
            },
            // Rule 4: server-info on the NonResource route. Read-only and
            // non-sensitive (server_id / ready / mode); used to observe serving generations.
            FedAllowRule {
                verbs: vec!["server-info".to_string()],
                kinds: vec![NON_RESOURCE_KIND.to_string()],
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
#[path = "../../tests/unit/fed_sync/default_policy_test.rs"]
mod tests;
