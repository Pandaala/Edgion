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
        ("POST", "/api/v1/reload") => return Some((Verb::Reload, NON_RESOURCE_KIND.into())),
        ("GET", "/api/v1/server-info") => return Some((Verb::ServerInfo, NON_RESOURCE_KIND.into())),
        ("POST", "/api/v1/cluster-region-routes/failover") | ("POST", "/api/v1/service-region-routes/failover") => {
            return Some((Verb::Failover, "RegionRoute".into()))
        }
        ("GET", "/api/v1/region-routes/effective") => return Some((Verb::List, "RegionRoute".into())),
        ("GET", "/api/v1/cluster-region-routes") | ("GET", "/api/v1/service-region-routes") => {
            return Some((Verb::List, "RegionRoute".into()))
        }
        ("POST", "/api/v1/services/acme/{namespace}/{name}/trigger") => {
            return Some((Verb::AcmeTrigger, "Service".into()))
        }
        // Manual conf_sync cert rotation: a cluster-scoped operation on the
        // controller's OWN CA — it targets no ResourceKind, hence NON_RESOURCE_KIND.
        ("POST", "/api/v1/services/conf-sync-ca/rotate") => {
            return Some((Verb::ConfSyncRotate, NON_RESOURCE_KIND.into()))
        }
        // Effective (resolved) GlobalConnectionIpRestriction view for the Center
        // dashboard: a read-only projection of EdgionStreamPlugins, so it is
        // classified as `list` on that kind.
        ("GET", "/api/v1/global-ip-restrictions/effective") => return Some((Verb::List, "EdgionStreamPlugins".into())),
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
mod tests {
    use super::*;
    use crate::authz::Verb;
    use http::Method;

    // helper: (verb, normalized kind) or None (=> deny)
    fn classify(method: &str, matched: Option<&str>, kind_param: Option<&str>) -> Option<(Verb, String)> {
        super::classify(&Method::from_bytes(method.as_bytes()).unwrap(), matched, kind_param)
    }

    #[test]
    fn absent_matched_path_denies() {
        assert!(classify("GET", None, None).is_none());
    }

    #[test]
    fn namespaced_get_is_get_on_normalized_kind() {
        // path param is the lowercase wire kind; classifier normalizes it
        let (v, k) = classify(
            "GET",
            Some("/api/v1/namespaced/{kind}/{namespace}/{name}"),
            Some("edgionconfigdata"),
        )
        .unwrap();
        assert_eq!(v, Verb::Get);
        assert_eq!(k, "EdgionConfigData");
    }

    #[test]
    fn namespaced_collection_get_is_list() {
        let (v, _) = classify("GET", Some("/api/v1/namespaced/{kind}"), Some("edgionconfigdata")).unwrap();
        assert_eq!(v, Verb::List);
    }

    #[test]
    fn namespaced_post_is_create() {
        let (v, k) = classify(
            "POST",
            Some("/api/v1/namespaced/{kind}/{namespace}"),
            Some("edgionconfigdata"),
        )
        .unwrap();
        assert_eq!(v, Verb::Create);
        assert_eq!(k, "EdgionConfigData");
    }

    #[test]
    fn failover_literal_route_is_failover_regionroute() {
        let (v, k) = classify("POST", Some("/api/v1/cluster-region-routes/failover"), None).unwrap();
        assert_eq!(v, Verb::Failover);
        assert_eq!(k, "RegionRoute");
    }

    #[test]
    fn reload_is_nonresource_kind() {
        let (v, k) = classify("POST", Some("/api/v1/reload"), None).unwrap();
        assert_eq!(v, Verb::Reload);
        assert_eq!(k, NON_RESOURCE_KIND);
    }

    #[test]
    fn unknown_route_denies() {
        assert!(classify("GET", Some("/api/v1/does/not/exist"), None).is_none());
    }

    #[test]
    fn unknown_kind_param_passes_through_unnormalized() {
        // attacker-supplied unknown kind: classify still returns the verb but the
        // raw (unnormalized) kind, which the policy will not match and the metric
        // will bucket as "unknown".
        let (v, k) = classify("GET", Some("/api/v1/namespaced/{kind}/{namespace}/{name}"), Some("zzz")).unwrap();
        assert_eq!(v, Verb::Get);
        assert_eq!(k, "zzz");
    }

    #[test]
    fn service_region_routes_failover_is_failover_regionroute() {
        let (v, k) = classify("POST", Some("/api/v1/service-region-routes/failover"), None).unwrap();
        assert_eq!(v, Verb::Failover);
        assert_eq!(k, "RegionRoute");
    }

    #[test]
    fn cluster_region_routes_list_is_list_regionroute() {
        let (v, k) = classify("GET", Some("/api/v1/cluster-region-routes"), None).unwrap();
        assert_eq!(v, Verb::List);
        assert_eq!(k, "RegionRoute");
    }

    #[test]
    fn acme_trigger_is_acme_trigger_service() {
        let (v, k) = classify("POST", Some("/api/v1/services/acme/{namespace}/{name}/trigger"), None).unwrap();
        assert_eq!(v, Verb::AcmeTrigger);
        assert_eq!(k, "Service");
    }

    #[test]
    fn conf_sync_rotate_is_nonresource_kind() {
        let (v, k) = classify("POST", Some("/api/v1/services/conf-sync-ca/rotate"), None).unwrap();
        assert_eq!(v, Verb::ConfSyncRotate);
        assert_eq!(k, NON_RESOURCE_KIND);
    }

    #[test]
    fn global_ip_restrictions_effective_is_list_stream_plugins() {
        let (v, k) = classify("GET", Some("/api/v1/global-ip-restrictions/effective"), None).unwrap();
        assert_eq!(v, Verb::List);
        assert_eq!(k, "EdgionStreamPlugins");
    }

    #[test]
    fn server_info_is_server_info() {
        let (v, k) = classify("GET", Some("/api/v1/server-info"), None).unwrap();
        assert_eq!(v, Verb::ServerInfo);
        assert_eq!(k, NON_RESOURCE_KIND);
    }

    #[test]
    fn diagnostics_routes_are_diagnostics() {
        let (v, k) = classify("GET", Some("/api/v1/diagnostics/nested_ext_ref_usage"), None).unwrap();
        assert_eq!(v, Verb::Diagnostics);
        assert_eq!(k, NON_RESOURCE_KIND);

        let (v2, k2) = classify("GET", Some("/api/v1/diagnostics/conf-conflicts"), None).unwrap();
        assert_eq!(v2, Verb::Diagnostics);
        assert_eq!(k2, NON_RESOURCE_KIND);
    }

    #[test]
    fn configserver_list_is_list() {
        let (v, k) = classify("GET", Some("/configserver/{kind}/list"), Some("httproute")).unwrap();
        assert_eq!(v, Verb::List);
        assert_eq!(k, "HTTPRoute");
    }

    #[test]
    fn configserver_get_is_get() {
        let (v, k) = classify("GET", Some("/configserver/{kind}"), Some("gateway")).unwrap();
        assert_eq!(v, Verb::Get);
        assert_eq!(k, "Gateway");
    }

    #[test]
    fn cluster_scoped_crud() {
        let (v, k) = classify("GET", Some("/api/v1/cluster/{kind}"), Some("gatewayclass")).unwrap();
        assert_eq!(v, Verb::List);
        assert_eq!(k, "GatewayClass");

        let (v, k) = classify("GET", Some("/api/v1/cluster/{kind}/{name}"), Some("gatewayclass")).unwrap();
        assert_eq!(v, Verb::Get);
        assert_eq!(k, "GatewayClass");

        let (v, k) = classify("POST", Some("/api/v1/cluster/{kind}"), Some("gatewayclass")).unwrap();
        assert_eq!(v, Verb::Create);
        assert_eq!(k, "GatewayClass");

        let (v, k) = classify("PUT", Some("/api/v1/cluster/{kind}/{name}"), Some("gatewayclass")).unwrap();
        assert_eq!(v, Verb::Update);
        assert_eq!(k, "GatewayClass");

        let (v, k) = classify("DELETE", Some("/api/v1/cluster/{kind}/{name}"), Some("gatewayclass")).unwrap();
        assert_eq!(v, Verb::Delete);
        assert_eq!(k, "GatewayClass");
    }

    #[test]
    fn keys_routes_are_list_keys() {
        let (v, _) = classify("GET", Some("/api/v1/keys/cluster/{kind}"), Some("httproute")).unwrap();
        assert_eq!(v, Verb::ListKeys);

        let (v, _) = classify("GET", Some("/api/v1/keys/namespaced/{kind}"), Some("httproute")).unwrap();
        assert_eq!(v, Verb::ListKeys);

        let (v, _) = classify(
            "GET",
            Some("/api/v1/keys/namespaced/{kind}/{namespace}"),
            Some("httproute"),
        )
        .unwrap();
        assert_eq!(v, Verb::ListKeys);
    }

    #[test]
    fn wipe_all_and_debug_test_mode_routes() {
        let (v, k) = classify("POST", Some("/api/v1/admin/wipe-all"), None).unwrap();
        assert_eq!(v, Verb::WipeAll);
        assert_eq!(k, NON_RESOURCE_KIND);

        let (v, k) = classify("GET", Some("/api/v1/debug/store-stats"), None).unwrap();
        assert_eq!(v, Verb::Debug);
        assert_eq!(k, NON_RESOURCE_KIND);
    }

    #[test]
    fn wrong_method_on_valid_template_denies() {
        // DELETE on a collection/list template is not a real route -> deny
        assert!(classify("DELETE", Some("/api/v1/namespaced/{kind}"), Some("edgionconfigdata")).is_none());
    }

    #[test]
    fn bounded_kind_label_unknown_input() {
        assert_eq!(bounded_kind_label("zzz_unknown_kind"), "unknown");
        assert_eq!(bounded_kind_label("*"), "*");
        assert_eq!(bounded_kind_label("RegionRoute"), "RegionRoute");
        assert_eq!(bounded_kind_label(NON_RESOURCE_KIND), NON_RESOURCE_KIND);
        assert_eq!(bounded_kind_label("EdgionConfigData"), "EdgionConfigData");
    }

    #[test]
    fn normalize_kind_known_and_unknown() {
        assert_eq!(normalize_kind("edgionconfigdata"), "EdgionConfigData");
        assert_eq!(normalize_kind("HTTPRoute"), "HTTPRoute");
        assert_eq!(normalize_kind("zzz"), "zzz");
    }

    // ===================== route <-> classifier drift guard =====================
    //
    // The module doc says "the route table here MUST mirror `create_admin_router`".
    // Nothing used to enforce that, and the cost of a miss is severe: an admin route
    // with no `classify()` arm makes `authz_layer` default-deny, so the endpoint
    // returns 403 to EVERY caller — including the wildcard admin token — and the
    // request never even reaches policy evaluation. That is exactly how
    // `POST /api/v1/services/conf-sync-ca/rotate` shipped dead.
    //
    // This guard scrapes every `.route("<path>", <method_router>)` registration out
    // of the router source at compile time (`include_str!`) and asserts that each
    // (METHOD, path) pair classifies to `Some(..)`.

    /// The router source, embedded at compile time. Any `.route(...)` added to
    /// `api/mod.rs` is therefore automatically in scope for this guard.
    const ROUTER_SRC: &str = include_str!("mod.rs");

    /// Extract every `(METHOD, path)` pair registered via `.route(...)` in `src`.
    ///
    /// Scans for `.route(` and walks to the matching close paren with a depth
    /// counter (route registrations are frequently spread over several lines), then
    /// pulls the first string literal (the path template) and every HTTP method
    /// constructor (`get`/`post`/`put`/`delete`/`patch`, possibly chained as
    /// `get(a).post(b)`) out of that slice.
    fn scrape_routes(src: &str) -> Vec<(String, String)> {
        let method_re = regex::Regex::new(r"\b(get|post|put|delete|patch)\s*\(").unwrap();
        let path_re = regex::Regex::new(r#""([^"]*)""#).unwrap();
        let mut out = Vec::new();
        let bytes = src.as_bytes();
        let mut search_from = 0usize;
        while let Some(rel) = src[search_from..].find(".route(") {
            let open = search_from + rel + ".route(".len() - 1; // index of '('
            let mut depth = 0i32;
            let mut end = open;
            for (i, b) in bytes.iter().enumerate().skip(open) {
                match b {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let body = &src[open + 1..end];
            search_from = end + 1;

            let Some(path) = path_re.captures(body).map(|c| c[1].to_string()) else {
                continue;
            };
            // Methods are matched on the part AFTER the path literal so a path
            // segment such as `/get/...` can never be mistaken for a method.
            let after_path = &body[path_re.find(body).unwrap().end()..];
            for c in method_re.captures_iter(after_path) {
                out.push((c[1].to_uppercase(), path.clone()));
            }
        }
        out
    }

    #[test]
    fn scrape_routes_parses_single_and_chained_registrations() {
        // Self-test of the scraper, so a silently-broken regex cannot make the
        // drift guard below vacuously pass.
        let src = r#"
            Router::new()
                .route("/a/{kind}", get(h1).post(h2))
                .route(
                    "/b/c",
                    delete(h3),
                )
                .route("/d", get(h4).with_state(s));
        "#;
        let mut got = scrape_routes(src);
        got.sort();
        assert_eq!(
            got,
            vec![
                ("DELETE".to_string(), "/b/c".to_string()),
                ("GET".to_string(), "/a/{kind}".to_string()),
                ("GET".to_string(), "/d".to_string()),
                ("POST".to_string(), "/a/{kind}".to_string()),
            ]
        );
    }

    #[test]
    fn every_admin_route_is_classified() {
        let routes = scrape_routes(ROUTER_SRC);
        // Sanity: the scraper must actually see the router. If `api/mod.rs` is ever
        // restructured so the routes no longer live there, fail loudly rather than
        // pass on an empty set.
        assert!(
            routes.len() > 20,
            "route scraper found only {} routes in api/mod.rs — the guard has gone blind; \
             fix scrape_routes() or point ROUTER_SRC at the file that now holds create_admin_router()",
            routes.len()
        );

        let mut unclassified = Vec::new();
        for (method, path) in &routes {
            // Public paths (`/health`, `/ready`, `/metrics`, `/api/v1/auth/status`)
            // are exempt: `authz_layer` skips them via `public_paths::is_static_public`
            // BEFORE calling classify(), so they never need a classifier arm.
            if crate::api::public_paths::is_static_public(path) {
                continue;
            }
            // Test-mode-only routes (`wipe-all`, `debug/store-stats`) are NOT exempt:
            // they go through the same authz layer, and they do have classifier arms.
            let kind_param = if path.contains("{kind}") {
                Some("httproute")
            } else {
                None
            };
            let m = Method::from_bytes(method.as_bytes()).unwrap();
            if super::classify(&m, Some(path.as_str()), kind_param).is_none() {
                unclassified.push(format!("{method} {path}"));
            }
        }

        assert!(
            unclassified.is_empty(),
            "these admin routes have no arm in authz_classifier::classify():\n  {}\n\n\
             An unclassified admin route default-denies: authz_layer returns 403 to EVERY caller \
             (including the wildcard admin token) before any policy is evaluated, so the endpoint is \
             dead in production. Fix: add an arm to classify() for each route above and, if the \
             operation needs a new verb, a Verb variant in authz/mod.rs (variant + as_str() + \
             from_str()). Non-resource operational endpoints carry NON_RESOURCE_KIND.",
            unclassified.join("\n  ")
        );
    }
}
