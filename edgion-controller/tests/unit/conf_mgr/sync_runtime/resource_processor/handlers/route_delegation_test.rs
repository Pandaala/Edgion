use super::*;
use std::collections::HashMap;

// =====================================================================
// Test fixtures
// =====================================================================

struct FakeLookup {
    routes: HashMap<String, Arc<HTTPRoute>>,
    /// (from_ns, to_ns) pairs that are grant-allowed.
    allowed_cross_ns: HashSet<(String, String)>,
}

impl FakeLookup {
    fn new(routes: Vec<HTTPRoute>) -> Self {
        let mut map = HashMap::new();
        for r in routes {
            let key = format!(
                "{}/{}",
                r.metadata.namespace.as_deref().unwrap_or("default"),
                r.metadata.name.as_deref().unwrap_or("")
            );
            map.insert(key, Arc::new(r));
        }
        Self {
            routes: map,
            allowed_cross_ns: HashSet::new(),
        }
    }

    fn allow_cross_ns(mut self, from_ns: &str, to_ns: &str) -> Self {
        self.allowed_cross_ns.insert((from_ns.to_string(), to_ns.to_string()));
        self
    }
}

impl DelegationLookup for FakeLookup {
    fn get_route(&self, key: &str) -> Option<Arc<HTTPRoute>> {
        self.routes.get(key).cloned()
    }
    fn list_route_keys_in_namespace(&self, ns: &str) -> Vec<String> {
        let prefix = format!("{ns}/");
        let mut keys: Vec<String> = self.routes.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
        keys.sort_unstable();
        keys
    }
    fn cross_ns_ref_allowed(
        &self,
        from_ns: &str,
        _group: Option<&str>,
        _kind: Option<&str>,
        _name: &str,
        to_ns: &str,
    ) -> bool {
        self.allowed_cross_ns
            .contains(&(from_ns.to_string(), to_ns.to_string()))
    }
    fn terminal_ref_denied(&self, terminal: TerminalBackendReference<'_>) -> Option<RefDenied> {
        let query = terminal.grant_query();
        let source = query.source().scope().namespace().unwrap();
        let target = query.target().scope().namespace().unwrap();
        (source != target
            && !self
                .allowed_cross_ns
                .contains(&(source.to_string(), target.to_string())))
        .then(|| RefDenied {
            target_namespace: target.to_string(),
            target_name: query.target().name().to_string(),
            reason: Some("NoMatchingReferenceGrant".into()),
        })
    }
}

fn route(ns: &str, name: &str, spec: serde_json::Value) -> HTTPRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": name, "namespace": ns},
        "spec": spec
    }))
    .expect("route fixture should deserialize")
}

fn delegating_rule(prefix: &str, ns: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "matches": [{"path": {"type": "PathPrefix", "value": prefix}}],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
            "name": name, "namespace": ns
        }]
    })
}

fn parent_with_rules(rules: serde_json::Value) -> HTTPRoute {
    route("infra", "parent", serde_json::json!({"rules": rules}))
}

fn simple_parent(prefix: &str, child_ns: &str, child_name: &str) -> HTTPRoute {
    parent_with_rules(serde_json::json!([delegating_rule(prefix, child_ns, child_name)]))
}

fn service_rule(path: &str, svc: &str) -> serde_json::Value {
    serde_json::json!({
        "matches": [{"path": {"type": "PathPrefix", "value": path}}],
        "backendRefs": [{"name": svc, "port": 80}]
    })
}

fn rule_paths(rules: &[HTTPRouteRule]) -> Vec<&str> {
    rules
        .iter()
        .flat_map(|r| r.matches.iter().flatten())
        .filter_map(|m| m.path.as_ref().and_then(|p| p.value.as_deref()))
        .collect()
}

fn dropped_messages(outcome: &FlattenOutcome) -> Vec<&str> {
    outcome
        .issues
        .iter()
        .filter_map(|i| match i {
            DelegationIssue::RuleDropped { message } => Some(message.as_str()),
            _ => None,
        })
        .collect()
}

// =====================================================================
// Activation & passthrough
// =====================================================================

#[test]
fn external_auth_response_header_policy_delegation_and_recovery() {
    for parent_scope in [false, true] {
        for backend_scope in [false, true] {
            // Delegation references cannot carry backend filters at all;
            // backend-scoped auth belongs to the child's Service backend.
            if parent_scope && backend_scope {
                continue;
            }
            for header in ["Content-Length", "WWW-Authenticate"] {
                let filter = serde_json::json!({"type": "ExternalAuth", "externalAuth": {
                    "target": {"name": "auth", "port": 8080},
                    "decision": {"clientHeaders": [header]}
                }});
                let mut delegation = delegating_rule("/api", "infra", "child");
                let mut terminal = service_rule("/api/child", "child-svc");
                let target = if parent_scope { &mut delegation } else { &mut terminal };
                if backend_scope {
                    target["backendRefs"][0]["filters"] = serde_json::json!([filter]);
                } else {
                    target["filters"] = serde_json::json!([filter]);
                }
                let parent = parent_with_rules(serde_json::json!([delegation, service_rule("/health", "healthy")]));
                let child = route("infra", "child", serde_json::json!({"rules": [terminal]}));
                let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
                let rules = outcome.resolved_rules.as_ref().unwrap();
                let valid = header == "WWW-Authenticate";
                assert_eq!(rules.len(), if valid { 2 } else { 1 });
                assert!(rules
                    .iter()
                    .any(|rule| rule.backend_refs.as_ref().unwrap()[0].name == "healthy"));
                if !valid && !parent_scope {
                    assert!(dropped_messages(&outcome)
                        .iter()
                        .any(|message| message.contains("ExternalAuth")));
                }
            }
        }
    }
}

#[test]
fn malformed_parent_header_modifier_drops_only_its_delegating_rule() {
    let mut bad = delegating_rule("/api", "infra", "child");
    bad["filters"] = serde_json::json!([{"type": "RequestHeaderModifier"}]);
    let parent = parent_with_rules(serde_json::json!([bad, service_rule("/health", "healthy")]));
    let child = route(
        "infra",
        "child",
        serde_json::json!({"rules": [service_rule("/api", "child-svc")]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].backend_refs.as_ref().unwrap()[0].name, "healthy");
    assert!(outcome.expanded_children.is_empty());
}

#[test]
fn malformed_child_header_modifiers_drop_only_offending_rules() {
    let parent = simple_parent("/api", "infra", "child");
    let mut bad_rule = service_rule("/api/bad-rule", "bad-rule");
    bad_rule["filters"] = serde_json::json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {
        "set": [{"name": "bad name", "value": "yes"}]
    }}]);
    let mut bad_backend = service_rule("/api/bad-backend", "bad-backend");
    bad_backend["backendRefs"][0]["filters"] = serde_json::json!([{"type": "ResponseHeaderModifier"}]);
    let child = route(
        "infra",
        "child",
        serde_json::json!({"rules": [
            bad_rule, bad_backend, service_rule("/api/good", "healthy")
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.as_ref().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].backend_refs.as_ref().unwrap()[0].name, "healthy");
    let drops = dropped_messages(&outcome);
    assert_eq!(drops.len(), 2);
    assert!(drops[0].contains("rules[0]") && drops[1].contains("rules[1]"));
}

#[test]
fn inherited_prefix_transform_drops_only_child_with_non_prefix_match() {
    let mut delegation = delegating_rule("/api", "infra", "child");
    delegation["filters"] = serde_json::json!([{
        "type": "URLRewrite",
        "urlRewrite": {"path": {"type": "ReplacePrefixMatch", "replacePrefixMatch": "/v2"}}
    }]);
    let parent = parent_with_rules(serde_json::json!([delegation]));
    let exact = serde_json::json!({
        "matches": [{"path": {"type": "Exact", "value": "/api/exact"}}],
        "backendRefs": [{"name": "exact", "port": 80}]
    });
    let child = route(
        "infra",
        "child",
        serde_json::json!({"rules": [exact, service_rule("/api/healthy", "healthy")]}),
    );

    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));

    let rules = outcome.resolved_rules.as_ref().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].backend_refs.as_ref().unwrap()[0].name, "healthy");
    let drops = dropped_messages(&outcome);
    assert_eq!(drops.len(), 1);
    assert!(drops[0].contains("child 'infra/child' rules[0]"));
    assert!(drops[0].contains("prefix request transforms require PathPrefix"));
}

#[test]
fn credentialed_expose_headers_delegation_diagnostic_and_recovery() {
    let parent = simple_parent("/api", "infra", "child");
    for exposed in ["*", "X-Request-Id"] {
        let mut rule = service_rule("/api/child", "svc");
        rule["filters"] = serde_json::json!([{"type": "CORS", "cors": {
            "allowOrigins": ["https://app.example"], "allowCredentials": true,
            "exposeHeaders": [exposed]
        }}]);
        let child = route("infra", "child", serde_json::json!({"rules": [rule]}));
        let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
        if exposed == "*" {
            assert!(outcome.resolved_rules.as_ref().unwrap().is_empty());
            assert!(dropped_messages(&outcome)
                .iter()
                .any(|message| message.contains("child 'infra/child' rules[0]") && message.contains("exposeHeaders")));
        } else {
            assert_eq!(rule_paths(outcome.resolved_rules.as_ref().unwrap()), ["/api/child"]);
            assert!(outcome.issues.is_empty());
        }
    }
}

#[test]
fn native_cors_validation_detail_survives_delegation() {
    let parent = simple_parent("/api", "infra", "child");
    for (cors, expected_field) in [
        (serde_json::json!({"allowMethods": ["get"]}), "allowMethods"),
        (serde_json::json!({"allowHeaders": ["bad header"]}), "allowHeaders"),
        (serde_json::json!({"maxAge": 0}), "maxAge"),
    ] {
        let mut rule = service_rule("/api/child", "svc");
        rule["filters"] = serde_json::json!([{"type": "CORS", "cors": cors}]);
        let child = route("infra", "child", serde_json::json!({"rules": [rule]}));
        let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
        assert!(outcome.resolved_rules.as_ref().unwrap().is_empty());
        assert!(dropped_messages(&outcome)
            .iter()
            .any(|message| { message.contains("child 'infra/child' rules[0]") && message.contains(expected_field) }));
    }
}

#[test]
fn no_delegation_yields_none() {
    let parent = parent_with_rules(serde_json::json!([service_rule("/a", "svc")]));
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![]));
    assert!(outcome.resolved_rules.is_none());
    assert!(outcome.issues.is_empty());
}

#[test]
fn non_delegating_sibling_rules_preserved_in_order() {
    let parent = parent_with_rules(serde_json::json!([
        service_rule("/plain", "svc-plain"),
        delegating_rule("/api", "billing", "child"),
    ]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [service_rule("/api/pay", "svc-pay")]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.expect("delegation present");
    assert_eq!(rule_paths(&rules), vec!["/plain", "/api/pay"]);
}

// =====================================================================
// Prefix containment
// =====================================================================

#[test]
fn prefix_containment_matrix() {
    // Segment boundary: /api does not contain /apix
    assert!(prefix_contains("/api", "PathPrefix", "/api"));
    assert!(prefix_contains("/api", "PathPrefix", "/api/v1"));
    assert!(!prefix_contains("/api", "PathPrefix", "/apix"));
    assert!(!prefix_contains("/api", "PathPrefix", "/other"));
    // Root delegates everything
    assert!(prefix_contains("/", "PathPrefix", "/anything"));
    assert!(prefix_contains("/", "Exact", "/x"));
    // Exact must still be under the prefix
    assert!(prefix_contains("/api", "Exact", "/api/v1"));
    assert!(!prefix_contains("/api", "Exact", "/ap"));
    // Trailing slash on prefix is tolerated
    assert!(prefix_contains("/api/", "PathPrefix", "/api/v1"));
    // Regex: must be explicitly anchored — the gateway matches path
    // regexes UNANCHORED, so a pattern without `^` can match anywhere.
    assert!(prefix_contains("/api", "RegularExpression", "^/api/[0-9]+"));
    assert!(!prefix_contains("/api", "RegularExpression", "/api/[0-9]+"));
    assert!(!prefix_contains("/api", "RegularExpression", ".*/api"));
    // Regex boundary: /api followed by non-slash is rejected
    assert!(!prefix_contains("/api", "RegularExpression", "^/apix/[0-9]+"));
    // Unknown match type is never contained
    assert!(!prefix_contains("/api", "Bogus", "/api/x"));
}

#[test]
fn regex_alternation_cannot_escape_prefix() {
    // `|` has the lowest precedence: `^/api/x|/evil` is `(^/api/x)|(/evil)`,
    // and the gateway matches path regexes unanchored — the second branch
    // would route `/evil`. Any unescaped `|` must be rejected.
    assert!(!prefix_contains("/api", "RegularExpression", "^/api/x|/evil"));
    assert!(!prefix_contains("/api", "RegularExpression", "^/api/(x|y)"));
    assert!(!prefix_contains("/", "RegularExpression", "^/a|/b"));
    // An escaped literal pipe is not alternation and stays acceptable.
    assert!(prefix_contains("/api", "RegularExpression", r"^/api/x\|y"));
}

#[test]
fn dot_segment_child_path_cannot_escape_prefix() {
    // Containment must be decided on the NORMALIZED value: the gateway
    // normalizes resolved_rules paths, so `/api/../admin` would compile as
    // `/admin` — outside the delegated `/api` prefix.
    let parent = simple_parent("/api", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            service_rule("/api/../admin", "svc"),
            service_rule("/api/./ok", "svc"),
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    // The escape attempt is dropped; the benign dot-segment path survives
    // and is stored NORMALIZED (so the gateway's own pass is idempotent).
    assert_eq!(rule_paths(&rules), vec!["/api/ok"]);
    let drops = dropped_messages(&outcome);
    assert_eq!(drops.len(), 1);
    assert!(drops[0].contains("rules[0]"), "drop message: {}", drops[0]);
}

#[test]
fn delegating_prefix_is_normalized_before_containment() {
    // A parent prefix carrying dot-segments must be normalized too, or the
    // child's (normalized) paths would never match it.
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{"path": {"type": "PathPrefix", "value": "/api/v1/./billing"}}],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [service_rule("/api/v1/billing/pay", "svc")]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    assert_eq!(
        rule_paths(&outcome.resolved_rules.clone().unwrap()),
        vec!["/api/v1/billing/pay"]
    );
}

#[test]
fn out_of_prefix_child_rule_dropped_with_issue() {
    let parent = simple_parent("/api/v1/billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            service_rule("/api/v1/billing/pay", "svc-pay"),
            service_rule("/evil", "svc-evil"),
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rule_paths(&rules), vec!["/api/v1/billing/pay"]);
    let drops = dropped_messages(&outcome);
    assert_eq!(drops.len(), 1);
    assert!(
        drops[0].contains("rules[1]"),
        "drop message should name the rule: {}",
        drops[0]
    );
}

#[test]
fn child_match_without_path_inherits_prefix() {
    let parent = simple_parent("/api", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{"method": "GET"}],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rule_paths(&rules), vec!["/api"]);
    assert_eq!(rules[0].matches.as_ref().unwrap()[0].method.as_deref(), Some("GET"));
}

#[test]
fn child_rule_without_matches_inherits_prefix() {
    let parent = simple_parent("/api", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{"backendRefs": [{"name": "svc", "port": 80}]}]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rule_paths(&rules), vec!["/api"]);
}

// =====================================================================
// Constraint merge
// =====================================================================

#[test]
fn parent_constraints_merged_into_child_matches() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{
            "path": {"type": "PathPrefix", "value": "/api"},
            "headers": [{"name": "x-tenant", "value": "b"}],
            "method": "GET"
        }],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{
                "path": {"type": "PathPrefix", "value": "/api/pay"},
                "headers": [{"name": "x-extra", "value": "1"}]
            }],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    let m = &rules[0].matches.as_ref().unwrap()[0];
    let header_names: Vec<&str> = m.headers.as_ref().unwrap().iter().map(|h| h.name.as_str()).collect();
    assert!(header_names.contains(&"x-extra") && header_names.contains(&"x-tenant"));
    assert_eq!(m.method.as_deref(), Some("GET"));
}

#[test]
fn duplicate_headers_preserve_effective_parent_and_child_constraints() {
    for child_value in ["one", "two"] {
        let parent = parent_with_rules(serde_json::json!([{
            "matches": [{"path": {"type": "PathPrefix", "value": "/api"}, "headers": [
                {"name": "X-Mode", "value": "one"},
                {"name": "x-mode", "value": "ignored-parent"}
            ]}],
            "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                "name": "child", "namespace": "billing"}]
        }]));
        let child = route(
            "billing",
            "child",
            serde_json::json!({"rules": [{
                "matches": [{"path": {"type": "PathPrefix", "value": "/api/pay"}, "headers": [
                    {"name": "x-mode", "value": child_value},
                    {"name": "X-Mode", "type": "RegularExpression", "value": "["},
                    {"name": "X-Other", "value": "west"}
                ]}],
                "backendRefs": [{"name": "svc", "port": 80}]
            }]}),
        );
        let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
        let rules = outcome.resolved_rules.as_ref().unwrap();
        if child_value == "one" {
            assert!(dropped_messages(&outcome).is_empty());
            let headers = rules[0].matches.as_ref().unwrap()[0].headers.as_ref().unwrap();
            assert_eq!(headers.len(), 2);
            assert_eq!(headers[0].value, "one");
            assert_eq!(headers[1].name, "X-Other");
        } else {
            assert!(rules.is_empty());
            assert_eq!(dropped_messages(&outcome).len(), 1);
        }
    }
}

#[test]
fn conflicting_header_or_method_drops_child_rule() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{
            "path": {"type": "PathPrefix", "value": "/api"},
            "headers": [{"name": "x-tenant", "value": "b"}],
            "method": "GET"
        }],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            {
                "matches": [{
                    "path": {"type": "PathPrefix", "value": "/api/a"},
                    "headers": [{"name": "X-Tenant", "value": "OTHER"}]
                }],
                "backendRefs": [{"name": "svc", "port": 80}]
            },
            {
                "matches": [{"path": {"type": "PathPrefix", "value": "/api/b"}, "method": "POST"}],
                "backendRefs": [{"name": "svc", "port": 80}]
            }
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert_eq!(dropped_messages(&outcome).len(), 2);
}

// =====================================================================
// Filter inheritance
// =====================================================================

#[test]
fn malformed_extension_ref_in_child_drops_only_that_http_rule() {
    let parent = simple_parent("/api", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            {
                "matches": [{"path": {"type": "PathPrefix", "value": "/api/bad"}}],
                "filters": [{"type": "ExtensionRef"}],
                "backendRefs": [{"name": "svc", "port": 80}]
            },
            {
                "matches": [{"path": {"type": "PathPrefix", "value": "/api/good"}}],
                "backendRefs": [{"name": "svc", "port": 80}]
            }
        ]}),
    );

    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    assert_eq!(outcome.resolved_rules.as_ref().unwrap().len(), 1);
    let drops = dropped_messages(&outcome);
    assert_eq!(drops.len(), 1);
    assert!(drops[0].contains("rules[0]"));
}

#[test]
fn mirror_delegation_preserves_declaring_namespace_and_revalidates_raw_children() {
    use super::super::request_mirror;
    for backend_filter in [false, true] {
        for target_ns in [None, Some("infra"), Some("shadow")] {
            let mirror = serde_json::json!({"type": "RequestMirror", "requestMirror": {
                "backendRef": {"name": "copy", "namespace": target_ns, "port": 80,
                    "refDenied": {"targetNamespace": "forged", "targetName": "forged"}}
            }});
            let mut terminal = service_rule("/api", "primary");
            if backend_filter {
                terminal["backendRefs"][0]["filters"] = serde_json::json!([mirror]);
            } else {
                terminal["filters"] = serde_json::json!([mirror]);
            }
            let parent = simple_parent("/api", "billing", "child");
            let child = route("billing", "child", serde_json::json!({"rules": [terminal]}));
            for allowed in [false, true] {
                let mut lookup = FakeLookup::new(vec![child.clone()]);
                if allowed {
                    lookup = lookup.allow_cross_ns("billing", target_ns.unwrap_or("billing"));
                }
                let result = flatten_route(&parent, &lookup);
                let rules = result.resolved_rules.as_ref().unwrap();
                assert_eq!(rules.len(), 1);
                let backend = request_mirror::refs(&rules[0]).next().unwrap();
                assert_eq!(backend.namespace.as_deref(), Some(target_ns.unwrap_or("billing")));
                assert_eq!(backend.ref_denied.is_some(), target_ns.is_some() && !allowed);
                if let Some(ns) = target_ns {
                    assert!(result.cross_ns_target_namespaces.contains(ns));
                }
            }
        }
    }
}

#[test]
fn mirror_inherited_from_intermediate_route_keeps_its_identity() {
    use super::super::request_mirror;
    let parent = simple_parent("/api", "billing", "middle");
    let mut middle_rule = delegating_rule("/api", "leaf", "terminal");
    middle_rule["filters"] = serde_json::json!([{"type": "RequestMirror", "requestMirror": {
        "backendRef": {"name": "copy", "namespace": "shadow", "port": 80}
    }}]);
    let middle = route("billing", "middle", serde_json::json!({"rules": [middle_rule]}));
    let leaf = route(
        "leaf",
        "terminal",
        serde_json::json!({"rules": [service_rule("/api", "primary")]}),
    );
    for grant_from in ["infra", "leaf", "billing"] {
        let lookup = FakeLookup::new(vec![middle.clone(), leaf.clone()]).allow_cross_ns(grant_from, "shadow");
        let result = flatten_route(&parent, &lookup);
        let rules = result.resolved_rules.as_ref().unwrap();
        assert_eq!(rules.len(), 1);
        let backend = request_mirror::refs(&rules[0]).next().unwrap();
        assert_eq!(backend.ref_denied.is_none(), grant_from == "billing");
        assert!(result.cross_ns_target_namespaces.contains("shadow"));
    }
}

#[test]
fn parent_filters_prepended_and_extension_refs_concatenate() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
        "filters": [
            {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-p", "value": "1"}]}},
            {"type": "ExtensionRef", "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "parent-plugin"}}
        ],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{"path": {"type": "PathPrefix", "value": "/api/pay"}}],
            "filters": [
                {"type": "ExtensionRef", "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "child-plugin"}}
            ],
            "backendRefs": [{
                "name": "svc", "port": 80,
                "filters": [{"type": "ExtensionRef", "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "child-backend-plugin"
                }}]
            }]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    let filters = rules[0].filters.as_ref().unwrap();
    // Order: inherited standard, inherited extension, child's own
    assert_eq!(filters.len(), 3);
    assert_eq!(filters[0].filter_type, HTTPRouteFilterType::RequestHeaderModifier);
    assert_eq!(
        filters[1].extension_ref.as_ref().map(|e| e.name.as_str()),
        Some("parent-plugin")
    );
    assert_eq!(
        filters[2].extension_ref.as_ref().map(|e| e.name.as_str()),
        Some("child-plugin")
    );
    assert_eq!(
        filters[1]
            .extension_ref
            .as_ref()
            .and_then(|e| e.resolved_namespace.as_deref()),
        Some("infra")
    );
    assert_eq!(
        filters[2]
            .extension_ref
            .as_ref()
            .and_then(|e| e.resolved_namespace.as_deref()),
        Some("billing")
    );
    assert_eq!(
        rules[0].backend_refs.as_ref().unwrap()[0].filters.as_ref().unwrap()[0]
            .extension_ref
            .as_ref()
            .and_then(|e| e.resolved_namespace.as_deref()),
        Some("billing")
    );
}

#[test]
fn child_backend_ref_filter_cannot_smuggle_mandatory_override() {
    // A child must not evade the mandatory-filter conflict check by
    // declaring the same-type standard filter at backendRef level.
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
        "filters": [
            {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-p", "value": "1"}]}}
        ],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{"path": {"type": "PathPrefix", "value": "/api/pay"}}],
            "backendRefs": [{
                "name": "svc", "port": 80,
                "filters": [
                    {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-p", "value": "override"}]}}
                ]
            }]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    let drops = dropped_messages(&outcome);
    assert_eq!(drops.len(), 1);
    assert!(drops[0].contains("RequestHeaderModifier"));
}

#[test]
fn child_standard_filter_conflicting_with_inherited_type_drops_rule() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
        "filters": [
            {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-p", "value": "1"}]}}
        ],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{"path": {"type": "PathPrefix", "value": "/api/pay"}}],
            "filters": [
                {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-c", "value": "2"}]}}
            ],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    let drops = dropped_messages(&outcome);
    assert_eq!(drops.len(), 1);
    assert!(drops[0].contains("RequestHeaderModifier"));
}

// =====================================================================
// Attribute inheritance
// =====================================================================

#[test]
fn attrs_child_first_else_inherited_across_two_levels() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
        "timeouts": {"request": "30s"},
        "retry": {"attempts": 5},
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
            "name": "mid", "namespace": "billing"
        }]
    }]));
    // Mid level overrides retry, delegates deeper.
    let mid = route(
        "billing",
        "mid",
        serde_json::json!({"rules": [{
            "matches": [{"path": {"type": "PathPrefix", "value": "/api/pay"}}],
            "retry": {"attempts": 2},
            "backendRefs": [{
                "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                "name": "leaf", "namespace": "billing"
            }]
        }]}),
    );
    let leaf = route(
        "billing",
        "leaf",
        serde_json::json!({"rules": [{
            "matches": [{"path": {"type": "PathPrefix", "value": "/api/pay/now"}}],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![mid, leaf]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules.len(), 1);
    // timeouts from root, retry from mid (nearest ancestor override)
    assert_eq!(
        rules[0].timeouts.as_ref().and_then(|t| t.request.as_deref()),
        Some("30s")
    );
    assert_eq!(rules[0].retry.as_ref().and_then(|r| r.attempts), Some(2));
    // Transitive dependency registration: root depends on both mid and leaf
    assert!(outcome.named_children.contains("billing/mid"));
    assert!(outcome.named_children.contains("billing/leaf"));
}

// =====================================================================
// Wildcard, reverse lock, hostnames
// =====================================================================

#[test]
fn wildcard_expands_all_children_sorted_excluding_parent() {
    let parent = route(
        "infra",
        "parent",
        serde_json::json!({"rules": [{
            "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
            "backendRefs": [{
                "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                "name": "*", "namespace": "infra"
            }]
        }]}),
    );
    let a = route(
        "infra",
        "a-child",
        serde_json::json!({"rules": [service_rule("/api/a", "svc-a")]}),
    );
    let b = route(
        "infra",
        "b-child",
        serde_json::json!({"rules": [service_rule("/api/b", "svc-b")]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![parent.clone(), a, b]));
    let rules = outcome.resolved_rules.clone().unwrap();
    // Sorted by key; the parent itself (also in ns) is excluded
    assert_eq!(rule_paths(&rules), vec!["/api/a", "/api/b"]);
    assert!(outcome.wildcard_namespaces.contains("infra"));
    assert!(outcome.issues.is_empty(), "issues: {:?}", outcome.issues);
}

#[test]
fn wildcard_zero_children_is_silent() {
    let parent = simple_parent("/api", "empty-ns", "*");
    let mut parent = parent;
    parent.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].name = "*".to_string();
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert!(outcome.issues.is_empty());
}

#[test]
fn reverse_lock_denies_unlisted_parent_and_allows_listed() {
    let parent = simple_parent("/api", "billing", "locked");
    let locked_to_other = route(
        "billing",
        "locked",
        serde_json::json!({
            "parentRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                             "name": "someone-else", "namespace": "infra"}],
            "rules": [service_rule("/api/pay", "svc")]
        }),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![locked_to_other]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert!(dropped_messages(&outcome)[0].contains("parentRefs restrict delegation"));

    let locked_to_us = route(
        "billing",
        "locked",
        serde_json::json!({
            "parentRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                             "name": "parent", "namespace": "infra"}],
            "rules": [service_rule("/api/pay", "svc")]
        }),
    );
    let parent2 = simple_parent("/api", "billing", "locked");
    let outcome2 = flatten_route(&parent2, &FakeLookup::new(vec![locked_to_us]));
    assert_eq!(rule_paths(&outcome2.resolved_rules.clone().unwrap()), vec!["/api/pay"]);
}

#[test]
fn gateway_parent_refs_do_not_reverse_lock() {
    // A child that (only) attaches to a Gateway is still delegatable.
    let parent = simple_parent("/api", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({
            "parentRefs": [{"name": "some-gateway"}],
            "rules": [service_rule("/api/pay", "svc")]
        }),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    assert_eq!(rule_paths(&outcome.resolved_rules.clone().unwrap()), vec!["/api/pay"]);
}

#[test]
fn child_with_hostnames_is_skipped() {
    let parent = simple_parent("/api", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({
            "hostnames": ["evil.example.com"],
            "rules": [service_rule("/api/pay", "svc")]
        }),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert!(dropped_messages(&outcome)[0].contains("hostnames"));
}

// =====================================================================
// Cycles, depth, missing children
// =====================================================================

#[test]
fn self_cycle_and_two_cycle_dropped() {
    // parent delegates to itself (named)
    let self_parent = simple_parent("/api", "infra", "parent");
    let outcome = flatten_route(&self_parent, &FakeLookup::new(vec![self_parent.clone()]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert!(dropped_messages(&outcome)[0].contains("DelegationLoop"));

    // a → b → a
    let a = route(
        "infra",
        "a",
        serde_json::json!({"rules": [delegating_rule("/api", "infra", "b")]}),
    );
    let b = route(
        "infra",
        "b",
        serde_json::json!({"rules": [delegating_rule("/api/x", "infra", "a")]}),
    );
    let outcome2 = flatten_route(&a, &FakeLookup::new(vec![a.clone(), b]));
    assert!(outcome2.resolved_rules.clone().unwrap().is_empty());
    assert!(dropped_messages(&outcome2).iter().any(|m| m.contains("DelegationLoop")));
}

#[test]
fn diamond_is_legal() {
    // parent → {mid1, mid2} → leaf : leaf expanded twice, no loop issue.
    let parent = parent_with_rules(serde_json::json!([
        delegating_rule("/api/a", "infra", "mid1"),
        delegating_rule("/api/b", "infra", "mid2"),
    ]));
    let mid1 = route(
        "infra",
        "mid1",
        serde_json::json!({"rules": [delegating_rule("/api/a/leaf", "infra", "leaf")]}),
    );
    let mid2 = route(
        "infra",
        "mid2",
        serde_json::json!({"rules": [delegating_rule("/api/b/leaf", "infra", "leaf")]}),
    );
    let leaf = route(
        "infra",
        "leaf",
        serde_json::json!({"rules": [
            service_rule("/api/a/leaf/x", "svc"),
            service_rule("/api/b/leaf/y", "svc"),
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![mid1, mid2, leaf]));
    let rules = outcome.resolved_rules.clone().unwrap();
    // Each mid admits exactly one of the leaf's rules (the other is out of
    // its prefix and dropped with an issue).
    assert_eq!(rule_paths(&rules), vec!["/api/a/leaf/x", "/api/b/leaf/y"]);
    assert!(!dropped_messages(&outcome).iter().any(|m| m.contains("DelegationLoop")));
}

#[test]
fn depth_cap_enforced() {
    // Chain of 10 delegations: r0 → r1 → ... ; edges beyond depth 8 dropped.
    let mut routes = Vec::new();
    for i in 0..10 {
        routes.push(route(
            "infra",
            &format!("r{i}"),
            serde_json::json!({"rules": [delegating_rule("/api", "infra", &format!("r{}", i + 1))]}),
        ));
    }
    routes.push(route(
        "infra",
        "r10",
        serde_json::json!({"rules": [service_rule("/api/deep", "svc")]}),
    ));
    let root = routes[0].clone();
    let outcome = flatten_route(&root, &FakeLookup::new(routes));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert!(dropped_messages(&outcome)
        .iter()
        .any(|m| m.contains("DelegationDepthExceeded")));
}

#[test]
fn missing_named_child_reported_and_registered() {
    let parent = simple_parent("/api", "billing", "ghost");
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert_eq!(
        outcome.issues,
        vec![DelegationIssue::MissingChild {
            namespace: "billing".to_string(),
            name: "ghost".to_string()
        }]
    );
    // Still registered so the child's later creation requeues the parent
    assert!(outcome.named_children.contains("billing/ghost"));
}

// =====================================================================
// Malformed delegating rules
// =====================================================================

#[test]
fn malformed_delegating_rules_dropped() {
    // Mixing a delegation ref with a Service ref
    let mixed = parent_with_rules(serde_json::json!([{
        "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
        "backendRefs": [
            {"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "c", "namespace": "billing"},
            {"name": "svc", "port": 80}
        ]
    }]));
    let outcome = flatten_route(&mixed, &FakeLookup::new(vec![]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert!(dropped_messages(&outcome)[0].contains("exactly one backendRef"));

    // Port set on the delegation ref
    let with_port = parent_with_rules(serde_json::json!([{
        "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
        "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                          "name": "c", "namespace": "billing", "port": 80}]
    }]));
    let outcome2 = flatten_route(&with_port, &FakeLookup::new(vec![]));
    assert!(dropped_messages(&outcome2)[0].contains("'port'"));

    // Non-PathPrefix match on the delegating rule
    let exact_match = parent_with_rules(serde_json::json!([{
        "matches": [{"path": {"type": "Exact", "value": "/api"}}],
        "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                          "name": "c", "namespace": "billing"}]
    }]));
    let outcome3 = flatten_route(&exact_match, &FakeLookup::new(vec![]));
    assert!(dropped_messages(&outcome3)[0].contains("PathPrefix"));
}

// =====================================================================
// BackendRef namespace explicitation + ReferenceGrant
// =====================================================================

#[test]
fn child_backend_ns_made_explicit_and_grant_checked_from_child_ns() {
    let parent = simple_parent("/api", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            // Implicit ns → must become explicit "billing", no grant needed
            service_rule("/api/local", "svc-local"),
            // Cross-ns to shared: grant present for billing→shared
            {"matches": [{"path": {"type": "PathPrefix", "value": "/api/shared"}}],
             "backendRefs": [{"name": "svc-shared", "namespace": "shared", "port": 80}]},
            // Cross-ns to secret-ns: no grant → ref_denied
            {"matches": [{"path": {"type": "PathPrefix", "value": "/api/denied"}}],
             "backendRefs": [{"name": "svc-denied", "namespace": "secret-ns", "port": 80}]}
        ]}),
    );
    let lookup = FakeLookup::new(vec![child]).allow_cross_ns("billing", "shared");
    let outcome = flatten_route(&parent, &lookup);
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules.len(), 3);

    let br0 = &rules[0].backend_refs.as_ref().unwrap()[0];
    assert_eq!(
        br0.namespace.as_deref(),
        Some("billing"),
        "implicit ns must be explicit"
    );
    assert!(br0.ref_denied.is_none());

    let br1 = &rules[1].backend_refs.as_ref().unwrap()[0];
    assert!(br1.ref_denied.is_none(), "granted cross-ns ref must not be denied");

    let br2 = &rules[2].backend_refs.as_ref().unwrap()[0];
    let denied = br2.ref_denied.as_ref().expect("ungranted cross-ns ref must be denied");
    assert_eq!(denied.target_namespace, "secret-ns");

    // Service receipts retain the declaring child namespace
    assert!(outcome
        .backend_dependencies
        .iter()
        .any(|receipt| receipt.service_key() == Some("billing/svc-local")));
}

// =====================================================================
// Rule naming
// =====================================================================

#[test]
fn flattened_rule_names_carry_provenance() {
    let parent = simple_parent("/api", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            {"name": "user-named",
             "matches": [{"path": {"type": "PathPrefix", "value": "/api/a"}}],
             "backendRefs": [{"name": "svc", "port": 80}]},
            service_rule("/api/b", "svc")
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules[0].name.as_deref(), Some("user-named"));
    assert_eq!(rules[1].name.as_deref(), Some("billing.child.r1"));
}

#[test]
fn delegated_terminal_identity_uses_child_uid_and_original_rule_identity() {
    let parent = simple_parent("/api", "billing", "child");
    let mut child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            {"name": "user-named",
             "matches": [{"path": {"type": "PathPrefix", "value": "/api/a"}}],
             "backendRefs": [{"name": "svc", "port": 80}]},
            service_rule("/api/b", "svc")
        ]}),
    );
    child.metadata.uid = Some("child-uid".to_string());

    let rules = flatten_route(&parent, &FakeLookup::new(vec![child]))
        .resolved_rules
        .expect("delegation should produce resolved rules");
    assert_eq!(rules[0].resolved_terminal_route_uid.as_deref(), Some("child-uid"));
    assert_eq!(
        rules[0].resolved_terminal_rule_identity.as_deref(),
        Some("name:user-named")
    );
    assert_eq!(rules[1].resolved_terminal_route_uid.as_deref(), Some("child-uid"));
    assert_eq!(rules[1].resolved_terminal_rule_identity.as_deref(), Some("index:1"));
}

#[test]
fn delegated_terminal_identity_changes_after_uid_delete_create() {
    let parent = simple_parent("/api", "billing", "child");
    let child_spec = serde_json::json!({"rules": [service_rule("/api/a", "svc")]});
    let mut first = route("billing", "child", child_spec.clone());
    first.metadata.uid = Some("child-generation-1".to_string());
    let first_identity = flatten_route(&parent, &FakeLookup::new(vec![first]))
        .resolved_rules
        .expect("first child generation should flatten")[0]
        .resolved_terminal_route_uid
        .clone();

    let mut recreated = route("billing", "child", child_spec);
    recreated.metadata.uid = Some("child-generation-2".to_string());
    let recreated_identity = flatten_route(&parent, &FakeLookup::new(vec![recreated]))
        .resolved_rules
        .expect("recreated child generation should flatten")[0]
        .resolved_terminal_route_uid
        .clone();

    assert_eq!(first_identity.as_deref(), Some("child-generation-1"));
    assert_eq!(recreated_identity.as_deref(), Some("child-generation-2"));
    assert_ne!(first_identity, recreated_identity);
}

#[test]
fn ordinary_rules_get_uid_or_typed_key_and_deterministic_rule_identity() {
    let mut uid_route = route(
        "apps",
        "ordinary",
        serde_json::json!({"rules": [
            {"name": "primary", "backendRefs": [{"name": "svc", "port": 80}]},
            {"backendRefs": [{"name": "fallback", "port": 80}]}
        ]}),
    );
    uid_route.metadata.uid = Some("ordinary-uid".to_string());
    populate_terminal_rule_identities(&mut uid_route);
    let rules = uid_route.spec.rules.as_ref().expect("ordinary route should keep rules");
    assert_eq!(rules[0].resolved_terminal_route_uid.as_deref(), Some("ordinary-uid"));
    assert_eq!(
        rules[0].resolved_terminal_rule_identity.as_deref(),
        Some("name:primary")
    );
    assert_eq!(rules[1].resolved_terminal_rule_identity.as_deref(), Some("index:1"));

    let mut uidless = route(
        "apps",
        "file-route",
        serde_json::json!({"rules": [{"backendRefs": [{"name": "svc", "port": 80}]}]}),
    );
    populate_terminal_rule_identities(&mut uidless);
    let rule = &uidless.spec.rules.as_ref().expect("UID-less route should keep rules")[0];
    assert_eq!(
        rule.resolved_terminal_route_uid.as_deref(),
        Some("HTTPRoute/apps/file-route")
    );
    assert_eq!(rule.resolved_terminal_rule_identity.as_deref(), Some("index:0"));
}

struct NativeReadSpy {
    inner: FakeLookup,
    reads: std::cell::Cell<usize>,
    lists: std::cell::Cell<usize>,
    queries: std::cell::Cell<usize>,
}
impl NativeReadSpy {
    fn new(inner: FakeLookup) -> Self {
        Self {
            inner,
            reads: Default::default(),
            lists: Default::default(),
            queries: Default::default(),
        }
    }
}
impl DelegationLookup for NativeReadSpy {
    fn get_route(&self, key: &str) -> Option<Arc<HTTPRoute>> {
        self.reads.set(self.reads.get() + 1);
        self.inner.get_route(key)
    }
    fn list_route_keys_in_namespace(&self, ns: &str) -> Vec<String> {
        self.lists.set(self.lists.get() + 1);
        self.inner.list_route_keys_in_namespace(ns)
    }
    fn cross_ns_ref_allowed(
        &self,
        _from: &str,
        _group: Option<&str>,
        _kind: Option<&str>,
        _name: &str,
        _to: &str,
    ) -> bool {
        panic!("no mirror in native fixtures")
    }
    fn terminal_ref_denied(&self, terminal: TerminalBackendReference<'_>) -> Option<RefDenied> {
        self.queries.set(self.queries.get() + 1);
        let source = terminal.source();
        let key = format!("{}/{}", source.scope().namespace().unwrap(), terminal.source_name());
        let original = &self.inner.routes[&key];
        let actual = HttpRouteBackendRefs::from_route(original)
            .unwrap()
            .authored_at(terminal.slot().rule(), terminal.slot().backend())
            .unwrap()
            .target_view();
        assert!(
            std::ptr::eq(actual.name().as_ptr(), terminal.grant_query().target().name().as_ptr()),
            "authorize original child field before clone"
        );
        assert_eq!(terminal.source_uid(), original.metadata.uid.as_deref());
        assert_eq!(terminal.source_generation(), original.metadata.generation);
        self.inner.terminal_ref_denied(terminal)
    }
}

#[test]
fn native_invalid_sources_and_selectors_do_not_authorize_or_discover_targets() {
    let mut parent = simple_parent("/api", "billing", "child");
    let lookup = NativeReadSpy::new(FakeLookup::new(vec![]));
    parent.metadata.name = Some("bad/name".into());
    let outcome = flatten_route(&parent, &lookup);
    assert!(outcome.resolved_rules.unwrap().is_empty());
    assert_eq!(lookup.reads.get(), 0);
    assert_eq!(lookup.lists.get(), 0);
    assert_eq!(lookup.queries.get(), 0);
    assert!(outcome.backend_dependencies.is_empty());
    for (name, namespace, port) in [
        ("bad/name", "billing", None),
        ("child", "", None),
        ("child", "billing", Some(80)),
        ("*", "billing", Some(80)),
    ] {
        let mut parent = simple_parent("/api", namespace, name);
        parent.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].port = port;
        let outcome = flatten_route(&parent, &lookup);
        assert_eq!(lookup.reads.get(), 0);
        assert_eq!(lookup.lists.get(), 0);
        assert_eq!(lookup.queries.get(), 0);
        assert!(outcome.named_children.is_empty());
        assert!(outcome.wildcard_namespaces.is_empty());
        assert!(outcome.backend_dependencies.is_empty());
        assert!(!outcome.issues.is_empty());
    }
    // A valid selection remains a recovery edge even if its current child owner is invalid.
    for selector in ["child", "*"] {
        let parent = simple_parent("/api", "billing", selector);
        let mut child = route(
            "billing",
            "child",
            serde_json::json!({"rules":[service_rule("/api","service"),delegating_rule("/api","hidden","grandchild")]}),
        );
        child.metadata.name = Some("bad/name".into());
        let mut fake = FakeLookup::new(vec![]);
        fake.routes.insert("billing/child".into(), Arc::new(child));
        let lookup = NativeReadSpy::new(fake);
        let outcome = flatten_route(&parent, &lookup);
        assert_eq!(lookup.reads.get(), 1);
        assert_eq!(lookup.queries.get(), 0);
        assert!(outcome.resolved_rules.unwrap().is_empty());
        assert!(outcome.backend_dependencies.is_empty());
        assert!(outcome.expanded_children.is_empty());
        if selector == "*" {
            assert!(outcome.wildcard_namespaces.contains("billing"));
        } else {
            assert!(outcome.named_children.contains("billing/child"));
        }
        assert!(!outcome.named_children.contains("hidden/grandchild"));
    }
}

#[test]
fn native_child_to_root_authority_and_invalid_weight_slots_preserve_declaring_owner() {
    let parent = simple_parent("/api", "billing", "child");
    for allowed in [false, true] {
        let mut child = route(
            "billing",
            "child",
            serde_json::json!({"rules":[{"backendRefs":[
                {"name":"root-service","namespace":"infra","port":80,"weight":3},
                {"name":"root-service","namespace":"infra","port":0,"weight":7},
                {"name":"root-service","namespace":"infra","port":443,"weight":2}
            ]}]}),
        );
        child.metadata.uid = Some("original-child-uid".into());
        child.metadata.generation = Some(9);
        let mut fake = FakeLookup::new(vec![child]);
        if allowed {
            fake = fake.allow_cross_ns("billing", "infra");
        } else {
            fake = fake.allow_cross_ns("infra", "infra");
        }
        let lookup = NativeReadSpy::new(fake);
        let outcome = flatten_route(&parent, &lookup);
        assert_eq!(lookup.queries.get(), 2, "invalid port must not reach Grant callback");
        assert_eq!(
            outcome.backend_dependencies.len(),
            2,
            "same key, separate valid port/slot receipts"
        );
        assert!(outcome.cross_ns_target_namespaces.contains("infra"));
        let refs = outcome.resolved_rules.as_ref().unwrap()[0]
            .backend_refs
            .as_ref()
            .unwrap();
        assert_eq!(
            refs.iter().map(|reference| reference.weight).collect::<Vec<_>>(),
            vec![Some(3), Some(7), Some(2)]
        );
        assert_eq!(
            refs.iter().map(|reference| reference.port).collect::<Vec<_>>(),
            vec![Some(80), Some(0), Some(443)]
        );
        assert_eq!(refs[0].ref_denied.is_some(), !allowed);
        assert_eq!(refs[2].ref_denied.is_some(), !allowed);
        assert!(refs[1].ref_denied.is_none());
    }
}
