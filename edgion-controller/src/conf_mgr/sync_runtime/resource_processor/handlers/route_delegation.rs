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
mod tests {
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
                assert!(dropped_messages(&outcome).iter().any(|message| message
                    .contains("child 'infra/child' rules[0]")
                    && message.contains("exposeHeaders")));
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
            assert!(dropped_messages(&outcome).iter().any(|message| {
                message.contains("child 'infra/child' rules[0]") && message.contains(expected_field)
            }));
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

        // Service tuples registered with explicit namespaces
        assert!(outcome
            .service_backend_tuples
            .iter()
            .any(|(_, _, ns, name)| ns.as_deref() == Some("billing") && name == "svc-local"));
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
}
