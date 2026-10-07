//! GRPCRoute delegation flattening.
//!
//! A parent GRPCRoute rule may delegate a gRPC service to child GRPCRoutes
//! (possibly in other namespaces) via `backendRef {group:
//! gateway.networking.k8s.io, kind: GRPCRoute}`. During `parse`, the
//! controller expands the delegation tree into a flat rule list stored in
//! `spec.resolved_rules`; the gateway compiles that list and never sees the
//! nesting.
//!
//! This mirrors `route_delegation.rs` (HTTPRoute delegation) structurally
//! (DFS shape, `FlattenCtx`, issue recording), but the "scope" being
//! delegated is a gRPC service name compared by spec-string equality, not a
//! path prefix compared by containment. Canonical semantics:
//! `docs/en/user-guide/grpc-route/delegation.md`; controller architecture:
//! `skills/01-architecture/05-resources/05-grpc-route.md`.
//!
//! Security-critical divergences from a literal HTTP mirror (regressions here
//! are routing-boundary escapes):
//! 1. A delegating rule with `matches` absent/empty is INVALID (HTTP treats a
//!    matchless delegating rule as delegating `"/"` — that default is NOT
//!    mirrored here, since delegate-all is out of scope for v1).
//! 2. A terminal child rule with `matches` absent/empty flattens to ONE
//!    inherited match PER delegated scope — never cloned matchless (the
//!    gateway synthesizes a catch-all unit for a matchless rule; a cloned
//!    matchless rule would match ALL traffic on the parent's hostnames).
//! 3. Any valid child match whose `method.service` is absent (matcher
//!    missing or method-only) inherits the delegated
//!    service as `{type: Exact, service: S}`.
//! 4. Any child match with `method.type: RegularExpression` invalidates that
//!    whole child rule (v1 rejects the match outright rather than proving a
//!    regex service cannot escape the scope).

use super::route_backend_refs::{self, BackendDependencyReceipt};
use edgion_resources::resources::refs::route_fields::{
    AuthoredBackendReference, AuthoredRouteTarget, GrpcRouteBackendRefs, TerminalBackendReference,
};

use std::collections::HashSet;
use std::sync::Arc;

use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::grpc_route::{
    GRPCBackendRef, GRPCHeaderMatch, GRPCMethodMatch, GRPCMethodMatchType, GRPCRoute, GRPCRouteFilter,
    GRPCRouteFilterType, GRPCRouteMatch, GRPCRouteRetry, GRPCRouteRule, GRPCRouteTimeouts,
};
use edgion_resources::resources::http_route::{DelegationIssue, SessionPersistence, GATEWAY_API_GROUP};

use super::route_delegation::MAX_DELEGATION_DEPTH;

/// Store abstraction so flattening is unit-testable without a ControllerRuntime.
pub trait DelegationLookup {
    /// Look up a GRPCRoute by `"ns/name"` key.
    fn get_route(&self, key: &str) -> Option<Arc<GRPCRoute>>;
    /// All GRPCRoute keys in a namespace, sorted (for deterministic wildcard
    /// expansion order).
    fn list_route_keys_in_namespace(&self, ns: &str) -> Vec<String>;
    /// Authorize only the original declaring Route's validated terminal field.
    fn terminal_ref_denied(&self, terminal: TerminalBackendReference<'_>) -> Option<RefDenied>;
}

/// Result of flattening one parent route.
pub struct FlattenOutcome {
    /// `None` for a valid source without delegation (Gateway uses `spec.rules`).
    /// Invalid sources return an empty effective rule set.
    pub resolved_rules: Option<Vec<GRPCRouteRule>>,
    pub issues: Vec<DelegationIssue>,
    /// Every named child key referenced anywhere in the delegation tree
    /// (including missing ones, so their later creation requeues this route).
    pub named_children: HashSet<String>,
    /// Every wildcard-delegated namespace anywhere in the delegation tree.
    pub wildcard_namespaces: HashSet<String>,
    /// Valid authored terminal dependencies, retained even when a rule is dropped.
    pub(super) backend_dependencies: Vec<BackendDependencyReceipt>,
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
            backend_dependencies: Vec::new(),
            cross_ns_target_namespaces: HashSet::new(),
            expanded_children: HashSet::new(),
        }
    }
}

/// A delegated match scope handed down one delegation edge: the gRPC service
/// name plus the mandatory header constraints accumulated from ancestor
/// matches.
#[derive(Clone, Debug)]
struct DelegatedScope {
    service: String,
    headers: Vec<GRPCHeaderMatch>,
}

/// Rule-level attributes inherited down the chain (child-first-else-inherited).
#[derive(Clone, Default)]
struct InheritedAttrs {
    timeouts: Option<GRPCRouteTimeouts>,
    retry: Option<GRPCRouteRetry>,
    session_persistence: Option<SessionPersistence>,
}

/// Filters inherited down the chain. Standard filters are mandatory and
/// conflict with same-type child filters; ExtensionRef filters concatenate.
#[derive(Clone, Default)]
struct InheritedFilters {
    standard: Vec<GRPCRouteFilter>,
    extension: Vec<GRPCRouteFilter>,
}

impl InheritedFilters {
    fn standard_types(&self) -> Vec<GRPCRouteFilterType> {
        self.standard.iter().map(|f| f.filter_type.clone()).collect()
    }
}

struct FlattenCtx<'a> {
    lookup: &'a dyn DelegationLookup,
    issues: Vec<DelegationIssue>,
    named_children: HashSet<String>,
    wildcard_namespaces: HashSet<String>,
    backend_dependencies: Vec<BackendDependencyReceipt>,
    cross_ns_target_namespaces: HashSet<String>,
    expanded_children: HashSet<String>,
    /// Route keys on the current DFS path (root first), for cycle detection.
    path: Vec<String>,
}

/// Flatten the delegation tree rooted at `parent`.
///
/// Pure over `lookup`; does not touch any runtime index (the caller registers
/// `named_children` / `wildcard_namespaces` / `backend_dependencies`).
pub fn flatten_route(parent: &GRPCRoute, lookup: &dyn DelegationLookup) -> FlattenOutcome {
    let parent_refs = match GrpcRouteBackendRefs::from_route(parent) {
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
            .is_some_and(|brs| brs.iter().any(GRPCBackendRef::is_route_delegation_ref))
    });
    if !has_delegation {
        return FlattenOutcome::no_delegation();
    }

    let mut ctx = FlattenCtx {
        lookup,
        issues: Vec::new(),
        named_children: HashSet::new(),
        wildcard_namespaces: HashSet::new(),
        backend_dependencies: route_backend_refs::collect_dependencies(parent_refs.authored()),
        cross_ns_target_namespaces: HashSet::new(),
        expanded_children: HashSet::new(),
        path: vec![parent_key],
    };

    // Rules already invalid per the standing per-rule checks are dropped here;
    // the gateway does not apply `invalid_rule_indices` to resolved_rules.
    let invalid_indices: HashSet<usize> = super::grpc_route::find_invalid_rule_indices(parent, parent_ns)
        .into_iter()
        .collect();

    let mut out_rules: Vec<GRPCRouteRule> = Vec::new();
    for (idx, rule) in rules.iter().enumerate() {
        if invalid_indices.contains(&idx) {
            if let Some(message) = rule.method_match_error() {
                ctx.issues.push(DelegationIssue::RuleDropped {
                    message: format!("rules[{idx}]: {message}"),
                });
            }
            continue;
        }
        let is_delegating = rule
            .backend_refs
            .as_ref()
            .is_some_and(|brs| brs.iter().any(GRPCBackendRef::is_route_delegation_ref));
        if !is_delegating {
            let copied = rule.clone();

            out_rules.push(copied);
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
                        .filter(|f| f.filter_type != GRPCRouteFilterType::ExtensionRef)
                        .cloned()
                        .collect(),
                    extension: rule
                        .filters
                        .iter()
                        .flatten()
                        .filter(|f| f.filter_type == GRPCRouteFilterType::ExtensionRef)
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
        named_children: ctx.named_children,
        wildcard_namespaces: ctx.wildcard_namespaces,
        backend_dependencies: ctx.backend_dependencies,
        cross_ns_target_namespaces: ctx.cross_ns_target_namespaces,
        expanded_children: ctx.expanded_children,
    }
}

/// Validate a delegating rule's shape and derive its delegated scopes.
///
/// Constraints (spec §"What makes a rule delegating"): exactly one
/// backendRef; `port` and per-backendRef `filters` absent; `matches` MUST be
/// present and non-empty (deliberate divergence from HTTP's `"/"` default —
/// delegate-all is out of scope for v1); every match must carry a `method`
/// matcher with `type` Exact-or-omitted, non-empty `service`, and no
/// `method`.
fn validate_delegating_rule(
    rule: &GRPCRouteRule,
    rule_idx: usize,
) -> Result<(&GRPCBackendRef, Vec<DelegatedScope>), String> {
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

    let matches = match rule.matches.as_ref() {
        None => {
            return Err(format!(
                "rules[{rule_idx}]: a delegating rule must set at least one match (delegate-all is not supported)"
            ))
        }
        Some(m) if m.is_empty() => {
            return Err(format!(
                "rules[{rule_idx}]: a delegating rule must set at least one match (delegate-all is not supported)"
            ))
        }
        Some(m) => m,
    };

    let mut scopes = Vec::with_capacity(matches.len());
    for m in matches {
        let Some(method) = m.method.as_ref() else {
            return Err(format!(
                "rules[{rule_idx}]: delegating rule matches must set a method matcher with 'service' (found a header-only match)"
            ));
        };
        if !matches!(method.match_type, None | Some(GRPCMethodMatchType::Exact)) {
            return Err(format!(
                "rules[{rule_idx}]: delegating rule matches must use Exact (or omitted) method matching (found {:?})",
                method.match_type
            ));
        }
        let Some(service) = method.service.as_deref().filter(|s| !s.is_empty()) else {
            return Err(format!(
                "rules[{rule_idx}]: delegating rule matches must set a non-empty 'method.service'"
            ));
        };
        if method.method.is_some() {
            return Err(format!(
                "rules[{rule_idx}]: delegating rule matches must not set 'method.method' (service-level delegation only)"
            ));
        }
        scopes.push(DelegatedScope {
            service: service.to_string(),
            headers: m.headers.clone().unwrap_or_default(),
        });
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
    out_rules: &mut Vec<GRPCRouteRule>,
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

        if let Err(error) = GrpcRouteBackendRefs::from_route(&child) {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!(
                    "child '{child_key}' dropped: {}",
                    route_backend_refs::source_error_message(error)
                ),
            });
            continue;
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
    child: &GRPCRoute,
    child_key: &str,
    scopes: &[DelegatedScope],
    inherited: &InheritedFilters,
    attrs: &InheritedAttrs,
    depth: usize,
    out_rules: &mut Vec<GRPCRouteRule>,
) {
    let Ok(child_refs) = GrpcRouteBackendRefs::from_route(child) else {
        return;
    };
    let receipts = route_backend_refs::collect_dependencies(child_refs.authored());
    ctx.cross_ns_target_namespaces.extend(
        receipts
            .iter()
            .filter_map(|receipt| receipt.cross_namespace().map(str::to_string)),
    );
    ctx.backend_dependencies.extend(receipts);
    let child_ns = child.metadata.namespace.as_deref().unwrap_or("default");
    let child_name = child.metadata.name.as_deref().unwrap_or("");
    let Some(rules) = &child.spec.rules else {
        return;
    };
    let invalid_indices: HashSet<usize> = super::grpc_route::find_invalid_rule_indices(child, child_ns)
        .into_iter()
        .collect();

    for (idx, rule) in rules.iter().enumerate() {
        if invalid_indices.contains(&idx) {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!("child '{child_key}' rules[{idx}] dropped: invalid rule (see child status)"),
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

        let is_delegating = rule
            .backend_refs
            .as_ref()
            .is_some_and(|brs| brs.iter().any(GRPCBackendRef::is_route_delegation_ref));

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
                f.filter_type != GRPCRouteFilterType::ExtensionRef && inherited_std_types.contains(&f.filter_type)
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
            // Nested delegation: the delegating rule's own scopes must fit
            // (string-equal) an incoming scope; the merged result becomes the
            // new scopes for the recursive expansion.
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
                            if scope.service != own.service {
                                continue;
                            }
                            match merge_headers(scope, &own.headers) {
                                Ok(headers) => {
                                    merged_scopes.push(DelegatedScope {
                                        service: own.service.clone(),
                                        headers,
                                    });
                                    fitted = true;
                                }
                                Err(e) => fit_error = Some(e),
                            }
                        }
                        if !fitted {
                            fit_error.get_or_insert_with(|| {
                                format!("service '{}' is outside every delegated scope", own.service)
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
                                    .filter(|f| f.filter_type != GRPCRouteFilterType::ExtensionRef),
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
                                    .filter(|f| f.filter_type == GRPCRouteFilterType::ExtensionRef)
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
        let matches_present = rule.matches.as_ref().filter(|m| !m.is_empty());
        let mut flattened_matches: Vec<GRPCRouteMatch> = Vec::new();
        let mut drop_reason: Option<String> = None;

        match matches_present {
            None => {
                // Matchless: one inherited match PER delegated scope. Never
                // clone a matchless rule — the gateway synthesizes a
                // catch-all unit for a matchless rule, so a cloned matchless
                // rule in resolved_rules would match ALL traffic.
                for scope in scopes {
                    flattened_matches.push(GRPCRouteMatch {
                        method: Some(GRPCMethodMatch {
                            match_type: Some(GRPCMethodMatchType::Exact),
                            service: Some(scope.service.clone()),
                            method: None,
                        }),
                        headers: if scope.headers.is_empty() {
                            None
                        } else {
                            Some(scope.headers.clone())
                        },
                    });
                }
            }
            Some(child_matches) => {
                'matches: for cm in child_matches {
                    match merge_match(scopes, cm) {
                        MergeResult::NotContained => {
                            drop_reason = Some("match service is outside every delegated scope".to_string());
                            break 'matches;
                        }
                        MergeResult::Conflict(e) => {
                            drop_reason = Some(e);
                            break 'matches;
                        }
                        MergeResult::Merged(mut ms) => flattened_matches.append(&mut ms),
                    }
                }
            }
        }

        if let Some(reason) = drop_reason {
            ctx.issues.push(DelegationIssue::RuleDropped {
                message: format!("child '{child_key}' rules[{idx}] dropped: {reason}"),
            });
            continue;
        }

        let mut flattened = rule.clone();
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
            let combined: Vec<GRPCRouteFilter> = inherited
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

        // Child backendRefs keep child-local semantics on the parent: make
        // the namespace explicit and run the ReferenceGrant check with the
        // CHILD namespace as referrer.
        if let Some(backend_refs) = flattened.backend_refs.as_mut() {
            for (backend_index, br) in backend_refs.iter_mut().enumerate() {
                for filter in br.filters.iter_mut().flatten() {
                    if filter.filter_type == GRPCRouteFilterType::ExtensionRef {
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

        out_rules.push(flattened);
    }
}

fn with_extension_ref_namespace(mut filter: GRPCRouteFilter, namespace: &str) -> GRPCRouteFilter {
    if filter.filter_type == GRPCRouteFilterType::ExtensionRef {
        if let Some(extension_ref) = filter.extension_ref.as_mut() {
            extension_ref.resolved_namespace = Some(namespace.to_string());
        }
    }
    filter
}

enum MergeResult {
    /// None of the delegated scopes admit this match.
    NotContained,
    /// Fits a scope, but the mandatory header constraints contradict it, or
    /// the match uses an unsupported (RegularExpression) method type.
    Conflict(String),
    /// One flattened match per scope this child match expands against
    /// (service-inheriting matches expand once per scope; an explicit
    /// service only expands against the scope(s) it equals).
    Merged(Vec<GRPCRouteMatch>),
}

/// Merge one child match against every delegated scope.
///
/// - `method.type: RegularExpression` invalidates the match's owning rule
///   outright (v1 rejects the whole match rather than proving a regex
///   service cannot escape the delegated scope).
/// - A non-empty `method.service` must string-equal a scope's service
///   (spec-string equality — the gateway compiles service/method values
///   verbatim, so there is no HTTP-style normalization gap); the match
///   expands against every scope it equals.
/// - An absent `method.service` (matcher missing or valid method-only)
///   inherits the scope's service and expands once per scope. Explicit empty
///   matchers and invalid Exact names are rejected by shared rule validation.
fn merge_match(scopes: &[DelegatedScope], child_match: &GRPCRouteMatch) -> MergeResult {
    if child_match.method.as_ref().and_then(|m| m.match_type.as_ref()) == Some(&GRPCMethodMatchType::RegularExpression)
    {
        return MergeResult::Conflict("RegularExpression method matches cannot be delegated".to_string());
    }

    let explicit_service = child_match.method.as_ref().and_then(|m| m.service.as_deref());
    let child_method_name = child_match.method.as_ref().and_then(|m| m.method.clone());

    let mut merged = Vec::new();
    for scope in scopes {
        if let Some(svc) = explicit_service {
            if scope.service != svc {
                continue;
            }
        }
        match merge_headers(scope, child_match.headers.as_deref().unwrap_or(&[])) {
            Ok(headers) => merged.push(GRPCRouteMatch {
                method: Some(GRPCMethodMatch {
                    match_type: Some(GRPCMethodMatchType::Exact),
                    service: Some(scope.service.clone()),
                    method: child_method_name.clone(),
                }),
                headers: if headers.is_empty() { None } else { Some(headers) },
            }),
            Err(e) => return MergeResult::Conflict(e),
        }
    }

    if merged.is_empty() {
        MergeResult::NotContained
    } else {
        MergeResult::Merged(merged)
    }
}

/// Merge the scope's mandatory headers with a child's. Same name (compared
/// case-insensitively) with a different value/type is a conflict; an
/// identical entry is deduped.
fn merge_headers(scope: &DelegatedScope, child_headers: &[GRPCHeaderMatch]) -> Result<Vec<GRPCHeaderMatch>, String> {
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
    Ok(headers)
}

/// True if the child either declares no GRPCRoute-kind parentRefs (accepts
/// any delegating parent) or lists `parent_key` among them.
fn reverse_lock_allows(child: &GRPCRoute, parent_key: &str) -> bool {
    let child_ns = child.metadata.namespace.as_deref().unwrap_or("default");
    let mut has_lock = false;
    for pr in child.spec.parent_refs.iter().flatten() {
        if pr.kind.as_deref() != Some("GRPCRoute")
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

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route_delegation_test.rs"]
mod tests;
