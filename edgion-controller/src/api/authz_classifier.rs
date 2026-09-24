//! Maps an admin-router request to `(Verb, kind)` using the route axum actually
//! matched (`MatchedPath`) plus the `{kind}` path param. Default-deny on an
//! absent `MatchedPath` or an unrecognized route. Kind is normalized through
//! `from_kind_name` so authz and the handlers agree on one canonical kind.
//!
//! The route table here MUST mirror `create_admin_router`. Regenerate it when
//! routes change.

use http::Method;

use crate::authz::Verb;
use edgion_resources::ResourceKind;

/// Synthetic classifier kind for non-resource operational endpoints
/// (`server-info`, `diagnostics`, `reload`, `wipe-all`, `debug`). These verbs
/// never target a `ResourceKind`, so they carry this named sentinel instead of
/// `"*"`. Using a named kind keeps RBAC policies auditable: a reviewer reading a
/// rule sees `kinds: [NonResource]` — an explicit "non-resource operations"
/// scope — rather than a wildcard that looks like "grant everything".
pub const NON_RESOURCE_KIND: &str = "NonResource";

/// Normalize a wire kind param to the canonical kind string, falling back to the
/// raw value (which the policy will not match) for unknown kinds.
/// `pub`: also consumed by the authz middleware and the fed watch check.
pub fn normalize_kind(raw: &str) -> String {
    match ResourceKind::from_kind_name(raw) {
        Some(k) => k.as_str().to_string(),
        None => raw.to_string(),
    }
}

/// Map a (possibly already-normalized) kind to a BOUNDED metric label: a known
/// ResourceKind, the synthetic `RegionRoute`/`*` labels, or `"unknown"`. Never
/// emit raw attacker-supplied kind values as labels (cardinality protection).
/// `pub`: also consumed by the authz middleware and the fed watch check.
pub fn bounded_kind_label(kind: &str) -> &'static str {
    if kind == "*" {
        return "*";
    }
    if kind == "RegionRoute" {
        return "RegionRoute";
    }
    if kind == NON_RESOURCE_KIND {
        return NON_RESOURCE_KIND;
    }
    match ResourceKind::from_kind_name(kind) {
        Some(k) => k.as_str(),
        None => "unknown",
    }
}

/// Classify a matched request into `(Verb, kind)`. `None` => default-deny.
pub fn classify(method: &Method, matched_path: Option<&str>, kind_param: Option<&str>) -> Option<(Verb, String)> {
    let template = matched_path?; // absent MatchedPath => deny
    let m = method;

    // Literal (no-{kind}-param) routes first.
    match (m.as_str(), template) {
        ("GET", "/api/v1/access") => return Some((Verb::AccessSelf, NON_RESOURCE_KIND.into())),
        ("POST", "/api/v1/reload") => return Some((Verb::Reload, NON_RESOURCE_KIND.into())),
        ("GET", "/api/v1/server-info") => return Some((Verb::ServerInfo, NON_RESOURCE_KIND.into())),
        ("POST", "/api/v1/cluster-region-routes/failover") => return Some((Verb::Failover, "RegionRoute".into())),
        ("GET", "/api/v1/region-routes/effective") => return Some((Verb::List, "RegionRoute".into())),
        ("POST", "/api/v1/services/acme/{namespace}/{name}/trigger") => {
            return Some((Verb::AcmeTrigger, "Service".into()))
        }
        // Manual conf_sync cert rotation: a cluster-scoped operation on the
        // controller's OWN CA — it targets no ResourceKind, hence NON_RESOURCE_KIND.
        ("POST", "/api/v1/services/conf-sync-ca/rotate") => {
            return Some((Verb::ConfSyncRotate, NON_RESOURCE_KIND.into()))
        }
        // test-mode-only routes (registered only under is_test_mode()); axum never
        // matches these in production.
        ("POST", "/api/v1/admin/wipe-all") => return Some((Verb::WipeAll, NON_RESOURCE_KIND.into())),
        ("GET", "/api/v1/debug/store-stats") => return Some((Verb::Debug, NON_RESOURCE_KIND.into())),
        _ => {}
    }

    // Diagnostics: all sub-paths are GETs with at least one segment after the prefix; an absent or
    // odd path falls through to the resource-route block and default-denies safely.
    if m == Method::GET && template.starts_with("/api/v1/diagnostics/") {
        return Some((Verb::Diagnostics, NON_RESOURCE_KIND.into()));
    }

    // `POST /api/v1/validate/{kind}` is a read-only inspection query (schema +
    // unknown-field verdict on a submitted-but-not-persisted document), not a
    // mutation on any stored resource — classified alongside `Diagnostics`
    // rather than `Create`, and NON_RESOURCE_KIND rather than the `{kind}`
    // param, since it does not read or write per-kind stored state.
    if m == Method::POST && template == "/api/v1/validate/{kind}" {
        return Some((Verb::Diagnostics, NON_RESOURCE_KIND.into()));
    }

    // Resource routes carrying a {kind} param.
    // axum's MatchedPath retains the `{param}` braces, so these template strings match verbatim.
    let kind = normalize_kind(kind_param?);
    let verb = match (m.as_str(), template) {
        ("GET", "/api/v1/cluster/{kind}") => Verb::List,
        ("GET", "/api/v1/cluster/{kind}/{name}") => Verb::Get,
        ("GET", "/api/v1/namespaced/{kind}") => Verb::List,
        ("GET", "/api/v1/namespaced/{kind}/{namespace}") => Verb::List,
        ("GET", "/api/v1/namespaced/{kind}/{namespace}/{name}") => Verb::Get,
        ("GET", "/api/v1/keys/cluster/{kind}") => Verb::ListKeys,
        ("GET", "/api/v1/keys/namespaced/{kind}") => Verb::ListKeys,
        ("GET", "/api/v1/keys/namespaced/{kind}/{namespace}") => Verb::ListKeys,
        ("POST", "/api/v1/cluster/{kind}") => Verb::Create,
        ("PUT", "/api/v1/cluster/{kind}/{name}") => Verb::Update,
        ("DELETE", "/api/v1/cluster/{kind}/{name}") => Verb::Delete,
        ("POST", "/api/v1/namespaced/{kind}/{namespace}") => Verb::Create,
        ("PUT", "/api/v1/namespaced/{kind}/{namespace}/{name}") => Verb::Update,
        ("DELETE", "/api/v1/namespaced/{kind}/{namespace}/{name}") => Verb::Delete,
        ("GET", "/configserver/{kind}/list") => Verb::List,
        ("GET", "/configserver/{kind}") => Verb::Get,
        _ => return None, // unrecognized route => deny
    };
    Some((verb, kind))
}

#[cfg(test)]
#[path = "../../tests/unit/api/authz_classifier_test.rs"]
mod tests;
