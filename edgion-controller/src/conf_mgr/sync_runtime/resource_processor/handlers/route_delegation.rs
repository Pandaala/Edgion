//! HTTPRoute delegation flattening.
//!
//! A parent HTTPRoute rule may delegate its path prefix to child HTTPRoutes
//! (possibly in other namespaces) via `backendRef {group:
//! gateway.networking.k8s.io, kind: HTTPRoute}`. During `parse`, the
//! controller expands the delegation tree into a flat rule list stored in
//! `spec.resolved_rules`; the gateway compiles that list and never sees the
//! nesting.
//!
//! Canonical semantics: `docs/en/user-guide/http-route/delegation.md`; controller
//! architecture: `skills/01-architecture/05-resources/04-http-route.md`.
//! Key locked decisions: child paths are absolute and must be contained in the
//! delegated prefix; parent filters are mandatory and non-overridable (a child
//! standard filter of an inherited type invalidates the child rule; ExtensionRef
//! chains concatenate and retain the declaring route's namespace); child hostnames are forbidden; rule-level attributes
//! are child-first-else-inherited; wildcard (`name: "*"`) selects all routes in
//! the target namespace; delegation is parent opt-in (children may reverse-lock
//! via `parentRefs {kind: HTTPRoute}`); cycles and depth > 8 drop the edge.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::http_route::{
    DelegationIssue, HTTPBackendRef, HTTPBackendRefKind, HTTPPathMatch, HTTPRoute, HTTPRouteFilter,
    HTTPRouteFilterType, HTTPRouteMatch, HTTPRouteRetry, HTTPRouteRule, HTTPRouteTimeouts, SessionPersistence,
    GATEWAY_API_GROUP,
};

/// `(group, kind, namespace(explicit), name)` of a Service backendRef.
pub type ServiceBackendTuple = (Option<String>, Option<String>, Option<String>, String);
/// `(group, kind, namespace(explicit), name)` of an EdgionBackend backendRef.
pub type EdgionBackendTuple = (Option<String>, Option<String>, Option<String>, String);

/// Maximum delegation chain depth (number of delegation edges from the root).
pub const MAX_DELEGATION_DEPTH: usize = 8;

const HTTP_ROUTE_KIND: &str = "HTTPRoute";

/// Replace every operator-provided or stale terminal identity on the route's
/// own rules with controller-derived values. Delegated terminal child rules are
/// stamped independently while flattening because lookup returns raw resources.
pub(super) fn populate_terminal_rule_identities(route: &mut HTTPRoute) {
    let route_identity = terminal_route_identity(route);
    for (index, rule) in route.spec.rules.iter_mut().flatten().enumerate() {
        set_terminal_rule_identity(rule, &route_identity, index);
    }
}

fn terminal_route_identity(route: &HTTPRoute) -> String {
    route.metadata.uid.clone().unwrap_or_else(|| {
        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        let name = route.metadata.name.as_deref().unwrap_or("");
        format!("{HTTP_ROUTE_KIND}/{namespace}/{name}")
    })
}

fn set_terminal_rule_identity(rule: &mut HTTPRouteRule, route_identity: &str, index: usize) {
    rule.resolved_terminal_route_uid = Some(route_identity.to_string());
    rule.resolved_terminal_rule_identity = Some(match rule.name.as_deref() {
        Some(name) => format!("name:{name}"),
        None => format!("index:{index}"),
    });
}

/// Store abstraction so flattening is unit-testable without a ControllerRuntime.
pub trait DelegationLookup {
    /// Look up an HTTPRoute by `"ns/name"` key.
    fn get_route(&self, key: &str) -> Option<Arc<HTTPRoute>>;
    /// All HTTPRoute keys in a namespace, sorted (for deterministic wildcard
    /// expansion order).
    fn list_route_keys_in_namespace(&self, ns: &str) -> Vec<String>;
    /// ReferenceGrant check for a flattened Service backendRef. `from_ns` is
    /// the namespace of the child route that declares the ref.
    fn cross_ns_ref_allowed(
        &self,
        from_ns: &str,
        group: Option<&str>,
        kind: Option<&str>,
        name: &str,
        to_ns: &str,
    ) -> bool;
}

/// Result of flattening one parent route.
pub struct FlattenOutcome {
    /// `None` ⇔ the route has no delegating rule (gateway keeps compiling
    /// `spec.rules`).
    pub resolved_rules: Option<Vec<HTTPRouteRule>>,
    pub issues: Vec<DelegationIssue>,
    /// Every named child key referenced anywhere in the delegation tree
    /// (including missing ones, so their later creation requeues this route).
    pub named_children: HashSet<String>,
    /// Every wildcard-delegated namespace anywhere in the delegation tree.
    pub wildcard_namespaces: HashSet<String>,
    /// `(group, kind, namespace(explicit), name)` of Service backendRefs in
    /// the flattened rules, for ServiceRefManager registration on the parent.
    pub service_backend_tuples: Vec<ServiceBackendTuple>,
    /// EdgionBackend backendRefs in flattened rules, with the declaring
    /// child namespace made explicit for the parent's reverse-ref index.
    pub backend_tuples: Vec<EdgionBackendTuple>,
    /// Namespaces referenced cross-namespace by flattened Service backendRefs
    /// (target ns ≠ declaring child ns), for CrossNsRefManager registration on
    /// the parent so ReferenceGrant changes requeue it.
    pub cross_ns_target_namespaces: HashSet<String>,
    /// Child keys whose rules were actually expanded into `resolved_rules`.
    /// The handler re-checks their existence after registering index edges to
    /// close the race where a child is deleted between the flatten read and
    /// the edge registration (its on_delete would miss the not-yet-registered
    /// edge and never requeue this parent).
    pub expanded_children: HashSet<String>,
}

impl FlattenOutcome {
    fn no_delegation() -> Self {
        Self {
            resolved_rules: None,
            issues: Vec::new(),
            named_children: HashSet::new(),
            wildcard_namespaces: HashSet::new(),
            service_backend_tuples: Vec::new(),
            backend_tuples: Vec::new(),
            cross_ns_target_namespaces: HashSet::new(),
            expanded_children: HashSet::new(),
        }
    }
}

/// A delegated match scope handed down one delegation edge: the path prefix
/// plus the mandatory non-path constraints accumulated from ancestor matches.
#[derive(Clone, Debug)]
struct DelegatedScope {
    prefix: String,
    headers: Vec<edgion_resources::resources::http_route::HTTPHeaderMatch>,
    query_params: Vec<edgion_resources::resources::http_route::HTTPQueryParamMatch>,
    method: Option<String>,
}

/// Rule-level attributes inherited down the chain (child-first-else-inherited).
#[derive(Clone, Default)]
struct InheritedAttrs {
    timeouts: Option<HTTPRouteTimeouts>,
    retry: Option<HTTPRouteRetry>,
    session_persistence: Option<SessionPersistence>,
}

/// Filters inherited down the chain. Standard filters are mandatory and
/// conflict with same-type child filters; ExtensionRef filters concatenate.
#[derive(Clone, Default)]
struct InheritedFilters {
    standard: Vec<HTTPRouteFilter>,
    extension: Vec<HTTPRouteFilter>,
}

impl InheritedFilters {
    fn standard_types(&self) -> Vec<HTTPRouteFilterType> {
        self.standard.iter().map(|f| f.filter_type.clone()).collect()
    }
}

struct FlattenCtx<'a> {
    lookup: &'a dyn DelegationLookup,
    issues: Vec<DelegationIssue>,
    named_children: HashSet<String>,
    wildcard_namespaces: HashSet<String>,
    service_backend_tuples: Vec<ServiceBackendTuple>,
    backend_tuples: Vec<EdgionBackendTuple>,
    cross_ns_target_namespaces: HashSet<String>,
    expanded_children: HashSet<String>,
    /// Route keys on the current DFS path (root first), for cycle detection.
    path: Vec<String>,
}

/// Flatten the delegation tree rooted at `parent`.
///
/// Pure over `lookup`; does not touch any runtime index (the caller registers
/// `named_children` / `wildcard_namespaces` / `service_backend_tuples`).
pub fn flatten_route(parent: &HTTPRoute, lookup: &dyn DelegationLookup) -> FlattenOutcome {
    let parent_ns = parent.metadata.namespace.as_deref().unwrap_or("default");
    let parent_name = parent.metadata.name.as_deref().unwrap_or("");
    let parent_key = format!("{parent_ns}/{parent_name}");

    let Some(rules) = &parent.spec.rules else {
        return FlattenOutcome::no_delegation();
    };
    let has_delegation = rules.iter().any(|r| {
        r.backend_refs
            .as_ref()
            .is_some_and(|brs| brs.iter().any(HTTPBackendRef::is_route_delegation_ref))
    });
    if !has_delegation {
        return FlattenOutcome::no_delegation();
    }

    let mut ctx = FlattenCtx {
        lookup,
        issues: Vec::new(),
        named_children: HashSet::new(),
        wildcard_namespaces: HashSet::new(),
        service_backend_tuples: Vec::new(),
        backend_tuples: Vec::new(),
        cross_ns_target_namespaces: HashSet::new(),
        expanded_children: HashSet::new(),
        path: vec![parent_key],
    };

    // Rules already invalid per the standing per-rule checks are dropped here;
    // the gateway does not apply `invalid_rule_indices` to resolved_rules.
    let invalid_indices: HashSet<usize> = super::http_route::find_invalid_rule_details(parent, parent_ns)
        .into_iter()
        .map(|(i, _)| i)
        .collect();

    let mut out_rules: Vec<HTTPRouteRule> = Vec::new();
    for (idx, rule) in rules.iter().enumerate() {
        if invalid_indices.contains(&idx) {
            continue;
        }
        let is_delegating = rule
            .backend_refs
            .as_ref()
            .is_some_and(|brs| brs.iter().any(HTTPBackendRef::is_route_delegation_ref));
        if !is_delegating {
            let mut copied = rule.clone();
            collect_backend_tuples(&mut ctx, parent_ns, &copied);
            // Non-delegating parent rule: keep operator-authored semantics
            // untouched (implicit backendRef namespace = parent ns is fine
            // because resolved_rules live on the parent).
            copied.name = rule.name.clone();
            push_effective_rule(&mut ctx, copied, &format!("parent rules[{idx}]"), &mut out_rules);
            continue;
        }

        match validate_delegating_rule(rule, idx) {
            Err(message) => ctx.issues.push(DelegationIssue::RuleDropped { message }),
            Ok((dref, scopes)) => {
                let inherited = InheritedFilters {
                    standard: rule
                        .filters
                        .iter()
                        .flatten()
                        .filter(|f| f.filter_type != HTTPRouteFilterType::ExtensionRef)
                        .cloned()
                        .collect(),
                    extension: rule
                        .filters
                        .iter()
                        .flatten()
                        .filter(|f| f.filter_type == HTTPRouteFilterType::ExtensionRef)
                        .cloned()
                        .map(|filter| with_extension_ref_namespace(filter, parent_ns))
                        .collect(),
                };
                let attrs = InheritedAttrs {
                    timeouts: rule.timeouts.clone(),
                    retry: rule.retry.clone(),
                    session_persistence: rule.session_persistence.clone(),
                };
                expand_delegation_ref(
                    &mut ctx,
                    parent_ns,
                    dref,
                    &scopes,
                    &inherited,
                    &attrs,
                    1,
                    &mut out_rules,
                );
            }
        }
    }

    FlattenOutcome {
        resolved_rules: Some(out_rules),
        issues: ctx.issues,
        named_children: ctx.named_children,
        wildcard_namespaces: ctx.wildcard_namespaces,
        service_backend_tuples: ctx.service_backend_tuples,
        backend_tuples: ctx.backend_tuples,
        cross_ns_target_namespaces: ctx.cross_ns_target_namespaces,
        expanded_children: ctx.expanded_children,
    }
}

/// Validate a delegating rule's shape and derive its delegated scopes.
///
/// Constraints (spec §"What makes a rule delegating"): exactly one backendRef;
/// `port` and per-backendRef `filters` absent; every match is `PathPrefix`
/// (a match with no path delegates `/`).
fn validate_delegating_rule(
    rule: &HTTPRouteRule,
    rule_idx: usize,
) -> Result<(&HTTPBackendRef, Vec<DelegatedScope>), String> {
    let backend_refs = rule
        .backend_refs
        .as_ref()
        .expect("caller checked delegation ref presence");
    if backend_refs.len() != 1 {
        return Err(format!(
            "rules[{rule_idx}]: a delegating rule must have exactly one backendRef (found {})",
            backend_refs.len()
        ));
    }
    let dref = &backend_refs[0];
    if dref.port.is_some() {
        return Err(format!("rules[{rule_idx}]: delegation backendRef must not set 'port'"));
    }
    if dref.filters.is_some() {
        return Err(format!(
            "rules[{rule_idx}]: delegation backendRef must not carry per-backend filters"
        ));
    }

    let mut scopes = Vec::new();
    match rule.matches.as_ref() {
        None => scopes.push(DelegatedScope {
            prefix: "/".to_string(),
            headers: Vec::new(),
            query_params: Vec::new(),
            method: None,
        }),
        Some(matches) => {
            for m in matches {
                let (match_type, prefix) = match m.path.as_ref() {
                    None => ("PathPrefix", "/"),
                    Some(p) => (
                        p.match_type.as_deref().unwrap_or("PathPrefix"),
                        p.value.as_deref().unwrap_or("/"),
                    ),
                };
                if match_type != "PathPrefix" {
                    return Err(format!(
                        "rules[{rule_idx}]: delegating rule matches must use PathPrefix (found {match_type})"
                    ));
                }
                scopes.push(DelegatedScope {
                    prefix: normalize_path_value(prefix),
                    headers: m.headers.clone().unwrap_or_default(),
                    query_params: m.query_params.clone().unwrap_or_default(),
                    method: m.method.clone(),
                });
            }
        }
    }
    Ok((dref, scopes))
}

/// Expand one delegation edge (a validated delegating rule → its children).
#[allow(clippy::too_many_arguments)]
fn expand_delegation_ref(
    ctx: &mut FlattenCtx,
    referrer_ns: &str,
    dref: &HTTPBackendRef,
    scopes: &[DelegatedScope],
    inherited: &InheritedFilters,
    attrs: &InheritedAttrs,
    depth: usize,
    out_rules: &mut Vec<HTTPRouteRule>,
) {
    let target_ns = dref.namespace.as_deref().unwrap_or(referrer_ns);
    let current_key = ctx.path.last().expect("DFS path is never empty").clone();

    let child_keys: Vec<String> = if dref.name == "*" {
        ctx.wildcard_namespaces.insert(target_ns.to_string());
        ctx.lookup
            .list_route_keys_in_namespace(target_ns)
            .into_iter()
            .filter(|k| *k != current_key)
            .collect()
    } else {
        let key = format!("{target_ns}/{}", dref.name);
        ctx.named_children.insert(key.clone());
        vec![key]
    };

    for child_key in child_keys {
        if ctx.path.contains(&child_key) {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!(
                    "DelegationLoop: '{child_key}' is already on the delegation chain {:?}",
                    ctx.path
                ),
            });
            continue;
        }
        if depth > MAX_DELEGATION_DEPTH {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!("DelegationDepthExceeded: chain deeper than {MAX_DELEGATION_DEPTH} at '{child_key}'"),
            });
            continue;
        }
        let Some(child) = ctx.lookup.get_route(&child_key) else {
            if dref.name != "*" {
                let (ns, name) = child_key.split_once('/').unwrap_or((target_ns, dref.name.as_str()));
                ctx.issues.push(DelegationIssue::MissingChild {
                    namespace: ns.to_string(),
                    name: name.to_string(),
                });
            }
            continue;
        };

        if child.spec.hostnames.is_some() {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!("child '{child_key}' skipped: child routes must not set spec.hostnames"),
            });
            continue;
        }
        if !reverse_lock_allows(&child, &current_key) {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!(
                    "child '{child_key}' skipped: its parentRefs restrict delegation and do not list '{current_key}'"
                ),
            });
            continue;
        }

        ctx.expanded_children.insert(child_key.clone());
        ctx.path.push(child_key.clone());
        flatten_child_rules(ctx, &child, &child_key, scopes, inherited, attrs, depth, out_rules);
        ctx.path.pop();
    }
}

/// Flatten the rules of one admitted child route under the given scopes.
#[allow(clippy::too_many_arguments)]
fn flatten_child_rules(
    ctx: &mut FlattenCtx,
    child: &HTTPRoute,
    child_key: &str,
    scopes: &[DelegatedScope],
    inherited: &InheritedFilters,
    attrs: &InheritedAttrs,
    depth: usize,
    out_rules: &mut Vec<HTTPRouteRule>,
) {
    let child_ns = child.metadata.namespace.as_deref().unwrap_or("default");
    let child_name = child.metadata.name.as_deref().unwrap_or("");
    let Some(rules) = &child.spec.rules else {
        return;
    };
    let invalid_details: HashMap<usize, String> = super::http_route::find_invalid_rule_details(child, child_ns)
        .into_iter()
        .collect();

    for (idx, rule) in rules.iter().enumerate() {
        if let Some(reason) = invalid_details.get(&idx) {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!("child '{child_key}' rules[{idx}] dropped: {reason}"),
            });
            continue;
        }

        // Raw child rules have not necessarily passed their own handler yet.
        // Resolve mirrors before inheritance, using this declaring child even
        // when a terminal rule belongs to a deeper descendant.
        let mut resolved_rule = rule.clone();
        let targets = super::request_mirror::resolve_rule(&mut resolved_rule, child_ns, |backend| {
            ctx.lookup.cross_ns_ref_allowed(
                child_ns,
                Some(&backend.group),
                backend.kind.as_deref(),
                &backend.name,
                backend.namespace.as_deref().unwrap_or(child_ns),
            )
        });
        ctx.cross_ns_target_namespaces.extend(targets);
        let rule = &resolved_rule;

        let is_delegating = rule
            .backend_refs
            .as_ref()
            .is_some_and(|brs| brs.iter().any(HTTPBackendRef::is_route_delegation_ref));

        // Both terminal and nested-delegating rules must fit the incoming
        // scopes and pass the mandatory-filter conflict check.
        let inherited_std_types = inherited.standard_types();
        // Check rule-level AND per-backendRef filters: a child must not be
        // able to smuggle a mandatory-filter override through br.filters.
        let child_std_conflict = rule
            .filters
            .iter()
            .flatten()
            .chain(
                rule.backend_refs
                    .iter()
                    .flatten()
                    .flat_map(|br| br.filters.iter().flatten()),
            )
            .find(|f| {
                f.filter_type != HTTPRouteFilterType::ExtensionRef && inherited_std_types.contains(&f.filter_type)
            });
        if let Some(conflict) = child_std_conflict {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!(
                    "child '{child_key}' rules[{idx}] dropped: standard filter {:?} conflicts with an inherited (mandatory) parent filter",
                    conflict.filter_type
                ),
            });
            continue;
        }

        if is_delegating {
            // Nested delegation: the delegating rule's own matches must fit
            // the incoming scopes; its merged matches become the new scopes.
            match validate_delegating_rule(rule, idx) {
                Err(message) => ctx.issues.push(DelegationIssue::RuleDropped {
                    message: format!("child '{child_key}' {message}"),
                }),
                Ok((dref, own_scopes)) => {
                    let mut merged_scopes = Vec::new();
                    let mut fit_error: Option<String> = None;
                    for own in &own_scopes {
                        let mut fitted = false;
                        for scope in scopes {
                            if !prefix_contains(&scope.prefix, "PathPrefix", &own.prefix) {
                                continue;
                            }
                            match merge_scope_constraints(scope, &own.headers, &own.query_params, own.method.as_deref())
                            {
                                Ok((headers, query_params, method)) => {
                                    merged_scopes.push(DelegatedScope {
                                        prefix: own.prefix.clone(),
                                        headers,
                                        query_params,
                                        method,
                                    });
                                    fitted = true;
                                }
                                Err(e) => fit_error = Some(e),
                            }
                        }
                        if !fitted {
                            fit_error.get_or_insert_with(|| {
                                format!("prefix '{}' is outside every delegated prefix", own.prefix)
                            });
                            merged_scopes.clear();
                            break;
                        }
                    }
                    if merged_scopes.is_empty() {
                        ctx.issues.push(DelegationIssue::RuleDropped {
                            message: format!(
                                "child '{child_key}' rules[{idx}] dropped: {}",
                                fit_error.unwrap_or_else(|| "no delegated scope fits".to_string())
                            ),
                        });
                        continue;
                    }
                    let next_inherited = InheritedFilters {
                        standard: inherited
                            .standard
                            .iter()
                            .chain(
                                rule.filters
                                    .iter()
                                    .flatten()
                                    .filter(|f| f.filter_type != HTTPRouteFilterType::ExtensionRef),
                            )
                            .cloned()
                            .collect(),
                        extension: inherited
                            .extension
                            .iter()
                            .cloned()
                            .chain(
                                rule.filters
                                    .iter()
                                    .flatten()
                                    .filter(|f| f.filter_type == HTTPRouteFilterType::ExtensionRef)
                                    .cloned()
                                    .map(|filter| with_extension_ref_namespace(filter, child_ns)),
                            )
                            .collect(),
                    };
                    let next_attrs = InheritedAttrs {
                        timeouts: rule.timeouts.clone().or_else(|| attrs.timeouts.clone()),
                        retry: rule.retry.clone().or_else(|| attrs.retry.clone()),
                        session_persistence: rule
                            .session_persistence
                            .clone()
                            .or_else(|| attrs.session_persistence.clone()),
                    };
                    expand_delegation_ref(
                        ctx,
                        child_ns,
                        dref,
                        &merged_scopes,
                        &next_inherited,
                        &next_attrs,
                        depth + 1,
                        out_rules,
                    );
                }
            }
            continue;
        }

        // Terminal rule: merge matches against the scopes.
        let child_matches: Vec<HTTPRouteMatch> = rule.matches.clone().unwrap_or_else(|| {
            vec![HTTPRouteMatch {
                path: None,
                headers: None,
                query_params: None,
                method: None,
            }]
        });

        let mut flattened_matches: Vec<HTTPRouteMatch> = Vec::new();
        let mut drop_reason: Option<String> = None;
        'matches: for cm in &child_matches {
            let mut fitted = false;
            for scope in scopes {
                match merge_match(scope, cm) {
                    MergeResult::NotContained => continue,
                    MergeResult::Conflict(e) => {
                        drop_reason = Some(e);
                        break 'matches;
                    }
                    MergeResult::Merged(m) => {
                        flattened_matches.push(m);
                        fitted = true;
                    }
                }
            }
            if !fitted {
                drop_reason = Some(format!(
                    "match path {:?} is outside every delegated prefix",
                    cm.path.as_ref().and_then(|p| p.value.as_deref())
                ));
                break;
            }
        }
        if let Some(reason) = drop_reason {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!("child '{child_key}' rules[{idx}] dropped: {reason}"),
            });
            continue;
        }

        let mut flattened = rule.clone();
        let child_route_identity = terminal_route_identity(child);
        set_terminal_rule_identity(&mut flattened, &child_route_identity, idx);
        flattened.matches = Some(flattened_matches);
        flattened.name = Some(
            rule.name
                .clone()
                .unwrap_or_else(|| format!("{child_ns}.{child_name}.r{idx}")),
        );
        flattened.timeouts = rule.timeouts.clone().or_else(|| attrs.timeouts.clone());
        flattened.retry = rule.retry.clone().or_else(|| attrs.retry.clone());
        flattened.session_persistence = rule
            .session_persistence
            .clone()
            .or_else(|| attrs.session_persistence.clone());
        flattened.filters = {
            let combined: Vec<HTTPRouteFilter> = inherited
                .standard
                .iter()
                .chain(inherited.extension.iter())
                .cloned()
                .chain(
                    rule.filters
                        .iter()
                        .flatten()
                        .cloned()
                        .map(|filter| with_extension_ref_namespace(filter, child_ns)),
                )
                .collect();
            if combined.is_empty() {
                None
            } else {
                Some(combined)
            }
        };

        // Child backendRefs keep child-local semantics on the parent: make the
        // namespace explicit and run the ReferenceGrant check with the CHILD
        // namespace as referrer.
        if let Some(backend_refs) = flattened.backend_refs.as_mut() {
            for br in backend_refs.iter_mut() {
                for filter in br.filters.iter_mut().flatten() {
                    if filter.filter_type == HTTPRouteFilterType::ExtensionRef {
                        if let Some(extension_ref) = filter.extension_ref.as_mut() {
                            extension_ref.resolved_namespace = Some(child_ns.to_string());
                        }
                    }
                }
                let target_ns = br.namespace.clone().unwrap_or_else(|| child_ns.to_string());
                br.namespace = Some(target_ns.clone());
                if target_ns != child_ns {
                    ctx.cross_ns_target_namespaces.insert(target_ns.clone());
                }
                if target_ns != child_ns
                    && !ctx.lookup.cross_ns_ref_allowed(
                        child_ns,
                        br.group.as_deref(),
                        br.kind.as_deref(),
                        &br.name,
                        &target_ns,
                    )
                {
                    br.ref_denied = Some(RefDenied {
                        target_namespace: target_ns,
                        target_name: br.name.clone(),
                        reason: Some("NoMatchingReferenceGrant".to_string()),
                    });
                } else {
                    br.ref_denied = None;
                }
            }
        }
        collect_backend_tuples(ctx, child_ns, &flattened);
        push_effective_rule(ctx, flattened, &format!("child '{child_key}' rules[{idx}]"), out_rules);
    }
}

fn push_effective_rule(
    ctx: &mut FlattenCtx<'_>,
    rule: HTTPRouteRule,
    source: &str,
    out_rules: &mut Vec<HTTPRouteRule>,
) {
    match edgion_resources::effective_request_transform_violations(&rule) {
        Ok(violations) => {
            if let Some(violation) = violations.into_iter().next() {
                ctx.issues.push(DelegationIssue::RuleDropped {
                    message: format!("{source} dropped: {violation}"),
                });
            } else {
                out_rules.push(rule);
            }
        }
        Err(_) => ctx.issues.push(DelegationIssue::RuleDropped {
            message: format!("{source} dropped: request transform validation contract is unavailable"),
        }),
    }
}

enum MergeResult {
    /// The child match path is not under this scope's prefix.
    NotContained,
    /// Contained, but the mandatory constraints contradict the child match.
    Conflict(String),
    Merged(HTTPRouteMatch),
}

/// True for the Gateway API structural default `{type: PathPrefix, value: "/"}`.
///
/// `edgion-resources` backfills this shape whenever `HTTPRouteMatch.path` is
/// omitted (the File/Etcd backends never pass through K8s admission), so by
/// the time a child rule reaches the flattener an "omitted" path is
/// indistinguishable from an explicit root prefix. Inside a delegated scope
/// both mean "everything delegated to me" and inherit the scope prefix;
/// treating them as a literal `/` instead would put every method/header-only
/// child match outside its own scope and drop it.
fn is_root_prefix_match(p: &HTTPPathMatch) -> bool {
    p.match_type.as_deref().unwrap_or("PathPrefix") == "PathPrefix" && p.value.as_deref().unwrap_or("/") == "/"
}

/// Merge one child match into one delegated scope.
fn merge_match(scope: &DelegatedScope, child_match: &HTTPRouteMatch) -> MergeResult {
    // Path containment. A child match with no path — or with the structural
    // default `PathPrefix /` that stands in for it — inherits the scope prefix.
    let merged_path: HTTPPathMatch = match child_match.path.as_ref() {
        None => HTTPPathMatch {
            match_type: Some("PathPrefix".to_string()),
            value: Some(scope.prefix.clone()),
        },
        Some(p) if is_root_prefix_match(p) => HTTPPathMatch {
            match_type: Some("PathPrefix".to_string()),
            value: Some(scope.prefix.clone()),
        },
        Some(p) => {
            let match_type = p.match_type.as_deref().unwrap_or("PathPrefix");
            // Containment MUST be decided on the value the gateway will
            // ultimately compile: the gateway normalizes Exact/PathPrefix
            // values of resolved_rules at load time, so `/api/../admin` would
            // collapse to `/admin` AFTER a naive string containment check
            // accepted it as being under `/api` — a prefix escape. Normalize
            // here (idempotent with the gateway's own pass) and persist the
            // normalized value into the flattened rule.
            let raw = p.value.as_deref().unwrap_or("/");
            let normalizable = is_normalizable_match_type(match_type);
            let value = if normalizable {
                normalize_path_value(raw)
            } else {
                raw.to_string()
            };
            if !prefix_contains(&scope.prefix, match_type, &value) {
                return MergeResult::NotContained;
            }
            let mut normalized = p.clone();
            if normalizable {
                normalized.value = Some(value);
            }
            normalized
        }
    };

    match merge_scope_constraints(
        scope,
        child_match.headers.as_deref().unwrap_or(&[]),
        child_match.query_params.as_deref().unwrap_or(&[]),
        child_match.method.as_deref(),
    ) {
        Err(e) => MergeResult::Conflict(e),
        Ok((headers, query_params, method)) => MergeResult::Merged(HTTPRouteMatch {
            path: Some(merged_path),
            headers: if headers.is_empty() { None } else { Some(headers) },
            query_params: if query_params.is_empty() {
                None
            } else {
                Some(query_params)
            },
            method,
        }),
    }
}

type MergedConstraints = (
    Vec<edgion_resources::resources::http_route::HTTPHeaderMatch>,
    Vec<edgion_resources::resources::http_route::HTTPQueryParamMatch>,
    Option<String>,
);

/// Merge the scope's mandatory headers/queryParams/method with a child's.
/// Same name with different value/type, or a differing method, is a conflict.
fn merge_scope_constraints(
    scope: &DelegatedScope,
    child_headers: &[edgion_resources::resources::http_route::HTTPHeaderMatch],
    child_query: &[edgion_resources::resources::http_route::HTTPQueryParamMatch],
    child_method: Option<&str>,
) -> Result<MergedConstraints, String> {
    let mut headers = child_headers.to_vec();
    for sh in &scope.headers {
        match headers.iter().find(|ch| ch.name.eq_ignore_ascii_case(&sh.name)) {
            None => headers.push(sh.clone()),
            Some(ch) => {
                let type_eq =
                    ch.match_type.as_deref().unwrap_or("Exact") == sh.match_type.as_deref().unwrap_or("Exact");
                if !(type_eq && ch.value == sh.value) {
                    return Err(format!(
                        "header match '{}' contradicts the inherited (mandatory) parent match",
                        sh.name
                    ));
                }
            }
        }
    }

    let mut query_params = child_query.to_vec();
    for sq in &scope.query_params {
        match query_params.iter().find(|cq| cq.name == sq.name) {
            None => query_params.push(sq.clone()),
            Some(cq) => {
                let type_eq =
                    cq.match_type.as_deref().unwrap_or("Exact") == sq.match_type.as_deref().unwrap_or("Exact");
                if !(type_eq && cq.value == sq.value) {
                    return Err(format!(
                        "queryParam match '{}' contradicts the inherited (mandatory) parent match",
                        sq.name
                    ));
                }
            }
        }
    }

    let method = match (scope.method.as_deref(), child_method) {
        (None, m) => m.map(str::to_string),
        (Some(sm), None) => Some(sm.to_string()),
        (Some(sm), Some(cm)) if sm == cm => Some(sm.to_string()),
        (Some(sm), Some(cm)) => {
            return Err(format!(
                "method match '{cm}' contradicts the inherited (mandatory) parent method '{sm}'"
            ))
        }
    };

    Ok((headers, query_params, method))
}

/// True if the pattern contains a `|` that is not escaped with a backslash.
///
/// Group nesting is deliberately NOT tracked: a delegated regex has no
/// legitimate need for alternation that our conservative check would wrongly
/// reject, and tracking groups would risk a parser mismatch with the regex
/// crate's own precedence rules (the whole point of this guard).
fn has_unescaped_alternation(pattern: &str) -> bool {
    let mut escaped = false;
    for c in pattern.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '|' => return true,
            _ => {}
        }
    }
    false
}

/// Match types whose path values the gateway normalizes at load time
/// (`normalize_path_matchers`). Regex patterns are never normalized.
fn is_normalizable_match_type(match_type: &str) -> bool {
    match_type == "Exact" || match_type == "PathPrefix"
}

/// Normalize a path value exactly as the gateway will at load time.
///
/// The controller does not run `pre_parse` on HTTPRoute objects, so raw store
/// values still carry dot-segments / percent-encodings. Containment must be
/// decided on the post-normalization value, otherwise `/api/../admin` passes
/// as "under /api" here and compiles as `/admin` on the data plane.
fn normalize_path_value(value: &str) -> String {
    edgion_resources::path_normalization::normalize_config_path(
        value,
        edgion_resources::path_normalization::get_path_normalization_config(),
    )
}

/// Is a child path value contained in the delegated prefix?
///
/// - `PathPrefix v` / `Exact v`: `v == P`, or `P == "/"`, or `v` extends `P`
///   at a segment boundary.
/// - `RegularExpression r`: conservative static check — after stripping a
///   leading `^`, the pattern must start with the regex-escaped literal `P`
///   (so a `P` containing regex metacharacters never false-passes).
fn prefix_contains(prefix: &str, match_type: &str, value: &str) -> bool {
    let p = if prefix == "/" {
        "/"
    } else {
        prefix.trim_end_matches('/')
    };
    match match_type {
        "PathPrefix" | "Exact" => {
            if p == "/" {
                return value.starts_with('/');
            }
            value == p || value.starts_with(&format!("{p}/"))
        }
        "RegularExpression" => {
            // The gateway matches path regexes UNANCHORED
            // (`Regex::is_match`, see routes/http/match_unit.rs), so a pattern
            // without a leading `^` can match anywhere in the path regardless
            // of how it starts. Containment therefore REQUIRES an explicit
            // anchor followed by the regex-escaped literal prefix.
            let Some(pattern) = value.strip_prefix('^') else {
                return false;
            };
            // Alternation has the LOWEST precedence, so `^/api/x|/evil` parses
            // as `(^/api/x)|(/evil)` — the second branch is unanchored and
            // escapes the prefix. Any unescaped `|` (at any nesting level; we
            // do not track groups) is rejected conservatively.
            if has_unescaped_alternation(pattern) {
                return false;
            }
            if p == "/" {
                return pattern.starts_with('/');
            }
            let escaped = regex::escape(p);
            pattern.starts_with(&escaped)
                && (pattern.len() == escaped.len() || pattern[escaped.len()..].starts_with('/'))
        }
        _ => false,
    }
}

/// True if the child either declares no HTTPRoute-kind parentRefs (accepts any
/// delegating parent) or lists `parent_key` among them.
fn reverse_lock_allows(child: &HTTPRoute, parent_key: &str) -> bool {
    let child_ns = child.metadata.namespace.as_deref().unwrap_or("default");
    let mut has_lock = false;
    for pr in child.spec.parent_refs.iter().flatten() {
        if pr.kind.as_deref() != Some("HTTPRoute")
            || pr.group.as_deref().unwrap_or(GATEWAY_API_GROUP) != GATEWAY_API_GROUP
        {
            continue;
        }
        has_lock = true;
        let ns = pr.namespace.as_deref().unwrap_or(child_ns);
        if format!("{ns}/{}", pr.name) == parent_key {
            return true;
        }
    }
    !has_lock
}

fn with_extension_ref_namespace(mut filter: HTTPRouteFilter, namespace: &str) -> HTTPRouteFilter {
    if filter.filter_type == HTTPRouteFilterType::ExtensionRef {
        if let Some(extension_ref) = filter.extension_ref.as_mut() {
            extension_ref.resolved_namespace = Some(namespace.to_string());
        }
    }
    filter
}

/// Record typed backendRef tuples of a flattened/copied rule for reverse-ref
/// registration under the parent route.
fn collect_backend_tuples(ctx: &mut FlattenCtx, implicit_ns: &str, rule: &HTTPRouteRule) {
    for br in rule.backend_refs.iter().flatten() {
        let tuple = (
            br.group.clone(),
            br.kind.clone(),
            Some(br.namespace.clone().unwrap_or_else(|| implicit_ns.to_string())),
            br.name.clone(),
        );
        match br.backend_kind() {
            HTTPBackendRefKind::Service => ctx.service_backend_tuples.push(tuple),
            HTTPBackendRefKind::EdgionBackend => ctx.backend_tuples.push(tuple),
            HTTPBackendRefKind::RouteDelegation | HTTPBackendRefKind::Unsupported => {}
        }
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/route_delegation_test.rs"]
mod tests;
