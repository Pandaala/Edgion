//! HTTPRoute delegation flattening.
//!
//! A parent HTTPRoute rule may delegate its path prefix to child HTTPRoutes
//! (possibly in other namespaces) via `backendRef {group:
//! gateway.networking.k8s.io, kind: HTTPRoute}`. During `parse`, the
//! controller expands the delegation tree into a flat rule list stored in
//! `spec.resolved_rules`; the gateway compiles that list and never sees the
//! nesting, but retains each effective match's ancestor path constraints.
//!
//! Canonical semantics: `docs/en/user-guide/http-route/delegation.md`; controller
//! architecture: `skills/01-architecture/05-resources/04-http-route.md`.
//! Key locked decisions: child paths are absolute and must be contained in the
//! delegated prefix (regexes are intersected with it at runtime); parent filters are mandatory and non-overridable (a child
//! standard filter of an inherited type invalidates the child rule; ExtensionRef
//! chains concatenate and retain the declaring route's namespace); child hostnames are forbidden; rule-level attributes
//! are child-first-else-inherited; wildcard (`name: "*"`) selects all routes in
//! the target namespace; delegation is parent opt-in (children may reverse-lock
//! via `parentRefs {kind: HTTPRoute}`); cycles and depth > 8 drop the edge.

use super::route_backend_refs::{self, BackendDependencyReceipt};
use super::route_utils::ExtensionDependencyReceipt;
use edgion_resources::resources::refs::route_fields::{
    AuthoredBackendReference, AuthoredRouteTarget, HttpRouteBackendRefs, HttpRouteExtensionRefs, RouteExtensionField,
    TerminalBackendReference,
};
use edgion_resources::resources::refs::route_fields::{HttpRouteMirrorRefs, MirrorSlot, TerminalMirrorReference};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::http_route::{
    DelegationIssue, HTTPBackendRef, HTTPPathMatch, HTTPRoute, HTTPRouteFilter, HTTPRouteFilterType, HTTPRouteMatch,
    HTTPRouteRetry, HTTPRouteRule, HTTPRouteTimeouts, ParentMatch, ResolvedParentMatch, SessionPersistence,
    GATEWAY_API_GROUP, MAX_HTTP_ROUTE_DELEGATION_DEPTH,
};

/// Maximum delegation chain depth (number of delegation edges from the root).
pub const MAX_DELEGATION_DEPTH: usize = MAX_HTTP_ROUTE_DELEGATION_DEPTH;

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
    /// Actual declaring auth fields only; absent runtime resolution fails closed.
    fn auth_decision(
        &self,
        field: &edgion_resources::resources::refs::route_auth_fields::AuthoredRouteAuthField<'_>,
    ) -> super::route_auth::AuthDecision {
        super::route_auth::AuthDecision::unavailable(field)
    }

    /// Look up an HTTPRoute by `"ns/name"` key.
    fn get_route(&self, key: &str) -> Option<Arc<HTTPRoute>>;
    /// All HTTPRoute keys in a namespace, sorted (for deterministic wildcard
    /// expansion order).
    fn list_route_keys_in_namespace(&self, ns: &str) -> Vec<String>;
    /// Mirror-only ReferenceGrant callback. Native backendRefs use terminal loans.
    fn mirror_ref_denied(&self, terminal: TerminalMirrorReference<'_>) -> Option<RefDenied>;
    /// Authorize only the original declaring Route's validated terminal field.
    fn terminal_ref_denied(&self, terminal: TerminalBackendReference<'_>) -> Option<RefDenied>;
    /// Register actual declaring fields before drops; inspect content only for
    /// structurally eligible rules, always after the recovery edge exists.
    fn extension_ref_error(&self, field: RouteExtensionField<'_>, inspect: bool) -> Option<String>;
}

/// Result of flattening one parent route.
pub struct FlattenOutcome {
    /// `None` for a valid source without delegation (Gateway uses `spec.rules`).
    /// Invalid sources return an empty effective rule set.
    pub resolved_rules: Option<Vec<HTTPRouteRule>>,
    pub issues: Vec<DelegationIssue>,
    pub auth_errors: Vec<super::super::ResolvedRefsError>,
    pub root_auth_decisions: Vec<super::route_auth::AuthDecision>,
    /// Every named child key referenced anywhere in the delegation tree
    /// (including missing ones, so their later creation requeues this route).
    pub named_children: HashSet<String>,
    /// Every wildcard-delegated namespace anywhere in the delegation tree.
    pub wildcard_namespaces: HashSet<String>,
    /// Valid authored terminal dependencies, retained even when a rule is dropped.
    pub(super) backend_dependencies: Vec<BackendDependencyReceipt>,
    pub(super) extension_dependencies: Vec<ExtensionDependencyReceipt>,
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
            auth_errors: Vec::new(),
            root_auth_decisions: Vec::new(),
            named_children: HashSet::new(),
            wildcard_namespaces: HashSet::new(),
            backend_dependencies: Vec::new(),
            extension_dependencies: Vec::new(),

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
    parent_prefixes: Vec<String>,
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
    auth_errors: Vec<super::super::ResolvedRefsError>,
    named_children: HashSet<String>,
    wildcard_namespaces: HashSet<String>,
    backend_dependencies: Vec<BackendDependencyReceipt>,
    extension_dependencies: Vec<ExtensionDependencyReceipt>,

    cross_ns_target_namespaces: HashSet<String>,
    expanded_children: HashSet<String>,
    /// Route keys on the current DFS path (root first), for cycle detection.
    path: Vec<String>,
}

/// Flatten the delegation tree rooted at `parent`.
///
/// Store interactions go through `lookup`; target inspection registers plugin
/// recovery dependencies first. Other returned indexes are published by the caller.
pub fn flatten_route(parent: &HTTPRoute, lookup: &dyn DelegationLookup) -> FlattenOutcome {
    let parent_refs = match HttpRouteBackendRefs::from_route(parent) {
        Ok(references) => references,
        Err(error) => {
            let mut outcome = FlattenOutcome::no_delegation();
            outcome.resolved_rules = Some(Vec::new());
            outcome.issues.push(DelegationIssue::RuleDropped {
                message: route_backend_refs::source_error_message(error),
            });
            return outcome;
        }
    };
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
    let root_auth_decisions = super::route_auth::decisions(parent, |field| lookup.auth_decision(field));
    let auth_errors = root_auth_decisions
        .iter()
        .flat_map(|decision| decision.errors.iter().cloned())
        .collect();
    if !has_delegation {
        let mut outcome = FlattenOutcome::no_delegation();
        outcome.auth_errors = auth_errors;
        outcome.root_auth_decisions = root_auth_decisions;
        return outcome;
    }

    let mut ctx = FlattenCtx {
        lookup,
        issues: Vec::new(),
        auth_errors,
        named_children: HashSet::new(),
        wildcard_namespaces: HashSet::new(),
        backend_dependencies: route_backend_refs::collect_dependencies(parent_refs.authored()),
        extension_dependencies: Vec::new(),

        cross_ns_target_namespaces: HashSet::new(),
        expanded_children: HashSet::new(),
        path: vec![parent_key],
    };

    // Preserve every valid root declaration even when its rule is dropped.
    if let Ok(references) = HttpRouteExtensionRefs::from_route(parent) {
        for field in references.authored() {
            if let Some(receipt) = ExtensionDependencyReceipt::from_field(field) {
                ctx.extension_dependencies.push(receipt);
            }
            lookup.extension_ref_error(field, false);
        }
    }

    // Rules already invalid per the standing per-rule checks are dropped here;
    // the gateway does not apply `invalid_rule_indices` to resolved_rules.
    let invalid_indices: HashSet<usize> = super::http_route::find_invalid_rule_details(parent, parent_ns)
        .into_iter()
        .map(|(i, _)| i)
        .collect();

    let mut out_rules: Vec<HTTPRouteRule> = Vec::new();
    for (idx, rule) in rules.iter().enumerate() {
        let mut resolved_rule = rule.clone();
        super::route_auth::scrub_rule(&mut resolved_rule);
        for decision in root_auth_decisions
            .iter()
            .filter(|decision| decision.slot().rule() == idx)
        {
            decision.clone().apply(&mut resolved_rule);
        }
        let rule = &resolved_rule;
        if invalid_indices.contains(&idx) {
            continue;
        }
        let is_delegating = rule
            .backend_refs
            .as_ref()
            .is_some_and(|brs| brs.iter().any(HTTPBackendRef::is_route_delegation_ref));
        if !is_delegating {
            let mut copied = rule.clone();
            copied.resolved_parent_match = Some(ResolvedParentMatch::Direct {});

            // Non-delegating parent rule: keep operator-authored semantics
            // untouched (implicit backendRef namespace = parent ns is fine
            // because resolved_rules live on the parent).
            copied.name = rule.name.clone();
            push_effective_rule(&mut ctx, copied, &format!("parent rules[{idx}]"), &mut out_rules);
            continue;
        }

        match validate_delegating_rule(rule, idx) {
            Err(message) => ctx.issues.push(DelegationIssue::RuleDropped { message }),
            Ok((_dref, scopes)) => {
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
                    parent_refs.authored_at(idx, 0),
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
        auth_errors: ctx.auth_errors,
        root_auth_decisions,
        named_children: ctx.named_children,
        wildcard_namespaces: ctx.wildcard_namespaces,
        backend_dependencies: ctx.backend_dependencies,
        extension_dependencies: ctx.extension_dependencies,

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
            parent_prefixes: vec!["/".to_string()],
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
                    parent_prefixes: vec![normalize_path_value(prefix)],
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
    reference: Option<AuthoredBackendReference<'_>>,
    scopes: &[DelegatedScope],
    inherited: &InheritedFilters,
    attrs: &InheritedAttrs,
    depth: usize,
    out_rules: &mut Vec<HTTPRouteRule>,
) {
    let selection = reference.map(AuthoredBackendReference::validated);
    let (target_ns, target_name) = match selection {
        Some(Ok(AuthoredRouteTarget::NamedRoute(reference))) => (
            reference.target().scope().namespace().unwrap_or_default(),
            reference.target().name(),
        ),
        Some(Ok(AuthoredRouteTarget::NamespaceRoutes(reference))) => (reference.selector().namespace(), "*"),
        Some(Err(error)) => {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!("Invalid delegation reference: {error}"),
            });
            return;
        }
        _ => {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: "Invalid delegation reference".to_string(),
            });
            return;
        }
    };
    let current_key = ctx.path.last().expect("DFS path is never empty").clone();
    let child_keys: Vec<String> = if target_name == "*" {
        ctx.wildcard_namespaces.insert(target_ns.to_string());
        ctx.lookup
            .list_route_keys_in_namespace(target_ns)
            .into_iter()
            .filter(|key| *key != current_key)
            .collect()
    } else {
        let key = format!("{target_ns}/{target_name}");
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
            if target_name != "*" {
                let (ns, name) = child_key.split_once('/').unwrap_or((target_ns, target_name));
                ctx.issues.push(DelegationIssue::MissingChild {
                    namespace: ns.to_string(),
                    name: name.to_string(),
                });
            }
            continue;
        };

        if let Err(error) = HttpRouteBackendRefs::from_route(&child) {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!(
                    "child '{child_key}' dropped: {}",
                    route_backend_refs::source_error_message(error)
                ),
            });
            continue;
        }
        let extension_refs = HttpRouteExtensionRefs::from_route(&child)
            .expect("the immutable child passed the same owning-source validation");
        // Register the entire child set before structural, scope, or inherited
        // filter rejection; the root must recover when a missing bundle appears.
        for field in extension_refs.authored() {
            if let Some(receipt) = ExtensionDependencyReceipt::from_field(field) {
                ctx.extension_dependencies.push(receipt);
            }
            ctx.lookup.extension_ref_error(field, false);
        }
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
    let Ok(child_refs) = HttpRouteBackendRefs::from_route(child) else {
        return;
    };
    let receipts = route_backend_refs::collect_dependencies(child_refs.authored());
    ctx.cross_ns_target_namespaces.extend(
        receipts
            .iter()
            .filter_map(|receipt| receipt.cross_namespace().map(str::to_string)),
    );
    ctx.backend_dependencies.extend(receipts);
    let extension_refs = HttpRouteExtensionRefs::from_route(child)
        .expect("the immutable child already passed the same owning-source validation");
    let child_ns = child.metadata.namespace.as_deref().unwrap_or("default");
    let child_name = child.metadata.name.as_deref().unwrap_or("");
    let Some(rules) = &child.spec.rules else {
        return;
    };
    let invalid_details: HashMap<usize, String> = super::http_route::find_invalid_rule_details(child, child_ns)
        .into_iter()
        .collect();

    let mut auth_decisions = super::route_auth::by_rule(child, |field| ctx.lookup.auth_decision(field));
    for (idx, rule) in rules.iter().enumerate() {
        let mut auth_rule = rule.clone();
        super::route_auth::scrub_rule(&mut auth_rule);
        for mut decision in auth_decisions.remove(&idx).unwrap_or_default() {
            ctx.auth_errors.append(&mut decision.errors);
            decision.apply(&mut auth_rule);
        }
        let rule = &auth_rule;
        let mirror_decisions = HttpRouteMirrorRefs::from_route(child)
            .map(|refs| {
                refs.authored()
                    .filter(|field| matches!(field.field().slot(), MirrorSlot::Route(slot) if slot.rule() == idx))
                    .map(|field| {
                        super::request_mirror::resolve(field, |terminal| ctx.lookup.mirror_ref_denied(terminal))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ctx.cross_ns_target_namespaces.extend(
            mirror_decisions
                .iter()
                .filter_map(|decision| decision.cross_namespace().map(str::to_string)),
        );
        if let Some(reason) = invalid_details.get(&idx) {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!("child '{child_key}' rules[{idx}] dropped: {reason}"),
            });
            continue;
        }

        if let Some(reason) = extension_refs
            .authored()
            .filter(|field| field.slot().rule() == idx)
            .find_map(|field| ctx.lookup.extension_ref_error(field, true))
        {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!("child '{child_key}' rules[{idx}] dropped: {reason}"),
            });
            continue;
        }

        let denials: Vec<Option<RefDenied>> = rule
            .backend_refs
            .iter()
            .flatten()
            .enumerate()
            .map(|(backend, _)| {
                match child_refs
                    .authored_at(idx, backend)
                    .expect("indices enumerate this borrowed child rule and backend vector")
                    .validated()
                {
                    Ok(AuthoredRouteTarget::Terminal(terminal)) => ctx.lookup.terminal_ref_denied(terminal),
                    _ => None,
                }
            })
            .collect();

        // Raw child rules have not necessarily passed their own handler yet.
        // Resolve mirrors before inheritance, using this declaring child even
        // when a terminal rule belongs to a deeper descendant.
        for decision in mirror_decisions {
            decision.apply_rule(&mut auth_rule);
        }
        let rule = &auth_rule;

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
                Ok((_dref, own_scopes)) => {
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
                                        parent_prefixes: scope
                                            .parent_prefixes
                                            .iter()
                                            .cloned()
                                            .chain(std::iter::once(own.prefix.clone()))
                                            .collect(),
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
                        child_refs.authored_at(idx, 0),
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
        let mut parent_matches = Vec::new();
        let mut drop_reason: Option<String> = None;
        'matches: for cm in &child_matches {
            let mut fitted = false;
            let mut branch_conflict = None;
            let is_regex = cm.path.as_ref().and_then(|path| path.match_type.as_deref()) == Some("RegularExpression");
            for scope in scopes {
                match merge_match(scope, cm) {
                    MergeResult::NotContained => continue,
                    MergeResult::Conflict(e) => {
                        // Regex paths intersect each parent OR branch at runtime.
                        // An incompatible conjunction must not discard another branch.
                        if is_regex {
                            branch_conflict.get_or_insert(e);
                            continue;
                        }
                        drop_reason = Some(e);
                        break 'matches;
                    }
                    MergeResult::Merged(m) => {
                        flattened_matches.push(m);
                        parent_matches.push(ParentMatch {
                            path_prefixes: scope.parent_prefixes.clone(),
                        });
                        fitted = true;
                    }
                }
            }
            if !fitted {
                drop_reason = Some(branch_conflict.unwrap_or_else(|| {
                    format!(
                        "match path {:?} is outside every delegated prefix",
                        cm.path.as_ref().and_then(|p| p.value.as_deref())
                    )
                }));
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
        flattened.resolved_parent_match = Some(ResolvedParentMatch::Delegated {
            matches: parent_matches,
        });
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
            for (backend_index, br) in backend_refs.iter_mut().enumerate() {
                for filter in br.filters.iter_mut().flatten() {
                    if filter.filter_type == HTTPRouteFilterType::ExtensionRef {
                        if let Some(extension_ref) = filter.extension_ref.as_mut() {
                            extension_ref.resolved_namespace = Some(child_ns.to_string());
                        }
                    }
                }
                if br.namespace.is_none() {
                    br.namespace = Some(child_ns.to_string());
                }
                br.ref_denied = denials.get(backend_index).cloned().flatten();
            }
        }

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
            // here and persist the normalized value into the flattened rule.
            // Exact/Prefix effective paths retain their existing Gateway pass;
            // ancestor sidecars are already BASE-normalized and are compiled as-is.
            let raw = p.value.as_deref().unwrap_or("/");
            let normalizable = is_normalizable_match_type(match_type);
            let value = if normalizable {
                normalize_path_value(raw)
            } else {
                raw.to_string()
            };
            if match_type != "RegularExpression" && !prefix_contains(&scope.prefix, match_type, &value) {
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
///
/// Regex matches instead carry the scope for runtime conjunction; textual
/// regular-expression containment cannot prove the required path boundary.
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

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/route_delegation_test.rs"]
mod tests;
