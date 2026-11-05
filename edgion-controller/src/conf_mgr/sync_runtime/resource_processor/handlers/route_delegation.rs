//! HTTPRoute/GRPCRoute delegation validation, key publication and status.
//!
//! A parent HTTPRoute rule delegates its path prefix to child HTTPRoutes via a
//! single `backendRef {group: gateway.networking.k8s.io, kind: HTTPRoute}`
//! (named, or `name: "*"` for every declaring child in the target namespace).
//! The Controller no longer flattens delegation trees: each route publishes
//! its own rules, and each child carries the controller-resolved
//! `resolved_delegation_keys` of the edges that passed bidirectional
//! confirmation (the parent delegates to the child AND the child lists the
//! parent in its route-kind parentRefs). The edges and levels live in the
//! participants-only [`DelegationGraph`].
//!
//! GRPCRoute shares the graph sync and status helpers here; its rule-shape
//! facts live in `grpc_route_delegation.rs`.
//!
//! Design: `tasks/todo/route-delegation-cascading-match-design.md` §4.2, §5, §7.

use std::collections::{BTreeSet, HashSet};

use super::super::delegation_graph::{DelegatingRule, DelegationGraph};
use super::route_utils::plugin_target;
use crate::conf_mgr::sync_runtime::resource_processor::{condition_reasons, AcceptedError, ResolvedRefsError};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::path_normalization::normalize::canonicalize_path_prefix;
use edgion_resources::path_normalization::{normalize_config_path, PathNormalizationConfig};
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::edgion_plugins::edgion_plugin::is_upstream_jump_plugin_type;
use edgion_resources::resources::edgion_plugins::EdgionPlugins;
use edgion_resources::resources::http_route::{HTTPRoute, HTTPRouteRule, GATEWAY_API_GROUP};
use edgion_resources::resources::refs::plugin_target::PluginTarget;
use edgion_resources::resources::refs::route_fields::HttpRouteExtensionRefs;
use edgion_resources::resources::route_delegation::{
    classify_parent_refs, delegation_target_of_http_rule, standard_filter_mask_http, DelegationKey, DelegationTarget,
    DelegationTargetName, ParentRefShape, RouteKindName, MAX_ROUTE_DELEGATION_LEVELS, MAX_ROUTE_KIND_PARENT_REFS,
};
use std::sync::Arc;

/// Status reasons of delegation (design §7). `BackendNotFound`,
/// `RefNotPermitted`, `UnsupportedValue` and `PartiallyInvalid` reuse the
/// shared Gateway API constants.
pub const DELEGATION_ACCEPTED: &str = "DelegationAccepted";
pub const NO_MATCHING_PARENT: &str = condition_reasons::NO_MATCHING_PARENT;
pub const NOT_ATTACHED: &str = "NotAttached";
pub const DEPTH_EXCEEDED: &str = "DepthExceeded";
pub const INVALID_PARENT_REFS: &str = "InvalidParentRefs";

/// True for request plugins that select the upstream themselves: the shared
/// upstream-jump classifier plus `AllEndpointStatus` (which answers from the
/// rule's own backends) and `AiProxy` (which dispatches to the rule's own AI
/// backends). They are valid only on terminal rules; on a
/// delegating rule they would pick a backend for a rule that has none.
pub(super) fn is_backend_selecting_plugin(type_name: &str) -> bool {
    is_upstream_jump_plugin_type(type_name) || matches!(type_name, "AllEndpointStatus" | "AiProxy")
}

/// Upper bound of keys on one child: at most 3 parents, each with at most a
/// named and a wildcard target covering the child.
const MAX_CHILD_DELEGATION_KEYS: usize = MAX_ROUTE_KIND_PARENT_REFS * 2;

/// Delegation facts of one route (HTTPRoute or GRPCRoute), derived from its
/// own spec.
pub(super) struct DelegationFacts {
    pub shape: ParentRefShape,
    /// True for an invalid parentRef shape or a child that sets hostnames:
    /// the route publishes no targets and no keys (§2.1, §2.6).
    pub inert: bool,
    /// Distinct targets of the route's valid delegating rules. Empty for an
    /// inert route.
    pub targets: Vec<DelegationTarget>,
    /// Delegating rules per target (aligned with `targets`): the standard
    /// filter mask and delegated scope of each valid rule writing the target.
    pub target_rules: Vec<Vec<DelegatingRule>>,
    /// `(rule index, reason)` for delegating rules with an illegal shape.
    pub malformed_rules: Vec<(usize, String)>,
    /// `(rule index, reason)` for delegating rules that attach a
    /// backend-selecting plugin.
    pub backend_plugin_on_delegating_rule: Vec<(usize, String)>,
    /// `ns/name` of the same-kind route parentRefs, for a child or an
    /// invalid-shape route (bounded by the parentRef count). Recorded in the
    /// graph so a parent never blames such a route for `RefNotPermitted`.
    pub declared_parents: Vec<String>,
}

/// HTTPRoute delegation facts.
pub(super) type HttpDelegationFacts = DelegationFacts;
/// GRPCRoute delegation facts (same fields as HTTP).
pub(super) type GrpcDelegationFacts = DelegationFacts;

impl DelegationFacts {
    pub(super) fn new(
        refs: Option<&[ParentReference]>,
        route_ns: &str,
        kind: RouteKindName,
        hostnames: Option<&Vec<String>>,
    ) -> Self {
        let shape = classify_parent_refs(refs, route_ns, kind);
        let inert = is_inert(&shape, hostnames);
        let declared_parents = match &shape {
            ParentRefShape::Child(parents) => parents.iter().map(|(ns, name)| format!("{ns}/{name}")).collect(),
            ParentRefShape::Invalid(_) => refs
                .into_iter()
                .flatten()
                .filter(|p| {
                    p.group.as_deref().unwrap_or(GATEWAY_API_GROUP) == GATEWAY_API_GROUP
                        && p.kind.as_deref() == Some(kind.as_str())
                })
                .map(|p| format!("{}/{}", p.namespace.as_deref().unwrap_or(route_ns), p.name))
                .collect(),
            ParentRefShape::None | ParentRefShape::Root => Vec::new(),
        };
        Self {
            declared_parents,
            shape,
            inert,
            targets: Vec::new(),
            target_rules: Vec::new(),
            malformed_rules: Vec::new(),
            backend_plugin_on_delegating_rule: Vec::new(),
        }
    }

    /// Record a valid delegating rule under its target.
    pub(super) fn add_target(&mut self, target: DelegationTarget, rule: DelegatingRule) {
        match self.targets.iter().position(|t| *t == target) {
            Some(index) => self.target_rules[index].push(rule),
            None => {
                self.targets.push(target);
                self.target_rules.push(vec![rule]);
            }
        }
    }

    /// Every invalid delegating rule, for the invalid-rule path.
    pub(super) fn invalid_rules(&self) -> impl Iterator<Item = &(usize, String)> {
        self.malformed_rules
            .iter()
            .chain(self.backend_plugin_on_delegating_rule.iter())
    }

    /// Routes that take part in delegation and therefore enter the graph.
    fn is_participant(&self) -> bool {
        !self.targets.is_empty() || matches!(self.shape, ParentRefShape::Child(_) | ParentRefShape::Invalid(_))
    }
}

/// True for a route whose shape makes it inert (§2.1, §2.6).
fn is_inert(shape: &ParentRefShape, hostnames: Option<&Vec<String>>) -> bool {
    match shape {
        ParentRefShape::Invalid(_) => true,
        ParentRefShape::Child(_) => hostnames.is_some_and(|h| !h.is_empty()),
        ParentRefShape::None | ParentRefShape::Root => false,
    }
}

/// Compute the route's delegation facts. An ordinary route (no delegating
/// rule, no route-kind parentRef) costs one parentRef classification and one
/// rule scan, and never reads the runtime.
pub(super) fn http_delegation_facts(route: &HTTPRoute, runtime: &ControllerRuntime) -> HttpDelegationFacts {
    let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
    let mut facts = HttpDelegationFacts::new(
        route.spec.parent_refs.as_deref(),
        route_ns,
        RouteKindName::HTTPRoute,
        route.spec.hostnames.as_ref(),
    );
    let inert = facts.inert;
    let Some(rules) = route.spec.rules.as_ref() else {
        return facts;
    };

    let mut statically_invalid: Option<HashSet<usize>> = None;
    for (index, rule) in rules.iter().enumerate() {
        let target = match delegation_target_of_http_rule(rule, route_ns) {
            None => continue,
            Some(Err(reason)) => {
                facts.malformed_rules.push((index, reason));
                continue;
            }
            Some(Ok(target)) => target,
        };
        let backend = &rule.backend_refs.as_ref().expect("a delegating rule has a backendRef")[0];
        if let Err(error) = backend.validated_target(route_ns) {
            facts
                .malformed_rules
                .push((index, format!("Invalid delegation reference: {error}")));
            continue;
        }
        if let Some(plugin) = backend_selecting_plugin(route, index, runtime) {
            facts.backend_plugin_on_delegating_rule.push((
                index,
                format!("{plugin} selects the upstream and is valid only on terminal rules, not on an HTTPRoute delegation rule"),
            ));
            continue;
        }
        let invalid = statically_invalid.get_or_insert_with(|| {
            super::http_route::find_invalid_rule_details(route, route_ns)
                .into_iter()
                .map(|(index, _)| index)
                .collect()
        });
        if inert || invalid.contains(&index) {
            continue;
        }
        facts.add_target(target, http_delegating_rule(rule));
    }
    facts
}

// =====================================================================
// Static scope check (child rules against the delegated scope)
// =====================================================================

/// One match of a child rule, as far as it can be compared statically with
/// a parent's delegated scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ChildMatch {
    /// Cannot be decided statically (regular expression, `:param` segment,
    /// no path or service): assumed to be reachable.
    Unchecked,
    /// Canonical HTTP `Exact` path.
    HttpExact(String),
    /// Canonical HTTP `PathPrefix` path.
    HttpPrefix(String),
    /// gRPC `Exact` method match naming this service.
    GrpcService(String),
}

/// Statically known facts of one child rule: its standard filter mask and
/// its matches (empty = the rule matches everything in scope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChildRuleFacts {
    pub mask: u32,
    pub matches: Vec<ChildMatch>,
}

/// Canonical form of an HTTP `Exact` / `PathPrefix` value, matching the
/// Gateway's build-time path canonicalization (consecutive slashes merged,
/// dot segments removed, a non-root prefix loses its trailing slash). `None`
/// when the value cannot be compared statically: a `:param` segment, or a
/// `%` / `;` sequence whose meaning depends on the Gateway's runtime
/// normalization configuration.
fn canonical_http_path(value: &str, prefix: bool) -> Option<String> {
    if value.is_empty() || !value.starts_with('/') || value.contains("/:") || value.contains(['%', ';']) {
        return None;
    }
    let mut path = normalize_config_path(value, &PathNormalizationConfig::default());
    if prefix {
        canonicalize_path_prefix(&mut path);
    }
    Some(path)
}

/// True when the canonical `prefix` matches the canonical `path` on a
/// segment boundary, as the Gateway's prefix matcher does: `/a/b` covers
/// `/a/b` and `/a/b/x` but not `/a/bx`.
pub(super) fn prefix_covers(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Delegating-rule facts of a valid HTTP delegating rule. A match without a
/// path delegates `PathPrefix /`, as does a rule without matches.
fn http_delegating_rule(rule: &HTTPRouteRule) -> DelegatingRule {
    let mask = standard_filter_mask_http(rule);
    let matches = rule.matches.as_deref().unwrap_or_default();
    if matches.is_empty() {
        return DelegatingRule::unscoped(mask);
    }
    let mut scopes = Vec::with_capacity(matches.len());
    for m in matches {
        let value = m.path.as_ref().and_then(|p| p.value.as_deref()).unwrap_or("/");
        match canonical_http_path(value, true) {
            Some(prefix) if prefix != "/" => scopes.push(prefix),
            _ => return DelegatingRule::unscoped(mask),
        }
    }
    DelegatingRule {
        mask,
        scopes: Some(scopes),
    }
}

/// Static facts of every HTTP child rule, in rule order.
pub(super) fn http_child_rules(rules: Option<&Vec<HTTPRouteRule>>) -> Vec<ChildRuleFacts> {
    rules
        .into_iter()
        .flatten()
        .map(|rule| ChildRuleFacts {
            mask: standard_filter_mask_http(rule),
            matches: rule
                .matches
                .iter()
                .flatten()
                .map(|m| {
                    let Some(path) = m.path.as_ref() else {
                        return ChildMatch::Unchecked;
                    };
                    let value = path.value.as_deref().unwrap_or("/");
                    match path.match_type.as_deref().unwrap_or("PathPrefix") {
                        "Exact" => {
                            canonical_http_path(value, false).map_or(ChildMatch::Unchecked, ChildMatch::HttpExact)
                        }
                        "PathPrefix" => {
                            canonical_http_path(value, true).map_or(ChildMatch::Unchecked, ChildMatch::HttpPrefix)
                        }
                        _ => ChildMatch::Unchecked,
                    }
                })
                .collect(),
        })
        .collect()
}

/// True when some request can match `child` after entering `parent_rule`.
fn rule_reaches(parent_rule: &DelegatingRule, child: &ChildRuleFacts) -> bool {
    let Some(scopes) = parent_rule.scopes.as_ref() else {
        return true;
    };
    child.matches.is_empty()
        || child.matches.iter().any(|m| match m {
            ChildMatch::Unchecked => true,
            ChildMatch::HttpExact(path) => scopes.iter().any(|prefix| prefix_covers(prefix, path)),
            ChildMatch::HttpPrefix(path) => scopes
                .iter()
                .any(|prefix| prefix_covers(prefix, path) || prefix_covers(path, prefix)),
            ChildMatch::GrpcService(service) => scopes.iter().any(|s| s == service),
        })
}

/// First enabled backend-selecting request plugin referenced by a rule-level
/// EdgionPlugins ExtensionRef of rule `index`.
fn backend_selecting_plugin(route: &HTTPRoute, index: usize, runtime: &ControllerRuntime) -> Option<&'static str> {
    let references = HttpRouteExtensionRefs::from_route(route).ok()?;
    references
        .authored()
        .filter(|field| field.slot().rule() == index && field.slot().backend().is_none())
        .find_map(|field| {
            let reference = field.validated().ok()??;
            let target = reference.target();
            let key = format!("{}/{}", target.scope().namespace()?, target.name());
            backend_selecting_plugin_of(runtime, &key)
        })
}

/// First enabled backend-selecting request plugin of the EdgionPlugins `key`.
pub(super) fn backend_selecting_plugin_of(runtime: &ControllerRuntime, key: &str) -> Option<&'static str> {
    let PluginTarget::Unique { source: plugins, .. } = plugin_target(runtime, key) else {
        return None;
    };
    first_backend_selecting(&plugins)
}

fn first_backend_selecting(plugins: &Arc<EdgionPlugins>) -> Option<&'static str> {
    plugins
        .spec
        .request_plugins
        .iter()
        .flatten()
        .filter(|entry| entry.is_enabled())
        .map(|entry| entry.plugin.type_name())
        .find(|name| is_backend_selecting_plugin(name))
}

/// Apply a route's facts to the graph and return the route keys to requeue.
///
/// `None` means the route is not (and was not) a participant: the only graph
/// work was a single `is_participant` probe. Requeue rules (§5.2):
/// children of added/removed targets, declared parents naming the route when
/// its declared set changes, and the route's confirmed children when its
/// level or blocked flag changes. The route itself is never requeued.
pub(super) fn sync_http_delegation_graph(
    graph: &DelegationGraph,
    route: &HTTPRoute,
    facts: &HttpDelegationFacts,
    gateway_attached: bool,
    own_rejected: impl FnOnce() -> bool,
) -> Option<Vec<String>> {
    let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
    let route_name = route.metadata.name.as_deref().unwrap_or("");
    sync_delegation_graph(graph, route_ns, route_name, facts, gateway_attached, own_rejected)
}

/// Kind-independent body of the graph sync (HTTPRoute and GRPCRoute).
///
/// Level semantics: a route is level 1 (a root) when it is a participant
/// with `shape == Root` and at least one Gateway parentRef. Gateway
/// acceptance by a listener is deliberately not required, so `NotAttached`
/// means "no path within 5 levels to a route with Gateway parentRefs".
///
/// `own_rejected` reports whether the route fails its own validation (every
/// one of its entries is `Accepted=False` for a reason of its own). Such a
/// route keeps its edges but gets no level, at any depth, so its
/// descendants become `NotAttached` through the level-change requeue. It is
/// evaluated only for participants.
pub(super) fn sync_delegation_graph(
    graph: &DelegationGraph,
    route_ns: &str,
    route_name: &str,
    facts: &DelegationFacts,
    gateway_attached: bool,
    own_rejected: impl FnOnce() -> bool,
) -> Option<Vec<String>> {
    let route_key = format!("{route_ns}/{route_name}");
    let route_key = route_key.as_str();
    let mut requeue: BTreeSet<String> = BTreeSet::new();
    if !facts.is_participant() {
        if !graph.is_participant(route_key) {
            return None;
        }
        requeue.extend(graph.remove(route_key));
    } else {
        let targets = facts
            .targets
            .iter()
            .cloned()
            .zip(facts.target_rules.iter().cloned())
            .collect();
        let delta = graph.update_parent(route_key, targets);
        for target in delta.added.iter().chain(delta.removed.iter()) {
            requeue.extend(graph.children_of_target(route_key, target));
        }
        // An inert route (invalid shape, or a child with hostnames) still
        // declares its same-kind parents: the parent then does not report it
        // as `RefNotPermitted` (child-caused problems stay on the child). Its
        // declaration is not confirmable, so it gets no keys and no level.
        let declared = facts.declared_parents.clone();
        // Declared parents depend on this child only through the declared
        // set; requeue them only when it changes, so an unchanged child (or a
        // 2-cycle of mutually declaring routes) never ping-pongs.
        let mut old_declared = graph.declared_parents(route_key);
        let mut new_declared = declared.clone();
        old_declared.sort_unstable();
        old_declared.dedup();
        new_declared.sort_unstable();
        new_declared.dedup();
        let affected = graph.update_child(route_key, route_ns, declared, !facts.inert);
        if old_declared != new_declared {
            requeue.extend(affected);
        }
        let blocked_changed = graph.set_blocked(route_key, own_rejected());
        graph.set_root(route_key, gateway_attached && facts.shape == ParentRefShape::Root);
        let change = graph.recompute_level(route_key);
        // A blocked flip changes the children's message ("not accepted" vs
        // "not reachable") even when the level stays `None`.
        if change.old != change.new || blocked_changed {
            requeue.extend(graph.children_of(route_key));
        }
    }
    requeue.remove(route_key);
    Some(requeue.into_iter().collect())
}

/// Delegation keys of the child's confirmed edges, sorted for a
/// deterministic payload and bounded by the 3-parent rule.
pub(super) fn resolve_child_keys(graph: &DelegationGraph, child_key: &str) -> Vec<DelegationKey> {
    let mut keys = graph.confirmed_keys(child_key);
    keys.sort_by(|a, b| key_order(a).cmp(&key_order(b)));
    keys.truncate(MAX_CHILD_DELEGATION_KEYS);
    keys
}

fn key_order(key: &DelegationKey) -> (&str, &str, &str, &str) {
    let target_name = match &key.target_name {
        DelegationTargetName::Named(name) => name.as_str(),
        DelegationTargetName::Wildcard => "*",
    };
    (&key.parent_ns, &key.parent_name, &key.target_ns, target_name)
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/route_delegation_test.rs"]
mod tests;

// =====================================================================
// Status (§7)
// =====================================================================

/// True when the route-kind processor currently caches `key`.
pub(super) fn route_exists(runtime: &ControllerRuntime, kind: RouteKindName, key: &str) -> bool {
    runtime
        .processor_registry
        .get(kind.as_str())
        .is_some_and(|processor| processor.contains_key(key))
}

/// Parent-side `ResolvedRefs` errors for named targets (§7.1): a target
/// naming the route itself is a self-delegation; a missing child is
/// `BackendNotFound`; an existing child that does not declare this parent is
/// `RefNotPermitted`. Wildcard targets never report an error, and problems
/// of a declaring child are reported on the child only.
pub(super) fn parent_named_target_errors(
    graph: &DelegationGraph,
    runtime: &ControllerRuntime,
    kind: RouteKindName,
    parent_key: &str,
    facts: &DelegationFacts,
) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for target in &facts.targets {
        let DelegationTargetName::Named(name) = &target.name else {
            continue;
        };
        let child_key = format!("{}/{name}", target.ns);
        if child_key == parent_key {
            errors.push(ResolvedRefsError::DelegationSelfReference {
                kind: kind.as_str(),
                namespace: target.ns.clone(),
                name: name.clone(),
            });
            continue;
        }
        if graph.declared_parents(&child_key).iter().any(|p| p == parent_key) {
            continue;
        }
        errors.push(if route_exists(runtime, kind, &child_key) {
            ResolvedRefsError::DelegationChildNotDeclaring {
                kind: kind.as_str(),
                namespace: target.ns.clone(),
                name: name.clone(),
            }
        } else {
            ResolvedRefsError::DelegationChildNotFound {
                kind: kind.as_str(),
                namespace: target.ns.clone(),
                name: name.clone(),
            }
        });
    }
    errors
}

/// Delegation verdict of one route-kind parentRef entry of a child (§7.2).
pub(super) enum ChildEntryVerdict {
    /// `Accepted=True / DelegationAccepted`. `conflicting_rules` are the child
    /// rules whose standard filters collide with a mandatory filter of every
    /// delegating rule they can be reached through; `out_of_scope_rules` are
    /// the child rules no delegating rule of this parent can reach. Both are
    /// reported as `PartiallyInvalid`. When together they cover every child
    /// rule, the entry is `Rejected` (`UnsupportedValue`) instead.
    Accepted {
        message: String,
        conflicting_rules: Vec<usize>,
        out_of_scope_rules: Vec<usize>,
    },
    Rejected {
        error: AcceptedError,
    },
}

fn rejected(reason: &'static str, message: String) -> ChildEntryVerdict {
    ChildEntryVerdict::Rejected {
        error: AcceptedError::Delegation { reason, message },
    }
}

/// Compute the verdict for the child entry naming `parent_key`.
///
/// `child_rules` yields the child's per-rule static facts; it is read only
/// for an accepted edge.
pub(super) fn child_entry_verdict(
    graph: &DelegationGraph,
    kind: RouteKindName,
    own_key: &str,
    parent_key: &str,
    facts: &DelegationFacts,
    child_rules: impl FnOnce() -> Vec<ChildRuleFacts>,
) -> ChildEntryVerdict {
    let kind = kind.as_str();
    match &facts.shape {
        ParentRefShape::Invalid(reason) => return rejected(INVALID_PARENT_REFS, (*reason).to_string()),
        ParentRefShape::Child(_) if facts.inert => {
            return rejected(
                condition_reasons::UNSUPPORTED_VALUE,
                "a delegation child must not set spec.hostnames; hostnames are owned by the root route".to_string(),
            )
        }
        _ => {}
    }
    let Some(edge_rules) = graph.edge_rules(parent_key, own_key) else {
        return rejected(
            NO_MATCHING_PARENT,
            format!("Parent {kind} '{parent_key}' does not exist or does not delegate to this route"),
        );
    };
    match graph.level(parent_key) {
        None if graph.is_blocked(parent_key) => rejected(
            NOT_ATTACHED,
            format!("Parent {kind} '{parent_key}' is not accepted (see its own status) and serves no delegated traffic"),
        ),
        None => rejected(
            NOT_ATTACHED,
            format!(
                "Parent {kind} '{parent_key}' is not reachable from a Gateway-attached route within {MAX_ROUTE_DELEGATION_LEVELS} levels"
            ),
        ),
        Some(level) if level as usize >= MAX_ROUTE_DELEGATION_LEVELS => rejected(
            DEPTH_EXCEEDED,
            format!(
                "Parent {kind} '{parent_key}' is at delegation level {level}; at most {MAX_ROUTE_DELEGATION_LEVELS} levels are allowed"
            ),
        ),
        Some(_) => {
            let child_rules = child_rules();
            let (conflicting_rules, out_of_scope_rules) = classify_child_rules(&edge_rules, &child_rules);
            if !child_rules.is_empty() && conflicting_rules.len() + out_of_scope_rules.len() == child_rules.len() {
                // Same rule as the parent side: an attachment with no rule
                // left to serve is not accepted.
                return rejected(
                    condition_reasons::UNSUPPORTED_VALUE,
                    no_serving_rule_message(kind, parent_key, child_rules.len()),
                );
            }
            ChildEntryVerdict::Accepted {
                message: format!("Parent {kind} '{parent_key}' delegates to this route"),
                conflicting_rules,
                out_of_scope_rules,
            }
        }
    }
}

/// Message of a child entry whose every rule is outside the parent's scope
/// or conflicts with its mandatory filters (`rule_count > 0`).
fn no_serving_rule_message(kind: &str, parent_key: &str, rule_count: usize) -> String {
    let rules: Vec<String> = (0..rule_count).map(|index| format!("rules[{index}]")).collect();
    format!(
        "No rule of this route can serve under parent {kind} '{parent_key}': {} are outside its scope or conflict with its filters",
        rules.join(", ")
    )
}

/// Split the child rules of one edge into `(conflicting, out_of_scope)`.
///
/// A rule no delegating rule can reach is out of scope. A reachable rule
/// conflicts only when its standard filters collide with the mandatory
/// filters of every delegating rule that reaches it: through any other one
/// it still serves (the Gateway checks the mask of the delegating rule the
/// request entered through).
pub(super) fn classify_child_rules(
    edge_rules: &[DelegatingRule],
    child_rules: &[ChildRuleFacts],
) -> (Vec<usize>, Vec<usize>) {
    let mut conflicting = Vec::new();
    let mut out_of_scope = Vec::new();
    for (index, child) in child_rules.iter().enumerate() {
        let mut reaching = edge_rules.iter().filter(|rule| rule_reaches(rule, child)).peekable();
        if reaching.peek().is_none() {
            out_of_scope.push(index);
        } else if child.mask != 0 && reaching.all(|rule| rule.mask & child.mask != 0) {
            conflicting.push(index);
        }
    }
    (conflicting, out_of_scope)
}

/// `PartiallyInvalid` segments for one edge: child rules outside the
/// parent's delegated scope, and child rules conflicting with the parent's
/// mandatory filters. Neither kind is dropped (it may stay active under
/// other parents), so each segment has its own prefix and no active-rule
/// count.
pub(super) fn edge_message(
    kind: RouteKindName,
    parent_key: &str,
    conflicting: &[usize],
    out_of_scope: &[usize],
) -> Option<String> {
    let kind = kind.as_str();
    let mut segments = Vec::new();
    if !out_of_scope.is_empty() {
        let entries: Vec<String> = out_of_scope.iter().map(|index| format!("rules[{index}]")).collect();
        segments.push(format!(
            "Rule(s) outside parent {kind} '{parent_key}' scope: {}",
            entries.join(", ")
        ));
    }
    if !conflicting.is_empty() {
        let entries: Vec<String> = conflicting
            .iter()
            .map(|index| {
                format!("rules[{index}]: a standard filter conflicts with a mandatory filter of the same type")
            })
            .collect();
        segments.push(format!(
            "Conflicting Rule(s) with parent {kind} '{parent_key}': {}",
            entries.join("; ")
        ));
    }
    (!segments.is_empty()).then(|| segments.join("; "))
}
