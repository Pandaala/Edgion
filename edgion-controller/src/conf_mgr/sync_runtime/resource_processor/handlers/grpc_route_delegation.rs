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

use std::collections::HashSet;
use std::sync::Arc;

use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::grpc_route::{
    GRPCBackendRef, GRPCHeaderMatch, GRPCMethodMatch, GRPCMethodMatchType, GRPCRoute, GRPCRouteFilter,
    GRPCRouteFilterType, GRPCRouteMatch, GRPCRouteRetry, GRPCRouteRule, GRPCRouteTimeouts,
};
use edgion_resources::resources::http_route::{DelegationIssue, SessionPersistence, GATEWAY_API_GROUP};

use super::route_delegation::MAX_DELEGATION_DEPTH;

/// `(group, kind, namespace(explicit), name)` of a Service backendRef.
pub type ServiceBackendTuple = (Option<String>, Option<String>, Option<String>, String);

/// Store abstraction so flattening is unit-testable without a ControllerRuntime.
pub trait DelegationLookup {
    /// Look up a GRPCRoute by `"ns/name"` key.
    fn get_route(&self, key: &str) -> Option<Arc<GRPCRoute>>;
    /// All GRPCRoute keys in a namespace, sorted (for deterministic wildcard
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
    pub resolved_rules: Option<Vec<GRPCRouteRule>>,
    pub issues: Vec<DelegationIssue>,
    /// Every named child key referenced anywhere in the delegation tree
    /// (including missing ones, so their later creation requeues this route).
    pub named_children: HashSet<String>,
    /// Every wildcard-delegated namespace anywhere in the delegation tree.
    pub wildcard_namespaces: HashSet<String>,
    /// `(group, kind, namespace(explicit), name)` of every Service backendRef
    /// in `resolved_rules`.
    pub service_backend_tuples: Vec<ServiceBackendTuple>,
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
    service_backend_tuples: Vec<ServiceBackendTuple>,
    cross_ns_target_namespaces: HashSet<String>,
    expanded_children: HashSet<String>,
    /// Route keys on the current DFS path (root first), for cycle detection.
    path: Vec<String>,
}

/// Flatten the delegation tree rooted at `parent`.
///
/// Pure over `lookup`; does not touch any runtime index (the caller registers
/// `named_children` / `wildcard_namespaces` / `service_backend_tuples`).
pub fn flatten_route(parent: &GRPCRoute, lookup: &dyn DelegationLookup) -> FlattenOutcome {
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
        service_backend_tuples: Vec::new(),
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
            collect_service_tuples(&mut ctx, parent_ns, &copied);
            out_rules.push(copied);
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
    referrer_ns: &str,
    dref: &GRPCBackendRef,
    scopes: &[DelegatedScope],
    inherited: &InheritedFilters,
    attrs: &InheritedAttrs,
    depth: usize,
    out_rules: &mut Vec<GRPCRouteRule>,
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
    child: &GRPCRoute,
    child_key: &str,
    scopes: &[DelegatedScope],
    inherited: &InheritedFilters,
    attrs: &InheritedAttrs,
    depth: usize,
    out_rules: &mut Vec<GRPCRouteRule>,
) {
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
                Ok((dref, own_scopes)) => {
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
            for br in backend_refs.iter_mut() {
                for filter in br.filters.iter_mut().flatten() {
                    if filter.filter_type == GRPCRouteFilterType::ExtensionRef {
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
        collect_service_tuples(ctx, child_ns, &flattened);
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

/// Record Service backendRef tuples of a flattened/copied rule for
/// ServiceRefManager registration under the parent route.
fn collect_service_tuples(ctx: &mut FlattenCtx, implicit_ns: &str, rule: &GRPCRouteRule) {
    for br in rule.backend_refs.iter().flatten() {
        if br.is_route_delegation_ref() {
            continue;
        }
        let group = br.group.as_deref().unwrap_or("");
        if !group.is_empty() && group != "core" {
            continue;
        }
        if br.kind.as_deref().unwrap_or("Service") != "Service" {
            continue;
        }
        ctx.service_backend_tuples.push((
            br.group.clone(),
            br.kind.clone(),
            Some(br.namespace.clone().unwrap_or_else(|| implicit_ns.to_string())),
            br.name.clone(),
        ));
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
        fn get_route(&self, key: &str) -> Option<Arc<GRPCRoute>> {
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
            .service_backend_tuples
            .iter()
            .any(|(_, _, ns, name)| ns.as_deref() == Some("billing") && name == "svc-local"));
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
}
