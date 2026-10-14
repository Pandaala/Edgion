use super::*;
use std::collections::HashMap;

// =====================================================================
// Test fixtures
// =====================================================================

struct FakeLookup {
    routes: HashMap<String, Arc<GRPCRoute>>,
    /// (from_ns, to_ns) pairs that are grant-allowed.
    allowed_cross_ns: HashSet<(String, String)>,
}

impl FakeLookup {
    fn new(routes: Vec<GRPCRoute>) -> Self {
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
    fn extension_ref_error(&self, _field: RouteExtensionField<'_>, _inspect: bool) -> Option<String> {
        None
    }

    fn get_route(&self, key: &str) -> Option<Arc<GRPCRoute>> {
        self.routes.get(key).cloned()
    }
    fn list_route_keys_in_namespace(&self, ns: &str) -> Vec<String> {
        let prefix = format!("{ns}/");
        let mut keys: Vec<String> = self.routes.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
        keys.sort_unstable();
        keys
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

fn route(ns: &str, name: &str, spec: serde_json::Value) -> GRPCRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": {"name": name, "namespace": ns},
        "spec": spec
    }))
    .expect("route fixture should deserialize")
}

fn delegating_rule(service: &str, ns: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "matches": [{"method": {"service": service}}],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": name, "namespace": ns
        }]
    })
}

fn parent_with_rules(rules: serde_json::Value) -> GRPCRoute {
    route("infra", "parent", serde_json::json!({"rules": rules}))
}

fn simple_parent(service: &str, child_ns: &str, child_name: &str) -> GRPCRoute {
    parent_with_rules(serde_json::json!([delegating_rule(service, child_ns, child_name)]))
}

fn service_rule(service: &str, svc: &str) -> serde_json::Value {
    serde_json::json!({
        "matches": [{"method": {"service": service}}],
        "backendRefs": [{"name": svc, "port": 80}]
    })
}

fn rule_services(rules: &[GRPCRouteRule]) -> Vec<&str> {
    rules
        .iter()
        .flat_map(|r| r.matches.iter().flatten())
        .filter_map(|m| m.method.as_ref().and_then(|mm| mm.service.as_deref()))
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
fn no_delegation_yields_none() {
    let parent = parent_with_rules(serde_json::json!([service_rule("svc.Plain", "svc")]));
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![]));
    assert!(outcome.resolved_rules.is_none());
    assert!(outcome.issues.is_empty());
}

#[test]
fn non_delegating_sibling_rules_preserved_in_order() {
    let parent = parent_with_rules(serde_json::json!([
        service_rule("svc.Plain", "svc-plain"),
        delegating_rule("svc.Billing", "billing", "child"),
    ]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [service_rule("svc.Billing", "svc-pay")]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.expect("delegation present");
    assert_eq!(rule_services(&rules), vec!["svc.Plain", "svc.Billing"]);
}

// =====================================================================
// Service containment (equal / not-equal / multi-scope)
// =====================================================================

#[test]
fn equal_service_is_contained() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [service_rule("svc.Billing", "svc-pay")]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rule_services(&rules), vec!["svc.Billing"]);
    assert!(outcome.issues.is_empty());
}

#[test]
fn not_equal_service_dropped_with_issue() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            service_rule("svc.Billing", "svc-pay"),
            service_rule("svc.Evil", "svc-evil"),
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rule_services(&rules), vec!["svc.Billing"]);
    let drops = dropped_messages(&outcome);
    assert_eq!(drops.len(), 1);
    assert!(drops[0].contains("rules[1]"), "drop message: {}", drops[0]);
    assert!(drops[0].contains("outside every delegated scope"));
}

#[test]
fn multi_scope_explicit_service_expands_against_every_equal_scope() {
    // A delegating rule with two matches delegates two services to the
    // same child; a terminal rule whose match explicitly equals ONE of
    // them only expands against that one scope.
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [
            {"method": {"service": "svc.A"}},
            {"method": {"service": "svc.B"}}
        ],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [service_rule("svc.A", "svc-a")]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rule_services(&rules), vec!["svc.A"]);
}

// =====================================================================
// Service inheritance distinguishes omission from invalid empty matchers
// =====================================================================

#[test]
fn inheritance_missing_method_matcher() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{"headers": [{"name": "x-req", "value": "1"}]}],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rule_services(&rules), vec!["svc.Billing"]);
    let m = &rules[0].matches.as_ref().unwrap()[0];
    assert_eq!(m.method.as_ref().unwrap().match_type, Some(GRPCMethodMatchType::Exact));
}

#[test]
fn empty_method_matcher_is_rejected_before_inheritance() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{"method": {}}],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert!(rules.is_empty());
    assert!(outcome.issues.iter().any(|issue| matches!(issue,
        DelegationIssue::RuleDropped { message } if message.contains("invalid rule")
    )));
}

#[test]
fn invalid_child_method_drops_whole_rule_but_preserves_valid_sibling() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            {"matches": [{"method": {"method": "Get"}}, {"method": {"service": "", "method": "Get"}}],
             "backendRefs": [{"name": "invalid", "port": 80}]},
            {"matches": [{"method": {"type": "RegularExpression", "method": "private-marker["}}],
             "backendRefs": [{"name": "invalid-regex", "port": 80}]},
            {"matches": [{"method": {"method": "List"}}],
             "backendRefs": [{"name": "valid", "port": 80}]}
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].backend_refs.as_ref().unwrap()[0].name, "valid");
    let matcher = rules[0].matches.as_ref().unwrap()[0].method.as_ref().unwrap();
    assert_eq!(matcher.service.as_deref(), Some("svc.Billing"));
    assert_eq!(matcher.method.as_deref(), Some("List"));
    assert_eq!(outcome.issues.len(), 2);
    assert!(!format!("{:?}", outcome.issues).contains("private-marker"));
}

#[test]
fn resolved_publication_excludes_invalid_parent_and_child_method_rules() {
    let mut parent = simple_parent("svc.Billing", "billing", "child");
    let siblings: Vec<GRPCRouteRule> = serde_json::from_value(serde_json::json!([
        {"matches": [{"method": {"type": "RegularExpression", "method": "["}}],
         "backendRefs": [{"name": "invalid-parent", "port": 80}]},
        {"matches": [{"method": {"method": "Fetch"}}],
         "backendRefs": [{"name": "valid-parent", "port": 80}]}
    ]))
    .unwrap();
    parent.spec.rules.as_mut().unwrap().extend(siblings);
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            {"matches": [{"method": {}}], "backendRefs": [{"name": "invalid-child", "port": 80}]},
            {"matches": [{"method": {"method": "Get"}}], "backendRefs": [{"name": "valid-child", "port": 80}]}
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    assert_eq!(outcome.issues.len(), 2);
    parent.spec.resolved_rules = outcome.resolved_rules;
    parent.spec.delegation_issues = outcome.issues;
    let published: GRPCRoute = serde_json::from_slice(&serde_json::to_vec(&parent).unwrap()).unwrap();
    let effective = published.effective_rules().unwrap();
    assert_eq!(effective.len(), 2);
    assert!(effective.iter().all(|rule| rule.method_match_error().is_none()));
    assert_eq!(effective[0].backend_refs.as_ref().unwrap()[0].name, "valid-child");
    assert_eq!(effective[1].backend_refs.as_ref().unwrap()[0].name, "valid-parent");
    let inherited = effective[0].matches.as_ref().unwrap()[0].method.as_ref().unwrap();
    assert_eq!(inherited.service.as_deref(), Some("svc.Billing"));
    assert_eq!(inherited.method.as_deref(), Some("Get"));
    assert!(published.spec.delegation_issues.is_empty());
}

#[test]
fn inheritance_method_only_matcher_keeps_method_name() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{"method": {"method": "Get"}}],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    let m = &rules[0].matches.as_ref().unwrap()[0];
    assert_eq!(m.method.as_ref().unwrap().service.as_deref(), Some("svc.Billing"));
    assert_eq!(m.method.as_ref().unwrap().method.as_deref(), Some("Get"));
}

#[test]
fn inheritance_expands_once_per_scope_with_multiple_delegated_services() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [
            {"method": {"service": "svc.A"}},
            {"method": {"service": "svc.B"}}
        ],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{"method": {"method": "List"}}],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules.len(), 1);
    let mut services = rule_services(&rules);
    services.sort_unstable();
    assert_eq!(services, vec!["svc.A", "svc.B"]);
}

// =====================================================================
// Matchless child rule regression (divergence #2)
// =====================================================================

#[test]
fn matchless_child_rule_flattens_to_one_inherited_match_per_scope_never_cloned_matchless() {
    // Two delegated services; a matchless terminal child rule MUST NOT be
    // cloned as-is (that would compile to a catch-all matching ALL
    // traffic) — it must expand to exactly one synthesized Exact match
    // per delegated service.
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [
            {"method": {"service": "svc.A"}},
            {"method": {"service": "svc.B"}}
        ],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{"backendRefs": [{"name": "svc", "port": 80}]}]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules.len(), 1, "matchless rule must not be cloned verbatim");
    let matches = rules[0].matches.as_ref().expect("synthesized matches");
    assert_eq!(matches.len(), 2, "one synthesized match per delegated scope");
    let mut services: Vec<&str> = matches
        .iter()
        .filter_map(|m| m.method.as_ref().and_then(|mm| mm.service.as_deref()))
        .collect();
    services.sort_unstable();
    assert_eq!(services, vec!["svc.A", "svc.B"]);
    for m in matches {
        assert_eq!(m.method.as_ref().unwrap().match_type, Some(GRPCMethodMatchType::Exact));
        assert!(m.method.as_ref().unwrap().method.is_none());
    }
}

#[test]
fn matchless_child_rule_single_scope_still_synthesizes_not_clones() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{"backendRefs": [{"name": "svc", "port": 80}]}]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules.len(), 1);
    let matches = rules[0].matches.as_ref().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0].method.as_ref().unwrap().service.as_deref(),
        Some("svc.Billing")
    );
}

// =====================================================================
// Regex rejection (divergence #4)
// =====================================================================

#[test]
fn regex_child_match_invalidates_whole_rule() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            {
                "matches": [{"method": {"type": "RegularExpression", "service": "svc\\..*"}}],
                "backendRefs": [{"name": "svc", "port": 80}]
            },
            service_rule("svc.Billing", "svc-ok")
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rule_services(&rules), vec!["svc.Billing"]);
    let drops = dropped_messages(&outcome);
    assert_eq!(drops.len(), 1);
    assert!(drops[0].contains("rules[0]"));
    assert!(drops[0].contains("RegularExpression"));
}

// =====================================================================
// Delegating-rule shape violations (divergence #1 + shape matrix)
// =====================================================================

#[test]
fn delegating_rule_matchless_is_invalid() {
    // matches absent
    let absent = parent_with_rules(serde_json::json!([{
        "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                          "name": "c", "namespace": "billing"}]
    }]));
    let outcome = flatten_route(&absent, &FakeLookup::new(vec![]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert!(dropped_messages(&outcome)[0].contains("delegate-all is not supported"));

    // matches present but empty
    let empty = parent_with_rules(serde_json::json!([{
        "matches": [],
        "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                          "name": "c", "namespace": "billing"}]
    }]));
    let outcome2 = flatten_route(&empty, &FakeLookup::new(vec![]));
    assert!(outcome2.resolved_rules.clone().unwrap().is_empty());
    assert!(dropped_messages(&outcome2)[0].contains("delegate-all is not supported"));
}

#[test]
fn delegating_rule_shape_violation_matrix() {
    // method.method set
    let with_method = parent_with_rules(serde_json::json!([{
        "matches": [{"method": {"service": "svc.Billing", "method": "Pay"}}],
        "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                          "name": "c", "namespace": "billing"}]
    }]));
    let outcome = flatten_route(&with_method, &FakeLookup::new(vec![]));
    assert!(dropped_messages(&outcome)[0].contains("method.method"));

    // regex type
    let regex = parent_with_rules(serde_json::json!([{
        "matches": [{"method": {"type": "RegularExpression", "service": "svc.Billing"}}],
        "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                          "name": "c", "namespace": "billing"}]
    }]));
    let outcome2 = flatten_route(&regex, &FakeLookup::new(vec![]));
    assert!(dropped_messages(&outcome2)[0].contains("Exact"));

    // header-only match (no method matcher at all)
    let header_only = parent_with_rules(serde_json::json!([{
        "matches": [{"headers": [{"name": "x-t", "value": "1"}]}],
        "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                          "name": "c", "namespace": "billing"}]
    }]));
    let outcome3 = flatten_route(&header_only, &FakeLookup::new(vec![]));
    assert!(dropped_messages(&outcome3)[0].contains("header-only"));

    // empty-string service
    let empty_service = parent_with_rules(serde_json::json!([{
        "matches": [{"method": {"service": ""}}],
        "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                          "name": "c", "namespace": "billing"}]
    }]));
    let outcome4 = flatten_route(&empty_service, &FakeLookup::new(vec![]));
    assert!(dropped_messages(&outcome4)[0].contains("requires a non-empty service or method"));

    // port set
    let with_port = parent_with_rules(serde_json::json!([{
        "matches": [{"method": {"service": "svc.Billing"}}],
        "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                          "name": "c", "namespace": "billing", "port": 80}]
    }]));
    let outcome5 = flatten_route(&with_port, &FakeLookup::new(vec![]));
    assert!(dropped_messages(&outcome5)[0].contains("'port'"));

    // backendRef filters set
    let with_filters = parent_with_rules(serde_json::json!([{
        "matches": [{"method": {"service": "svc.Billing"}}],
        "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                          "name": "c", "namespace": "billing",
                          "filters": [{"type": "RequestHeaderModifier",
                                       "requestHeaderModifier": {"set": [{"name": "x", "value": "1"}]}}]}]
    }]));
    let outcome6 = flatten_route(&with_filters, &FakeLookup::new(vec![]));
    assert!(dropped_messages(&outcome6)[0].contains("per-backend filters"));

    // multiple refs (mixed delegation + Service)
    let mixed = parent_with_rules(serde_json::json!([{
        "matches": [{"method": {"service": "svc.Billing"}}],
        "backendRefs": [
            {"group": "gateway.networking.k8s.io", "kind": "GRPCRoute", "name": "c", "namespace": "billing"},
            {"name": "svc", "port": 80}
        ]
    }]));
    let outcome7 = flatten_route(&mixed, &FakeLookup::new(vec![]));
    assert!(dropped_messages(&outcome7)[0].contains("exactly one backendRef"));
}

// =====================================================================
// Header constraint merge
// =====================================================================

#[test]
fn scope_headers_merged_into_child_matches() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{
            "method": {"service": "svc.Billing"},
            "headers": [{"name": "x-tenant", "value": "b"}]
        }],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{
                "method": {"service": "svc.Billing"},
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
}

#[test]
fn conflicting_header_case_insensitive_drops_child_rule() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{
            "method": {"service": "svc.Billing"},
            "headers": [{"name": "x-tenant", "value": "b"}]
        }],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{
                "method": {"service": "svc.Billing"},
                "headers": [{"name": "X-Tenant", "value": "OTHER"}]
            }],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    let drops = dropped_messages(&outcome);
    assert_eq!(drops.len(), 1);
    assert!(drops[0].contains("x-tenant"));
}

#[test]
fn identical_header_entry_is_deduped_not_conflicted() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{
            "method": {"service": "svc.Billing"},
            "headers": [{"name": "x-tenant", "value": "b"}]
        }],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{
                "method": {"service": "svc.Billing"},
                "headers": [{"name": "X-Tenant", "value": "b"}]
            }],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules.len(), 1, "identical header must be deduped, not conflicted");
    let headers = rules[0].matches.as_ref().unwrap()[0].headers.as_ref().unwrap();
    assert_eq!(headers.len(), 1);
}

#[test]
fn duplicate_headers_are_canonical_before_delegation_merge() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{
            "method": {"service": "svc.Billing"},
            "headers": [
                {"name": "X-Tenant", "value": "b"},
                {"name": "x-tenant", "value": "ignored"}
            ]
        }],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{
                "method": {"service": "svc.Billing"},
                "headers": [{"name": "x-tenant", "value": "b"}]
            }],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );

    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.as_ref().unwrap();
    let headers = rules[0].matches.as_ref().unwrap()[0].headers.as_ref().unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!((headers[0].name.as_str(), headers[0].value.as_str()), ("x-tenant", "b"));
}

// =====================================================================
// Filter inheritance
// =====================================================================

#[test]
fn malformed_extension_ref_in_child_drops_only_that_grpc_rule() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{"method": {"service": "svc.Billing"}}],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            {
                "matches": [{"method": {"service": "svc.Billing", "method": "Bad"}}],
                "filters": [{"type": "ExtensionRef"}],
                "backendRefs": [{"name": "svc", "port": 80}]
            },
            {
                "matches": [{"method": {"service": "svc.Billing", "method": "Good"}}],
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
fn parent_filters_prepended_and_extension_refs_concatenate() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{"method": {"service": "svc.Billing"}}],
        "filters": [
            {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-p", "value": "1"}]}},
            {"type": "ExtensionRef", "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "parent-plugin"}}
        ],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{"method": {"service": "svc.Billing"}}],
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
    assert_eq!(filters.len(), 3);
    assert_eq!(filters[0].filter_type, GRPCRouteFilterType::RequestHeaderModifier);
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
fn child_standard_filter_conflicting_with_inherited_type_drops_rule() {
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{"method": {"service": "svc.Billing"}}],
        "filters": [
            {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-p", "value": "1"}]}}
        ],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{"method": {"service": "svc.Billing"}}],
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
        "matches": [{"method": {"service": "svc.Billing"}}],
        "timeouts": {"request": "30s"},
        "retry": {"attempts": 5},
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "mid", "namespace": "billing"
        }]
    }]));
    let mid = route(
        "billing",
        "mid",
        serde_json::json!({"rules": [{
            "matches": [{"method": {"service": "svc.Billing"}}],
            "retry": {"attempts": 2},
            "backendRefs": [{
                "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                "name": "leaf", "namespace": "billing"
            }]
        }]}),
    );
    let leaf = route(
        "billing",
        "leaf",
        serde_json::json!({"rules": [service_rule("svc.Billing", "svc")]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![mid, leaf]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules[0].timeouts.as_ref().and_then(|t| t.request.as_deref()),
        Some("30s")
    );
    assert_eq!(rules[0].retry.as_ref().and_then(|r| r.attempts), Some(2));
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
            "matches": [{"method": {"service": "svc.Any"}}],
            "backendRefs": [{
                "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                "name": "*", "namespace": "infra"
            }]
        }]}),
    );
    let a = route(
        "infra",
        "a-child",
        serde_json::json!({"rules": [service_rule("svc.Any", "svc-a")]}),
    );
    let b = route(
        "infra",
        "b-child",
        serde_json::json!({"rules": [service_rule("svc.Any", "svc-b")]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![parent.clone(), a, b]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules.len(), 2);
    let backends: Vec<&str> = rules
        .iter()
        .flat_map(|r| r.backend_refs.iter().flatten())
        .map(|br| br.name.as_str())
        .collect();
    // a-child before b-child (sorted key order)
    assert_eq!(backends, vec!["svc-a", "svc-b"]);
    assert!(outcome.wildcard_namespaces.contains("infra"));
    assert!(outcome.issues.is_empty(), "issues: {:?}", outcome.issues);
}

#[test]
fn wildcard_zero_children_is_silent() {
    let mut parent = simple_parent("svc.Billing", "empty-ns", "placeholder");
    parent.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].name = "*".to_string();
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert!(outcome.issues.is_empty());
}

#[test]
fn reverse_lock_denies_unlisted_parent_and_allows_listed() {
    let parent = simple_parent("svc.Billing", "billing", "locked");
    let locked_to_other = route(
        "billing",
        "locked",
        serde_json::json!({
            "parentRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                             "name": "someone-else", "namespace": "infra"}],
            "rules": [service_rule("svc.Billing", "svc")]
        }),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![locked_to_other]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert!(dropped_messages(&outcome)[0].contains("parentRefs restrict delegation"));

    let locked_to_us = route(
        "billing",
        "locked",
        serde_json::json!({
            "parentRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                             "name": "parent", "namespace": "infra"}],
            "rules": [service_rule("svc.Billing", "svc")]
        }),
    );
    let parent2 = simple_parent("svc.Billing", "billing", "locked");
    let outcome2 = flatten_route(&parent2, &FakeLookup::new(vec![locked_to_us]));
    assert_eq!(
        rule_services(&outcome2.resolved_rules.clone().unwrap()),
        vec!["svc.Billing"]
    );
}

#[test]
fn child_with_hostnames_is_skipped() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({
            "hostnames": ["evil.example.com"],
            "rules": [service_rule("svc.Billing", "svc")]
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
    let self_parent = simple_parent("svc.Billing", "infra", "parent");
    let outcome = flatten_route(&self_parent, &FakeLookup::new(vec![self_parent.clone()]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert!(dropped_messages(&outcome)[0].contains("DelegationLoop"));

    let a = route(
        "infra",
        "a",
        serde_json::json!({"rules": [delegating_rule("svc.Billing", "infra", "b")]}),
    );
    let b = route(
        "infra",
        "b",
        serde_json::json!({"rules": [delegating_rule("svc.Billing", "infra", "a")]}),
    );
    let outcome2 = flatten_route(&a, &FakeLookup::new(vec![a.clone(), b]));
    assert!(outcome2.resolved_rules.clone().unwrap().is_empty());
    assert!(dropped_messages(&outcome2).iter().any(|m| m.contains("DelegationLoop")));
}

#[test]
fn diamond_is_legal() {
    let parent = parent_with_rules(serde_json::json!([
        delegating_rule("svc.A", "infra", "mid1"),
        delegating_rule("svc.B", "infra", "mid2"),
    ]));
    let mid1 = route(
        "infra",
        "mid1",
        serde_json::json!({"rules": [delegating_rule("svc.A", "infra", "leaf")]}),
    );
    let mid2 = route(
        "infra",
        "mid2",
        serde_json::json!({"rules": [delegating_rule("svc.B", "infra", "leaf")]}),
    );
    let leaf = route(
        "infra",
        "leaf",
        serde_json::json!({"rules": [
            service_rule("svc.A", "svc-a"),
            service_rule("svc.B", "svc-b"),
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![mid1, mid2, leaf]));
    let rules = outcome.resolved_rules.clone().unwrap();
    let mut services = rule_services(&rules);
    services.sort_unstable();
    assert_eq!(services, vec!["svc.A", "svc.B"]);
    assert!(!dropped_messages(&outcome).iter().any(|m| m.contains("DelegationLoop")));
}

#[test]
fn depth_cap_enforced() {
    let mut routes = Vec::new();
    for i in 0..10 {
        routes.push(route(
            "infra",
            &format!("r{i}"),
            serde_json::json!({"rules": [delegating_rule("svc.Deep", "infra", &format!("r{}", i + 1))]}),
        ));
    }
    routes.push(route(
        "infra",
        "r10",
        serde_json::json!({"rules": [service_rule("svc.Deep", "svc")]}),
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
    let parent = simple_parent("svc.Billing", "billing", "ghost");
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![]));
    assert!(outcome.resolved_rules.clone().unwrap().is_empty());
    assert_eq!(
        outcome.issues,
        vec![DelegationIssue::MissingChild {
            namespace: "billing".to_string(),
            name: "ghost".to_string()
        }]
    );
    assert!(outcome.named_children.contains("billing/ghost"));
}

// =====================================================================
// BackendRef namespace explicitation + ReferenceGrant
// =====================================================================

#[test]
fn child_backend_ns_made_explicit_and_grant_checked_from_child_ns() {
    // gRPC delegation is service-level only, so every child rule must
    // inherit (or exactly match) the delegated service "svc.Billing";
    // rules are distinguished by method name instead of by sub-service.
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            {"matches": [{"method": {"method": "Local"}}],
             "backendRefs": [{"name": "svc-local", "port": 80}]},
            {"matches": [{"method": {"method": "Shared"}}],
             "backendRefs": [{"name": "svc-shared", "namespace": "shared", "port": 80}]},
            {"matches": [{"method": {"method": "Denied"}}],
             "backendRefs": [{"name": "svc-denied", "namespace": "secret-ns", "port": 80}]}
        ]}),
    );
    let lookup = FakeLookup::new(vec![child]).allow_cross_ns("billing", "shared");
    let outcome = flatten_route(&parent, &lookup);
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules.len(), 3);

    fn backend_ref_for<'a>(rules: &'a [GRPCRouteRule], method: &str) -> &'a GRPCBackendRef {
        &rules
            .iter()
            .find(|r| {
                r.matches.as_ref().unwrap()[0]
                    .method
                    .as_ref()
                    .and_then(|m| m.method.as_deref())
                    == Some(method)
            })
            .unwrap()
            .backend_refs
            .as_ref()
            .unwrap()[0]
    }

    let br0 = backend_ref_for(&rules, "Local");
    assert_eq!(
        br0.namespace.as_deref(),
        Some("billing"),
        "implicit ns must be explicit"
    );
    assert!(br0.ref_denied.is_none());

    let br1 = backend_ref_for(&rules, "Shared");
    assert!(br1.ref_denied.is_none(), "granted cross-ns ref must not be denied");

    let br2 = backend_ref_for(&rules, "Denied");
    let denied = br2.ref_denied.as_ref().expect("ungranted cross-ns ref must be denied");
    assert_eq!(denied.target_namespace, "secret-ns");

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
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            {"name": "user-named",
             "matches": [{"method": {"service": "svc.Billing", "method": "A"}}],
             "backendRefs": [{"name": "svc", "port": 80}]},
            service_rule("svc.Billing", "svc")
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules[0].name.as_deref(), Some("user-named"));
    assert_eq!(rules[1].name.as_deref(), Some("billing.child.r1"));
}

// =====================================================================
// Invalid child rules (standing per-rule checks)
// =====================================================================

#[test]
fn malformed_parent_header_modifier_drops_only_its_delegating_rule() {
    let mut bad = delegating_rule("svc.Billing", "billing", "child");
    bad["filters"] = serde_json::json!([{"type": "RequestHeaderModifier"}]);
    let parent = parent_with_rules(serde_json::json!([bad, service_rule("svc.Health", "healthy")]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [service_rule("svc.Billing", "child-svc")]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].backend_refs.as_ref().unwrap()[0].name, "healthy");
    assert!(outcome.expanded_children.is_empty());
}

#[test]
fn malformed_child_header_modifiers_drop_only_offending_rules() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    let mut bad_rule = service_rule("svc.Billing", "bad-rule");
    bad_rule["filters"] = serde_json::json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {
        "set": [{"name": "x-test", "value": "bad\r\nvalue"}]
    }}]);
    let mut bad_backend = service_rule("svc.Billing", "bad-backend");
    bad_backend["backendRefs"][0]["filters"] = serde_json::json!([{"type": "ResponseHeaderModifier"}]);
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            bad_rule, bad_backend, service_rule("svc.Billing", "healthy")
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
fn invalid_child_rule_dropped_with_issue_valid_sibling_flattens() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    // rules[0] is invalid per find_invalid_rule_indices (duplicate
    // single-instance filter type within one filter list); rules[1] is
    // valid and must still flatten.
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            {
                "matches": [{"method": {"service": "svc.Billing"}}],
                "filters": [
                    {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "a", "value": "1"}]}},
                    {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "b", "value": "2"}]}}
                ],
                "backendRefs": [{"name": "svc-bad", "port": 80}]
            },
            service_rule("svc.Billing", "svc-ok")
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    let rules = outcome.resolved_rules.clone().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].backend_refs.as_ref().unwrap()[0].name, "svc-ok");
    let drops = dropped_messages(&outcome);
    assert_eq!(drops.len(), 1);
    assert_eq!(
        drops[0],
        "child 'billing/child' rules[0] dropped: invalid rule (see child status)"
    );
}

// =====================================================================
// cross_ns_target_namespaces + expanded_children
// =====================================================================

#[test]
fn cross_ns_target_namespaces_recorded_for_flattened_backend_refs() {
    let parent = simple_parent("svc.Billing", "billing", "child");
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [
            // Same-ns (implicit) ref: must NOT be recorded
            service_rule("svc.Billing", "svc-local"),
            // Cross-ns ref: target ns recorded regardless of grant outcome
            {"matches": [{"method": {"method": "Shared"}}],
             "backendRefs": [{"name": "svc-shared", "namespace": "shared", "port": 80}]}
        ]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![child]));
    assert!(outcome.cross_ns_target_namespaces.contains("shared"));
    assert!(!outcome.cross_ns_target_namespaces.contains("billing"));
    assert_eq!(outcome.cross_ns_target_namespaces.len(), 1);
}

#[test]
fn expanded_children_tracks_actually_expanded_keys_not_missing_ones() {
    let parent = parent_with_rules(serde_json::json!([
        delegating_rule("svc.Billing", "billing", "present"),
        delegating_rule("svc.Billing", "billing", "ghost"),
    ]));
    let present = route(
        "billing",
        "present",
        serde_json::json!({"rules": [service_rule("svc.Billing", "svc")]}),
    );
    let outcome = flatten_route(&parent, &FakeLookup::new(vec![present]));
    // The existing child was expanded; the missing one was not, but both
    // are registered as named children (so the ghost's later creation
    // requeues this parent).
    assert!(outcome.expanded_children.contains("billing/present"));
    assert!(!outcome.expanded_children.contains("billing/ghost"));
    assert!(outcome.named_children.contains("billing/present"));
    assert!(outcome.named_children.contains("billing/ghost"));
}

// =====================================================================
// br.filters smuggle check
// =====================================================================

#[test]
fn child_backend_ref_filter_cannot_smuggle_mandatory_override() {
    // A child must not evade the mandatory-filter conflict check by
    // declaring the same-type standard filter at backendRef level.
    let parent = parent_with_rules(serde_json::json!([{
        "matches": [{"method": {"service": "svc.Billing"}}],
        "filters": [
            {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-p", "value": "1"}]}}
        ],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "name": "child", "namespace": "billing"
        }]
    }]));
    let child = route(
        "billing",
        "child",
        serde_json::json!({"rules": [{
            "matches": [{"method": {"service": "svc.Billing"}}],
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
    fn extension_ref_error(&self, _field: RouteExtensionField<'_>, _inspect: bool) -> Option<String> {
        None
    }

    fn get_route(&self, key: &str) -> Option<Arc<GRPCRoute>> {
        self.reads.set(self.reads.get() + 1);
        self.inner.get_route(key)
    }
    fn list_route_keys_in_namespace(&self, ns: &str) -> Vec<String> {
        self.lists.set(self.lists.get() + 1);
        self.inner.list_route_keys_in_namespace(ns)
    }
    fn terminal_ref_denied(&self, terminal: TerminalBackendReference<'_>) -> Option<RefDenied> {
        self.queries.set(self.queries.get() + 1);
        let source = terminal.source();
        let key = format!("{}/{}", source.scope().namespace().unwrap(), terminal.source_name());
        let original = &self.inner.routes[&key];
        let actual = GrpcRouteBackendRefs::from_route(original)
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
    let mut parent = simple_parent("svc.Billing", "billing", "child");
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
        let mut parent = simple_parent("svc.Billing", namespace, name);
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
        let parent = simple_parent("svc.Billing", "billing", selector);
        let mut child = route(
            "billing",
            "child",
            serde_json::json!({"rules":[service_rule("svc.Billing","service"),delegating_rule("svc.Billing","hidden","grandchild")]}),
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
    let parent = simple_parent("svc.Billing", "billing", "child");
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

struct ExtensionReadSpy {
    inner: FakeLookup,
    registered: std::cell::RefCell<HashSet<(usize, Option<usize>, usize)>>,
    inspected: std::cell::Cell<usize>,
}
impl DelegationLookup for ExtensionReadSpy {
    fn get_route(&self, key: &str) -> Option<Arc<GRPCRoute>> {
        self.inner.get_route(key)
    }
    fn list_route_keys_in_namespace(&self, ns: &str) -> Vec<String> {
        self.inner.list_route_keys_in_namespace(ns)
    }
    fn terminal_ref_denied(&self, terminal: TerminalBackendReference<'_>) -> Option<RefDenied> {
        self.inner.terminal_ref_denied(terminal)
    }
    fn extension_ref_error(&self, field: RouteExtensionField<'_>, inspect: bool) -> Option<String> {
        let reference = field.validated().ok().flatten()?;
        let source = reference.source();
        assert_eq!(source.name(), "child");
        assert_eq!(source.scope().namespace(), Some("children"));
        assert_eq!(reference.source_uid(), Some("child-uid"));
        assert_eq!(reference.source_generation(), Some(8));
        assert_eq!(reference.target().scope().namespace(), Some("children"));
        let slot = reference.slot();
        let actual = GrpcRouteExtensionRefs::from_route(&self.inner.routes["children/child"])
            .unwrap()
            .authored_at(slot.rule(), slot.backend(), slot.filter())
            .unwrap()
            .validated()
            .unwrap()
            .unwrap();
        assert!(
            std::ptr::eq(actual.target().name().as_ptr(), reference.target().name().as_ptr()),
            "lookup borrows original child before clone"
        );
        if inspect {
            assert_eq!(
                self.registered.borrow().len(),
                3,
                "all valid child edges, including structurally dropped rules, precede inspection"
            );
            assert_eq!(slot.rule(), 0, "invalid rule material is never inspected");
            self.inspected.set(self.inspected.get() + 1);
            if slot.backend().is_some() {
                return Some("backend scope rejected".into());
            }
        } else {
            self.registered
                .borrow_mut()
                .insert((slot.rule(), slot.backend(), slot.filter()));
        }
        None
    }
}

#[test]
fn child_extension_loans_register_before_scope_drops_and_preserve_source_identity() {
    let extension = serde_json::json!({"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"bundle","resolvedNamespace":"forged"}});
    let mut valid = service_rule("demo.Service", "service");
    valid["filters"] = serde_json::json!([extension.clone()]);
    valid["backendRefs"][0]["filters"] = serde_json::json!([extension.clone()]);
    let mut invalid = service_rule("demo.Service", "service");
    invalid["filters"] = serde_json::json!([{"type":"ExtensionRef"},extension]);
    let mut child = route("children", "child", serde_json::json!({"rules":[valid,invalid]}));
    child.metadata.uid = Some("child-uid".into());
    child.metadata.generation = Some(8);
    let spy = ExtensionReadSpy {
        inner: FakeLookup::new(vec![child.clone()]),
        registered: Default::default(),
        inspected: Default::default(),
    };
    let parent = simple_parent("demo.Service", "children", "child");
    let outcome = flatten_route(&parent, &spy);
    assert!(outcome.resolved_rules.as_ref().unwrap().is_empty());
    assert_eq!(outcome.extension_dependencies.len(), 3);
    assert_eq!(spy.registered.borrow().len(), 3);
    assert_eq!(spy.inspected.get(), 2);
    assert!(dropped_messages(&outcome)
        .iter()
        .any(|message| message.contains("backend scope rejected")));
    assert!(outcome.named_children.contains("children/child"));

    // Valid declarations retain recovery edges even if route-level child gates drop them.
    child.spec.hostnames = Some(vec!["example.test".into()]);
    let spy = ExtensionReadSpy {
        inner: FakeLookup::new(vec![child.clone()]),
        registered: Default::default(),
        inspected: Default::default(),
    };
    let outcome = flatten_route(&parent, &spy);
    assert!(outcome.resolved_rules.unwrap().is_empty());
    assert_eq!(spy.registered.borrow().len(), 3);
    assert_eq!(spy.inspected.get(), 0);
    child.metadata.namespace = Some("bad/ns".into());
    let mut inner = FakeLookup::new(vec![]);
    inner.routes.insert("children/child".into(), Arc::new(child));
    let spy = ExtensionReadSpy {
        inner,
        registered: Default::default(),
        inspected: Default::default(),
    };
    assert!(flatten_route(&parent, &spy).resolved_rules.unwrap().is_empty());
    assert!(spy.registered.borrow().is_empty());
    assert_eq!(spy.inspected.get(), 0);
    let mut invalid_parent = parent;
    invalid_parent.metadata.name = Some("bad/name".into());
    assert!(flatten_route(&invalid_parent, &spy).resolved_rules.unwrap().is_empty());
    assert!(spy.registered.borrow().is_empty());
}

#[test]
fn session_lifetime_child_first_else_inherited_across_two_levels() {
    for (mid_policy, child_policy, expected) in [
        (None, None, Some("30s")),
        (Some(serde_json::json!({"absoluteTimeout":"5s"})), None, Some("5s")),
        (
            Some(serde_json::json!({"absoluteTimeout":"5s"})),
            Some(serde_json::json!({})),
            None,
        ),
        (None, Some(serde_json::json!({"absoluteTimeout":"1ms"})), Some("1ms")),
    ] {
        let parent = parent_with_rules(serde_json::json!([{
            "matches": [{"method": {"service": "svc.Billing"}}],
            "sessionPersistence": {"absoluteTimeout": "30s"},
            "timeouts": {"request": "30s"},
            "retry": {"attempts": 5},
            "backendRefs": [{
                "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                "name": "mid", "namespace": "billing"
            }]
        }]));
        let mut mid = route(
            "billing",
            "mid",
            serde_json::json!({"rules": [{
                "matches": [{"method": {"service": "svc.Billing"}}],
                "retry": {"attempts": 2},
                "backendRefs": [{
                    "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                    "name": "leaf", "namespace": "billing"
                }]
            }]}),
        );
        let mut leaf = route(
            "billing",
            "leaf",
            serde_json::json!({"rules": [service_rule("svc.Billing", "svc")]}),
        );
        mid.spec.rules.as_mut().unwrap()[0].session_persistence =
            mid_policy.map(|p| serde_json::from_value(p).unwrap());
        leaf.spec.rules.as_mut().unwrap()[0].session_persistence =
            child_policy.map(|p| serde_json::from_value(p).unwrap());
        let outcome = flatten_route(&parent, &FakeLookup::new(vec![mid, leaf]));
        let rules = outcome.resolved_rules.clone().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0]
                .session_persistence
                .as_ref()
                .unwrap()
                .absolute_timeout
                .as_deref(),
            expected
        );
        assert_eq!(
            rules[0].timeouts.as_ref().and_then(|t| t.request.as_deref()),
            Some("30s")
        );
        assert_eq!(rules[0].retry.as_ref().and_then(|r| r.attempts), Some(2));
        assert!(outcome.named_children.contains("billing/mid"));
        assert!(outcome.named_children.contains("billing/leaf"));
    }
}
