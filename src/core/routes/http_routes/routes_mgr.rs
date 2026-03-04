use crate::core::gateway::gateway::GatewayInfo;
use crate::core::lb::{ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
use crate::core::matcher::host_match::radix_match::RadixHostMatchEngine;
use crate::core::routes::http_routes::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::core::routes::http_routes::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::core::routes::http_routes::match_unit::RouteMatchResult;
use crate::core::routes::http_routes::HttpRouteRuleUnit;
use crate::types::ctx::EdgionHttpContext;
use crate::types::err::EdError;
use crate::types::HTTPRoute;
use crate::types::{HTTPBackendRef, HTTPRouteRule};
use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::sync::{Arc, Mutex, RwLock};

type DomainStr = String;

pub struct RouteRules {
    /// All resource keys (HTTPRoute) that apply to this hostname
    /// Format: "namespace/name"
    pub(crate) resource_keys: RwLock<HashSet<String>>,

    /// Exact and prefix match routes (handled by radix tree)
    /// Stored as Arc to avoid cloning when returning from match_route
    pub(crate) route_rules_list: RwLock<Vec<Arc<HttpRouteRuleUnit>>>,
    /// Match engine for exact/prefix routes. None if there are no normal routes (only regex routes)
    pub(crate) match_engine: Option<Arc<RadixRouteMatchEngine>>,

    /// Regex match routes (handled separately)
    /// Uses HttpRouteRuleUnit with path_regex field set
    /// Stored as Arc to avoid cloning when returning from match_route
    /// Kept for backward compatibility and debugging, but matching uses regex_routes_engine
    pub(crate) regex_routes: RwLock<Vec<Arc<HttpRouteRuleUnit>>>,
    /// Regex routes engine for lock-free matching. None if there are no regex routes
    pub(crate) regex_routes_engine: Option<Arc<RegexRoutesEngine>>,
}

impl Clone for RouteRules {
    fn clone(&self) -> Self {
        Self {
            resource_keys: RwLock::new(self.resource_keys.read().unwrap().clone()),
            route_rules_list: RwLock::new(self.route_rules_list.read().unwrap().clone()),
            match_engine: self.match_engine.clone(),
            regex_routes: RwLock::new(self.regex_routes.read().unwrap().clone()),
            regex_routes_engine: self.regex_routes_engine.clone(),
        }
    }
}

impl RouteRules {
    /// Select backend from the matched route rule
    pub fn select_backend(rule: &Arc<HTTPRouteRule>) -> Result<HTTPBackendRef, EdError> {
        // Initialize selector if not yet initialized
        if !rule.backend_finder.is_initialized() {
            let (items, weights) = match &rule.backend_refs {
                Some(refs) if !refs.is_empty() => {
                    let items: Vec<HTTPBackendRef> = refs.clone();
                    // Default weight to 1 if not specified
                    let weights: Vec<Option<i32>> = refs.iter().map(|br| br.weight.or(Some(1))).collect();
                    (items, weights)
                }
                _ => (vec![], vec![]),
            };
            rule.backend_finder.init(items, weights);
        }

        // Select backend
        let backend_ref = rule.backend_finder.select().map_err(|err_code| match err_code {
            ERR_NO_BACKEND_REFS => EdError::BackendNotFound(),
            ERR_INCONSISTENT_WEIGHT => EdError::InconsistentWeight(),
            _ => EdError::BackendNotFound(),
        })?;

        // Check if this backend reference is denied (no matching ReferenceGrant)
        if let Some(ref denied) = backend_ref.ref_denied {
            return Err(EdError::RefDenied {
                target_namespace: denied.target_namespace.clone(),
                target_name: denied.target_name.clone(),
                reason: denied
                    .reason
                    .clone()
                    .unwrap_or_else(|| "NoMatchingReferenceGrant".to_string()),
            });
        }

        // Note: BackendTLSPolicy query is performed in pg_upstream_peer.rs
        // where route namespace is available for proper namespace inheritance

        Ok(backend_ref)
    }

    /// Find a specific backend_ref by name from the route rule's backend_refs.
    ///
    /// Used by DynamicInternalUpstream plugin to bypass weighted selection and
    /// target a specific backend.
    ///
    /// Security: The target backend_ref must exist in the route's backend_refs
    /// list, which was already validated at route configuration time
    /// (including ReferenceGrant for cross-namespace references).
    ///
    /// Note: This function intentionally does NOT filter by weight.
    /// A backend_ref with weight=0 can still be targeted via DynamicInternalUpstream,
    /// enabling "hidden backend" patterns for debugging/testing.
    pub fn find_backend_by_name(
        rule: &Arc<HTTPRouteRule>,
        target_name: &str,
        target_namespace: Option<&str>,
        route_namespace: &str,
    ) -> Result<HTTPBackendRef, EdError> {
        let backend_refs = rule
            .backend_refs
            .as_ref()
            .filter(|refs| !refs.is_empty())
            .ok_or(EdError::BackendNotFound())?;

        for br in backend_refs {
            // Match by name
            if br.name != target_name {
                continue;
            }

            // If namespace is specified in jump target, verify it matches
            if let Some(ns) = target_namespace {
                let br_ns = br.namespace.as_deref().unwrap_or(route_namespace);
                if br_ns != ns {
                    continue;
                }
            }

            // Check cross-namespace reference denial
            if let Some(ref denied) = br.ref_denied {
                return Err(EdError::RefDenied {
                    target_namespace: denied.target_namespace.clone(),
                    target_name: denied.target_name.clone(),
                    reason: denied
                        .reason
                        .clone()
                        .unwrap_or_else(|| "NoMatchingReferenceGrant".to_string()),
                });
            }

            return Ok(br.clone());
        }

        Err(EdError::BackendNotFound())
    }

    /// Match a route using the match engines.
    /// Try match in order: regex → radix (exact + prefix).
    /// Returns `RouteMatchResult` on success (route + matched gateway context).
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `ctx`: Request context containing hostname and other request info
    /// - `gateway_infos`: All gateway/listener contexts available on this listener
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &EdgionHttpContext,
        gateway_infos: &[GatewayInfo],
    ) -> Result<RouteMatchResult, EdError> {
        if let Some(ref regex_engine) = self.regex_routes_engine {
            if let Some(result) = regex_engine.match_route(session, ctx, gateway_infos)? {
                return Ok(result);
            }
        }

        if let Some(ref match_engine) = self.match_engine {
            return match_engine.match_route(session, ctx, gateway_infos);
        }

        Err(EdError::RouteNotFound())
    }
}

pub struct DomainRouteRules {
    /// Exact domain matching (e.g., "example.com")
    /// Uses HashMap for O(1) lookup performance
    pub(crate) exact_domain_map: ArcSwap<HashMap<DomainStr, Arc<RouteRules>>>,

    /// Wildcard domain matching (e.g., "*.example.com")
    /// Uses RadixHostMatchEngine for wildcard support
    /// None if no wildcard domains are configured
    pub(crate) wildcard_engine: ArcSwap<Option<RadixHostMatchEngine<RouteRules>>>,
}

impl DomainRouteRules {
    /// Match a route for the given hostname and session against the global route table.
    ///
    /// Matching priority (per Gateway API spec):
    /// 1. Exact domain match (HashMap lookup - O(1))
    /// 2. Wildcard domain match (RadixHostMatchEngine - O(log n))
    /// 3. Catch-all ("*") domain match
    ///
    /// Gateway/listener validation happens inside `deep_match` for each candidate route.
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `ctx`: Request context containing hostname and other request info
    /// - `gateway_infos`: All gateway/listener contexts available on this listener
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &EdgionHttpContext,
        gateway_infos: &[GatewayInfo],
    ) -> Result<RouteMatchResult, EdError> {
        let hostname = &ctx.request_info.hostname;

        let exact_map = self.exact_domain_map.load();
        let wildcard_engine = self.wildcard_engine.load();

        if let Some(route_rules) = exact_map.get(&hostname.to_lowercase()) {
            return route_rules.match_route(session, ctx, gateway_infos);
        }

        // Step 2: Try wildcard domain match (O(log n))
        if let Some(ref engine) = wildcard_engine.as_ref() {
            if let Some(route_rules) = engine.match_host(hostname) {
                return route_rules.match_route(session, ctx, gateway_infos);
            }
        }

        // Step 3: Try catch-all routes (hostname "*", from HTTPRoutes with no spec.hostnames)
        if let Some(route_rules) = exact_map.get("*") {
            return route_rules.match_route(session, ctx, gateway_infos);
        }

        Err(EdError::RouteNotFound())
    }
}

type RouteKey = String; // Format: "namespace/name"

pub struct RouteManager {
    /// Single global route table shared by all gateways/listeners.
    /// Routes carry their own parentRef → Gateway binding; gateway validation
    /// happens during deep_match via the caller-supplied `gateway_infos`.
    pub(crate) global_routes: ArcSwap<DomainRouteRules>,

    /// Stores all HTTPRoute resources for lookup during delete events.
    /// Key format: "namespace/name"
    /// Uses Mutex since route updates are serialized (no concurrent writes needed).
    pub(crate) http_routes: Mutex<HashMap<RouteKey, HTTPRoute>>,
}

// Global RouteManager instance
static GLOBAL_ROUTE_MANAGER: LazyLock<Arc<RouteManager>> = LazyLock::new(|| Arc::new(RouteManager::new()));

/// Get the global RouteManager instance
pub fn get_global_route_manager() -> Arc<RouteManager> {
    GLOBAL_ROUTE_MANAGER.clone()
}

impl Default for RouteManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteManager {
    pub fn new() -> Self {
        Self {
            global_routes: ArcSwap::from_pointee(DomainRouteRules {
                exact_domain_map: ArcSwap::from_pointee(HashMap::new()),
                wildcard_engine: ArcSwap::from_pointee(None),
            }),
            http_routes: Mutex::new(HashMap::new()),
        }
    }

    /// Get the current global route table snapshot.
    pub fn get_global_routes(&self) -> arc_swap::Guard<Arc<DomainRouteRules>> {
        self.global_routes.load()
    }
}
