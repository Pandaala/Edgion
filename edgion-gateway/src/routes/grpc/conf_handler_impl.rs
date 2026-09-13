use crate::routes::common::{
    effective_hostnames, filter_accepted_parent_refs, gateways_from_parent_refs, resolve_parent_refs,
};
use crate::routes::grpc::match_unit::{GrpcMatchInfo, GrpcMatchSpecificity, GrpcRouteInfo, GrpcRuleRuntime};
use crate::routes::grpc::routes_mgr::{resolved_listeners_for_grpc_route, GlobalGrpcRouteManagers};
use crate::routes::grpc::{get_global_grpc_route_managers, GrpcRouteRuleUnit};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::{GRPCRoute, GRPCRouteRule, ListenerKey, ResourceMeta};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

impl ConfHandler<GRPCRoute> for GlobalGrpcRouteManagers {
    fn full_set(&self, data: &HashMap<String, Arc<GRPCRoute>>) {
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        let start_time = Instant::now();
        tracing::info!(component = "grpc_route_manager", count = data.len(), "full set start");

        // Step 1: Parse all routes and build units cache
        self.route_cache.clear();
        let mut new_units_cache: HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>> = HashMap::new();

        for (key, route) in data {
            let units = parse_route_to_units(key, route);
            new_units_cache.insert(key.clone(), units);
            self.route_cache.insert(key.clone(), route.clone()); // Arc clone
        }

        *self.route_units_cache.lock() = new_units_cache;

        // Step 2: Rebuild all per-listener managers (lock already held above).
        self.rebuild_all_listener_managers_locked();

        tracing::info!(
            component = "grpc_route_manager",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "full set done"
        );
        drop(_guard);
        crate::plugins::publication::global_publisher().replace_routes(
            crate::plugins::publication::inputs::RouteKind::Grpc,
            data.iter()
                .map(|(key, route)| {
                    (
                        key.clone(),
                        crate::plugins::publication::inputs::RouteInput::from_grpc(route),
                    )
                })
                .collect(),
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<GRPCRoute>>,
        update: HashMap<String, Arc<GRPCRoute>>,
        remove: HashSet<String>,
    ) {
        let policy_inputs = add
            .iter()
            .chain(update.iter())
            .map(|(key, route)| {
                (
                    key.clone(),
                    crate::plugins::publication::inputs::RouteInput::from_grpc(route),
                )
            })
            .collect();
        let policy_removed = remove.clone();
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        let start_time = Instant::now();
        tracing::info!(
            component = "grpc_route_manager",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update start"
        );

        let mut add_or_update = add;
        add_or_update.extend(update);

        // Compute affected listeners BEFORE updating cache
        let mut affected_listeners: HashSet<ListenerKey> = HashSet::new();

        for (key, route) in &add_or_update {
            let listeners = resolved_listeners_for_grpc_route(route);
            if listeners.is_empty() {
                tracing::warn!(route = %key, "GRPCRoute has no resolved_listeners during partial_update");
            } else {
                for listener in listeners {
                    affected_listeners.insert(listener.clone());
                }
            }
            if let Some(old) = self.route_cache.get(key) {
                for listener in resolved_listeners_for_grpc_route(old.value()) {
                    affected_listeners.insert(listener.clone());
                }
            }
        }
        for key in &remove {
            if let Some(old) = self.route_cache.get(key) {
                for listener in resolved_listeners_for_grpc_route(old.value()) {
                    affected_listeners.insert(listener.clone());
                }
            }
        }

        // Update route cache and units cache
        {
            let mut units_cache = self.route_units_cache.lock();
            for (key, route) in &add_or_update {
                let units = parse_route_to_units(key, route);
                units_cache.insert(key.clone(), units);
                self.route_cache.insert(key.clone(), route.clone()); // Arc clone
            }
            for key in &remove {
                self.route_cache.remove(key);
                units_cache.remove(key);
            }
        }

        // Rebuild affected listener managers
        self.rebuild_affected_listener_managers(&affected_listeners);

        tracing::info!(
            component = "grpc_route_manager",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "partial update done"
        );
        drop(_guard);
        crate::plugins::publication::global_publisher().update_routes(
            crate::plugins::publication::inputs::RouteKind::Grpc,
            policy_inputs,
            &policy_removed,
        );
    }
}

/// Create a GrpcRouteManager handler for registration with ConfigClient
pub fn create_grpc_route_handler() -> Box<dyn ConfHandler<GRPCRoute> + Send + Sync> {
    Box::new(get_global_grpc_route_managers())
}

#[cfg(test)]
pub(crate) fn parse_route_to_units_for_test(resource_key: &str, route: &Arc<GRPCRoute>) -> Vec<Arc<GrpcRouteRuleUnit>> {
    parse_route_to_units(resource_key, route)
}

/// Parse a single GRPCRoute into route rule units.
fn parse_route_to_units(resource_key: &str, route: &Arc<GRPCRoute>) -> Vec<Arc<GrpcRouteRuleUnit>> {
    if let Err(error) = route.spec.validate_hostnames() {
        tracing::warn!(component = "grpc_route_manager", route = resource_key, %error,
            "Invalid GRPCRoute hostnames; skipping the complete route");
        return Vec::new();
    }
    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
    let route_name = route.metadata.name.as_deref().unwrap_or("");

    let accepted_refs = match filter_accepted_parent_refs(
        route.spec.parent_refs.as_deref(),
        route.status.as_ref().map(|s| s.parents.as_slice()),
        Some(route_namespace),
    ) {
        Some(refs) => refs,
        None => return Vec::new(),
    };

    // Derive attached gateway pairs before accepted_refs is moved into route_info.
    let attached_gateways = gateways_from_parent_refs(&accepted_refs, route_namespace);

    let route_sv = route.get_sync_version();
    let (route_labels_meta, route_annotations_meta) = crate::ctx::extract_route_meta(&route.metadata);
    let creation_ts_ms = route
        .metadata
        .creation_timestamp
        .as_ref()
        .map(|t| t.0.timestamp_millis())
        .unwrap_or(0);
    let route_info = Arc::new(GrpcRouteInfo {
        parent_refs: Some(resolve_parent_refs(&accepted_refs, route_namespace)),
        effective_hostnames: effective_hostnames(
            route.spec.resolved_hostnames.as_deref(),
            route.spec.hostnames.as_deref(),
        ),
    });

    // Delegation: when the controller flattened a delegation tree into
    // resolved_rules, compile those INSTEAD of the operator-authored rules.
    // invalid_rule_indices refers to spec.rules; when resolved_rules is
    // present, flattening already dropped invalid rules and the indices do
    // not apply.
    let invalid_rule_indices = if route.spec.resolved_rules.is_some() {
        None
    } else {
        route.spec.invalid_rule_indices.as_ref()
    };

    let mut units = Vec::new();
    if let Some(rules) = route.effective_rules() {
        for (rule_id, rule) in rules.iter().enumerate() {
            // Skip rules marked invalid by Controller
            if invalid_rule_indices.is_some_and(|inv| inv.contains(&rule_id)) {
                continue;
            }
            // Compile matchers once, before allocating rule runtime state. An
            // invalid method predicate withdraws its entire rule on both planes,
            // including input whose Controller status is absent or stale.
            let catch_all = edgion_resources::GRPCRouteMatch {
                method: None,
                headers: None,
            };
            let effective_matches = rule
                .matches
                .as_deref()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| std::slice::from_ref(&catch_all));
            let matched_infos: Vec<_> = effective_matches
                .iter()
                .enumerate()
                .map(|(match_id, matched)| {
                    GrpcMatchInfo::new(
                        route_namespace.to_string(),
                        route_name.to_string(),
                        rule_id,
                        match_id,
                        matched.clone(),
                        route_sv,
                        rule.name.clone(),
                    )
                    .with_route_meta(route_labels_meta.clone(), route_annotations_meta.clone())
                })
                .collect();
            if matched_infos.iter().any(|info| info.compiled_method_match.is_none()) {
                continue;
            }
            let rule_arc = Arc::new(rule.clone());
            let rule_runtime: GrpcRuleRuntime = GrpcRouteRuleUnit::compile_grpc_rule(rule, route_namespace);
            let backend_finder = GrpcRouteRuleUnit::build_backend_selector(&rule_runtime.backends);
            let metric_handles = Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::build(
                &attached_gateways,
                route_namespace,
                route_name,
                &grpc_backends_from_rule(rule, route_namespace),
            ));

            for matched_info in matched_infos {
                let specificity = GrpcMatchSpecificity {
                    header_count: matched_info
                        .matched
                        .headers
                        .as_ref()
                        .map_or(0, |headers| headers.len() as u16),
                    creation_ts_ms,
                    route_key: resource_key.to_string(),
                    rule_index: rule_id as u32,
                    match_index: matched_info.match_id as u32,
                };
                units.push(Arc::new(GrpcRouteRuleUnit {
                    resource_key: resource_key.to_string(),
                    matched_info,
                    rule: Arc::clone(&rule_arc),
                    route: Some(Arc::clone(route)),
                    route_info: Arc::clone(&route_info),
                    specificity,
                    metric_handles: Arc::clone(&metric_handles),
                    plugin_runtime: Arc::clone(&rule_runtime.plugin_runtime),
                    backend_finder: Arc::clone(&backend_finder),
                    backends: Arc::clone(&rule_runtime.backends),
                }));
            }
        }
    }
    units
}

/// Extract `(backend_ns, backend_name)` tuples in `backend_refs` order, with
/// fallback to `route_ns` when the backend ref omits its namespace.
fn grpc_backends_from_rule(rule: &GRPCRouteRule, route_ns: &str) -> Vec<(String, String)> {
    rule.backend_refs
        .as_ref()
        .map(|refs| {
            refs.iter()
                .map(|b| {
                    (
                        b.namespace.clone().unwrap_or_else(|| route_ns.to_string()),
                        b.name.clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::{EdgionHttpContext, NormalizedHostname};
    use crate::routes::grpc::{try_match_grpc_route, GrpcMatchEngine};
    use crate::runtime::store::listener_gateway_info::{GatewayInfoIndex, ListenerEntry};
    use crate::runtime::GatewayInfo;
    use pingora_proxy::Session;
    use serde_json::json;
    use tokio::io::AsyncWriteExt;

    async fn matching_session(path: &str, hostname: &str, headers: &str) -> (Session, EdgionHttpContext) {
        let (mut client, server) = tokio::io::duplex(8192);
        client
            .write_all(
                format!("POST {path} HTTP/1.1\r\nHost: {hostname}\r\nContent-Length: 0\r\n{headers}\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let mut session = Session::new_h1(Box::new(server));
        assert!(session.read_request().await.unwrap());
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.normalized_current_path = path.to_string();
        ctx.request_info.hostname = NormalizedHostname::new(hostname);
        (session, ctx)
    }

    fn matching_index() -> GatewayInfoIndex {
        matching_index_with_hostname(None)
    }

    fn matching_index_with_hostname(hostname: Option<&str>) -> GatewayInfoIndex {
        GatewayInfoIndex::from_entries_for_test(vec![(
            "default/test-gw".to_string(),
            ListenerEntry {
                info: Arc::new(GatewayInfo::new(
                    Some("default".to_string()),
                    "test-gw".to_string(),
                    Some("grpc".to_string()),
                    None,
                    None,
                    None,
                    vec![],
                    None,
                )),
                hostname: hostname.map(str::to_string),
                allowed_routes: None,
            },
        )])
    }

    fn matching_route(name: &str, host: &str, matcher: serde_json::Value) -> Arc<GRPCRoute> {
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GRPCRoute",
            "metadata": {"name": name, "namespace": "default"},
            "spec": {"parentRefs": [{"name": "test-gw", "sectionName": "grpc"}],
                "hostnames": [host], "resolvedListeners": ["127.0.0.1:19235/HTTP"],
                "rules": [{"matches": [matcher], "backendRefs": [{"name": "svc", "port": 80}]}]}
        }))
        .unwrap();
        // Exercise the same serialized fields carried by conf_sync.
        Arc::new(serde_json::from_slice(&serde_json::to_vec(&route).unwrap()).unwrap())
    }

    #[tokio::test]
    async fn built_grpc_route_cannot_cross_an_empty_exact_listener_in_another_gateway() {
        let route = matching_route(
            "broad-owner-route",
            "*.example.com",
            json!({"method": {"method": "Get"}}),
        );
        let units = parse_route_to_units("default/broad-owner-route", &route);
        assert_eq!(units.len(), 1);
        let unit = Arc::clone(&units[0]);
        let engine = GrpcMatchEngine::new(units);
        let broad = matching_index_with_hostname(Some("*.example.com"));
        let blocked = GatewayInfoIndex::from_entries_for_test(vec![
            (
                "default/test-gw".into(),
                broad.get("default/test-gw", "grpc").unwrap().clone(),
            ),
            (
                "default/empty-exact".into(),
                ListenerEntry {
                    info: Arc::new(GatewayInfo::new(
                        Some("default".into()),
                        "empty-exact".into(),
                        Some("grpc".into()),
                        None,
                        None,
                        None,
                        Vec::new(),
                        None,
                    )),
                    hostname: Some("api.example.com".into()),
                    allowed_routes: None,
                },
            ),
        ]);
        let (session, ctx) = matching_session("/svc/Get", "api.example.com", "").await;
        let (selected, owner) = engine.match_route(&session, &ctx, &broad, "api.example.com").unwrap();
        assert!(Arc::ptr_eq(&selected, &unit));
        assert_eq!(owner.gateway_key(), "default/test-gw");
        assert!(matches!(
            engine.match_route(&session, &ctx, &blocked, "api.example.com"),
            Err(edgion_resources::EdError::RouteNotFound())
        ));
        let (other_session, other_ctx) = matching_session("/svc/Get", "other.example.com", "").await;
        let (selected, owner) = engine
            .match_route(&other_session, &other_ctx, &blocked, "other.example.com")
            .unwrap();
        assert!(Arc::ptr_eq(&selected, &unit));
        assert_eq!(owner.gateway_key(), "default/test-gw");
        assert!(engine.match_route(&session, &ctx, &broad, "api.example.com").is_ok());
    }

    #[tokio::test]
    async fn method_only_matches_through_real_session_and_header_failure_falls_back() {
        let index = matching_index();
        for method in [
            json!({"method":"Get"}),
            json!({"type":"Exact", "method":"Get"}),
            json!({"type":"RegularExpression", "method":"^Get.*$"}),
        ] {
            let route = matching_route(
                "method",
                "api.example.com",
                json!({"method":method,
                "headers":[{"name":"x-version", "value":"v2"}]}),
            );
            let fallback = matching_route("fallback", "*.example.com", json!({}));
            let mut units = parse_route_to_units("default/method", &route);
            units.extend(parse_route_to_units("default/fallback", &fallback));
            let engine = GrpcMatchEngine::new(units);
            for service in ["svc.User", "other.Service"] {
                let (session, ctx) =
                    matching_session(&format!("/{service}/Get"), "api.example.com", "x-version: v2\r\n").await;
                assert_eq!(
                    engine
                        .match_route(&session, &ctx, &index, "api.example.com")
                        .unwrap()
                        .0
                        .resource_key,
                    "default/method"
                );
            }
            let (session, ctx) = matching_session("/svc.User/Get", "api.example.com", "x-version: v1\r\n").await;
            assert_eq!(
                engine
                    .match_route(&session, &ctx, &index, "api.example.com")
                    .unwrap()
                    .0
                    .resource_key,
                "default/fallback"
            );
            let (session, ctx) = matching_session("/svc.User/Set", "api.example.com", "x-version: v2\r\n").await;
            assert_eq!(
                engine
                    .match_route(&session, &ctx, &index, "api.example.com")
                    .unwrap()
                    .0
                    .resource_key,
                "default/fallback"
            );
        }
    }

    #[test]
    fn invalid_method_withdraws_whole_rule_even_without_controller_status() {
        for bad in [
            json!({}),
            json!({"service":"", "method":"Get"}),
            json!({"type":"RegularExpression", "service":"svc", "method":"["}),
        ] {
            let mut route = (*matching_route("invalid", "api.example.com", json!({"method":bad}))).clone();
            let rule = &mut route.spec.rules.as_mut().unwrap()[0];
            rule.matches
                .as_mut()
                .unwrap()
                .push(serde_json::from_value(json!({"method":{"method":"Get"}})).unwrap());
            let mut valid = rule.clone();
            valid.matches = Some(vec![
                serde_json::from_value(json!({"method":{"method":"List"}})).unwrap()
            ]);
            route.spec.rules.as_mut().unwrap().push(valid);
            for resolved in [false, true] {
                if resolved {
                    route.spec.resolved_rules = route.spec.rules.clone();
                }
                let units = parse_route_to_units("default/invalid", &Arc::new(route.clone()));
                assert_eq!(units.len(), 1);
                assert_eq!(units[0].matched_info.rule_id, 1);
                assert_eq!(
                    units[0].matched_info.matched.method.as_ref().unwrap().method.as_deref(),
                    Some("List")
                );
            }
        }
    }

    #[tokio::test]
    async fn exact_listener_intersection_does_not_promote_wildcard_or_hostless_routes() {
        let mut exact = (*matching_route(
            "exact-host",
            "api.example.com",
            json!({"method":{"method":"Get"},
            "headers":[{"name":"x-version", "value":"v2"}]}),
        ))
        .clone();
        let mut wildcard =
            (*matching_route("wildcard-host", "*.example.com", json!({"method":{"method":"Get"}}))).clone();
        let mut hostless = (*matching_route(
            "hostless",
            "unused.example.com",
            json!({"method":{"service":"svc.User", "method":"Get"}}),
        ))
        .clone();
        hostless.spec.hostnames = None;
        // Controller intersection with an exact Listener yields the same
        // effective hostname for all three, but their Route priorities differ.
        let mut units = Vec::new();
        for route in [&mut hostless, &mut wildcard, &mut exact] {
            route.spec.resolved_hostnames = Some(vec!["api.example.com".to_string()]);
            let key = format!("default/{}", route.metadata.name.as_deref().unwrap());
            units.extend(parse_route_to_units(&key, &Arc::new(route.clone())));
        }
        let engine = GrpcMatchEngine::new(units);
        let index = matching_index_with_hostname(Some("api.example.com"));
        for (headers, expected) in [
            ("x-version: v2\r\n", "default/exact-host"),
            ("", "default/wildcard-host"),
        ] {
            let (session, ctx) = matching_session("/svc.User/Get", "api.example.com", headers).await;
            assert_eq!(
                engine
                    .match_route(&session, &ctx, &index, "api.example.com")
                    .unwrap()
                    .0
                    .resource_key,
                expected
            );
        }
        let (session, ctx) = matching_session("/svc.User/Get", "other.example.com", "").await;
        assert!(engine.match_route(&session, &ctx, &index, "other.example.com").is_err());
    }

    #[tokio::test]
    async fn method_and_hostname_indexes_replace_atomically_on_partial_update_and_delete() {
        let mgr = GlobalGrpcRouteManagers::new();
        let key = "default/method-reload".to_string();
        let listener = ListenerKey("127.0.0.1:19235/HTTP".to_string());
        let initial = matching_route("method-reload", "old.example.com", json!({"method":{"method":"Get"}}));
        mgr.full_set(&HashMap::from([(key.clone(), initial)]));
        let manager = mgr.by_listener.get(&listener).unwrap().clone();
        let old = Arc::clone(&manager.load_route_table());
        let index = matching_index();
        let (mut old_session, old_ctx) = matching_session("/svc.User/Get", "old.example.com", "").await;
        assert!(old
            .match_route(&mut old_session, &old_ctx, &index, "old.example.com")
            .is_ok());
        let updated = matching_route(
            "method-reload",
            "new.example.com",
            json!({"method":{"type":"RegularExpression", "method":"^List.*$"}}),
        );
        mgr.partial_update(HashMap::new(), HashMap::from([(key.clone(), updated)]), HashSet::new());
        let new = Arc::clone(&manager.load_route_table());
        assert!(!Arc::ptr_eq(&old, &new));
        assert!(new
            .match_route(&mut old_session, &old_ctx, &index, "old.example.com")
            .is_err());
        assert!(old
            .match_route(&mut old_session, &old_ctx, &index, "old.example.com")
            .is_ok());
        let (mut new_session, new_ctx) = matching_session("/other.Service/ListAll", "new.example.com", "").await;
        assert!(new
            .match_route(&mut new_session, &new_ctx, &index, "new.example.com")
            .is_ok());
        mgr.partial_update(HashMap::new(), HashMap::new(), HashSet::from([key]));
        assert!(!manager.load_route_table().has_routes());
        assert!(new
            .match_route(&mut new_session, &new_ctx, &index, "new.example.com")
            .is_ok());
    }

    #[tokio::test]
    async fn invalid_authored_hostname_list_withdraws_the_whole_published_route() {
        let managers = GlobalGrpcRouteManagers::new();
        let resource_key = "default/validated-hosts".to_string();
        let listener = ListenerKey("127.0.0.1:19235/HTTP".to_string());
        let valid = matching_route("validated-hosts", "grpc", json!({}));
        let index = matching_index();
        let (mut session, ctx) = matching_session("/svc/Get", "grpc", "").await;
        for hosts in [
            vec!["*".to_string()],
            vec!["grpc".to_string(), "*".to_string()],
            vec!["grpc".to_string(), "UPPER.example.com".to_string()],
            vec!["grpc".to_string(), "127.0.0.1".to_string()],
            vec!["grpc".to_string(); 17],
        ] {
            managers.full_set(&HashMap::from([(resource_key.clone(), valid.clone())]));
            let manager = managers.by_listener.get(&listener).unwrap().clone();
            let old = Arc::clone(&manager.load_route_table());
            assert!(old.match_route(&mut session, &ctx, &index, "grpc").is_ok());
            let mut invalid = (*valid).clone();
            invalid.spec.hostnames = Some(hosts);
            // Forged/stale resolved projection must not salvage authored invalidity.
            invalid.spec.resolved_hostnames = Some(vec!["grpc".to_string()]);
            let invalid = Arc::new(invalid);
            assert!(parse_route_to_units(&resource_key, &invalid).is_empty());
            managers.partial_update(
                HashMap::new(),
                HashMap::from([(resource_key.clone(), invalid)]),
                HashSet::new(),
            );
            assert!(!manager.load_route_table().has_routes());
            assert!(old.match_route(&mut session, &ctx, &index, "grpc").is_ok());
        }
        for hosts in [None, Some(Vec::new())] {
            let mut hostless = (*valid).clone();
            hostless.spec.hostnames = hosts;
            assert!(!parse_route_to_units(&resource_key, &Arc::new(hostless)).is_empty());
        }
    }

    #[tokio::test]
    async fn partial_and_full_rebuild_share_hostname_tree_with_coherent_payloads() {
        let managers = GlobalGrpcRouteManagers::new();
        let listener = ListenerKey("127.0.0.1:19235/HTTP".to_string());
        let first = matching_route("first", "api.example.com", json!({"method":{"method":"Get"}}));
        let fallback = matching_route("fallback", "*.example.com", json!({}));
        managers.full_set(&HashMap::from([
            ("default/first".to_string(), first),
            ("default/fallback".to_string(), fallback.clone()),
        ]));
        let manager = managers.by_listener.get(&listener).unwrap().clone();
        let old = Arc::clone(&manager.load_route_table());
        let unchanged = old
            .grpc_routes
            .route_rules_list
            .iter()
            .find(|unit| unit.resource_key == "default/fallback")
            .unwrap()
            .clone();
        let replacement = matching_route("first", "api.example.com", json!({"method":{"method":"List"}}));
        managers.partial_update(
            HashMap::new(),
            HashMap::from([("default/first".to_string(), replacement.clone())]),
            HashSet::new(),
        );
        let updated = Arc::clone(&manager.load_route_table());
        let old_engine = old.grpc_routes.match_engine.as_ref().unwrap();
        let new_engine = updated.grpc_routes.match_engine.as_ref().unwrap();
        assert!(old_engine.shares_hosts(new_engine));
        assert!(updated
            .grpc_routes
            .route_rules_list
            .iter()
            .any(|unit| Arc::ptr_eq(unit, &unchanged)));
        let index = matching_index();
        let (mut session, ctx) = matching_session("/svc/Get", "api.example.com", "").await;
        assert_eq!(
            old.match_route(&mut session, &ctx, &index, "api.example.com")
                .unwrap()
                .0
                .resource_key,
            "default/first"
        );
        assert_eq!(
            updated
                .match_route(&mut session, &ctx, &index, "api.example.com")
                .unwrap()
                .0
                .resource_key,
            "default/fallback"
        );
        let (mut list_session, list_ctx) = matching_session("/svc/List", "api.example.com", "").await;
        assert_eq!(
            updated
                .match_route(&mut list_session, &list_ctx, &index, "api.example.com")
                .unwrap()
                .0
                .resource_key,
            "default/first"
        );

        managers.full_set(&HashMap::from([
            ("default/first".to_string(), replacement),
            ("default/fallback".to_string(), fallback),
        ]));
        let reconnected = Arc::clone(&manager.load_route_table());
        assert!(new_engine.shares_hosts(reconnected.grpc_routes.match_engine.as_ref().unwrap()));
        managers.rebuild_all_listener_managers();
        let rebuilt = Arc::clone(&manager.load_route_table());
        assert!(new_engine.shares_hosts(rebuilt.grpc_routes.match_engine.as_ref().unwrap()));
        managers.full_set(&HashMap::new());
        assert!(!manager.load_route_table().has_routes());
        assert!(reconnected.has_routes());
    }

    #[tokio::test]
    async fn grpc_first_and_http_fallback_keep_overlapping_hostnames_usable() {
        use crate::routes::http::routes_mgr::GlobalHttpRouteManagers;
        let grpc_mgr = GlobalGrpcRouteManagers::new();
        let http_mgr = GlobalHttpRouteManagers::new();
        let listener = ListenerKey("127.0.0.1:19235/HTTP".to_string());
        let grpc = matching_route("grpc-first", "api.example.com", json!({"method":{"method":"Get"}}));
        grpc_mgr.full_set(&HashMap::from([("default/grpc-first".to_string(), grpc)]));
        let http = Arc::new(
            serde_json::from_value(json!({
                "apiVersion":"gateway.networking.k8s.io/v1", "kind":"HTTPRoute",
                "metadata":{"name":"http-fallback", "namespace":"default"},
                "spec":{"parentRefs":[{"name":"test-gw", "sectionName":"grpc"}],
                    "hostnames":["api.example.com"], "resolvedListeners":["127.0.0.1:19235/HTTP"],
                    "rules":[{"matches":[{"path":{"type":"PathPrefix", "value":"/"}}],
                        "backendRefs":[{"name":"svc", "port":80}]}]}
            }))
            .unwrap(),
        );
        http_mgr.full_set(&HashMap::from([("default/http-fallback".to_string(), http)]));
        let grpc_manager = grpc_mgr.by_listener.get(&listener).unwrap().clone();
        let http_manager = http_mgr.by_listener.get(&listener).unwrap().clone();
        let grpc_table = Arc::clone(&grpc_manager.load_route_table());
        let http_table = http_manager.load_route_table();
        let index = matching_index();
        for (method, expect_grpc) in [("Get", true), ("Set", false)] {
            let (mut session, mut ctx) = matching_session(&format!("/svc.User/{method}"), "api.example.com", "").await;
            let matched = try_match_grpc_route(&grpc_table, &mut session, &mut ctx, &index)
                .await
                .unwrap();
            assert_eq!(matched, expect_grpc);
            assert_eq!(ctx.is_grpc_route_matched, expect_grpc);
            if !matched {
                assert!(http_table.match_route(&mut session, &ctx, &index).is_ok());
            }
        }
    }

    /// Build a minimal GRPCRoute suitable for unit tests.
    ///
    /// The route has a single parent ref (so `filter_accepted_parent_refs` returns
    /// `Some`) and one catch-all rule with a backend ref (so `parse_route_to_units`
    /// produces at least one unit).
    fn build_test_grpc_route(namespace: &str, name: &str) -> GRPCRoute {
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "namespace": namespace, "name": name },
            "spec": {
                "parentRefs": [{ "name": "test-gw", "kind": "Gateway" }],
                "rules": [{
                    "backendRefs": [{ "name": "svc1", "port": 8080 }]
                }]
            }
        });
        let mut route: GRPCRoute = serde_json::from_value(json).expect("Failed to build GRPCRoute");
        route.pre_parse();
        route
    }

    /// Build a GRPCRoute with two `spec.rules`, each with a distinct backend
    /// name, so tests can tell which rule(s) actually compiled.
    fn build_grpc_route_with_two_rules(namespace: &str, name: &str) -> GRPCRoute {
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "namespace": namespace, "name": name },
            "spec": {
                "parentRefs": [{ "name": "test-gw", "kind": "Gateway" }],
                "rules": [
                    { "matches": [{ "method": { "service": "svc.SpecA" } }],
                      "backendRefs": [{ "name": "spec-svc-a", "port": 8080 }] },
                    { "matches": [{ "method": { "service": "svc.SpecB" } }],
                      "backendRefs": [{ "name": "spec-svc-b", "port": 8080 }] }
                ]
            }
        });
        let mut route: GRPCRoute = serde_json::from_value(json).expect("Failed to build GRPCRoute");
        route.pre_parse();
        route
    }

    /// Extract backend Service names from every unit's compiled rule, so
    /// tests can assert exactly which backendRefs were compiled.
    fn unit_backend_names(units: &[Arc<GrpcRouteRuleUnit>]) -> Vec<String> {
        let mut names: Vec<String> = units
            .iter()
            .flat_map(|u| u.rule.backend_refs.iter().flatten())
            .map(|br| br.name.clone())
            .collect();
        names.sort();
        names
    }

    /// Build-site wiring, gRPC side. Mirrors the HTTP test in
    /// `routes::http::conf_handler_impl`: only this test would fail if
    /// `parse_route_to_units` ever handed `resolve_parent_refs` something other
    /// than the route namespace that the request path later passes as `route_ns`.
    ///
    /// gRPC uses its own `unwrap_or("default")` fallback, independent of HTTP's
    /// stricter "reject a route with no namespace" rule, so it needs its own guard.
    #[test]
    fn build_site_resolves_parent_ref_keys_with_the_route_namespace() {
        // The fixture declares `parentRefs: [{ name: "test-gw" }]` with no
        // namespace, so the resolved key must fall back to the route namespace.
        let route = build_grpc_route_with_two_rules("route-ns", "wiring");
        let units = parse_route_to_units("route-ns/wiring", &Arc::new(route));
        assert!(!units.is_empty(), "the fixture must compile to at least one unit");

        for unit in &units {
            let resolved = unit
                .route_info
                .parent_refs
                .as_ref()
                .expect("the build site always populates parent_refs");
            assert_eq!(resolved.len(), 1);
            assert_eq!(
                resolved[0].gateway_key, "route-ns/test-gw",
                "the omitted parentRef namespace must fall back to the ROUTE namespace"
            );
            assert_eq!(resolved[0].parent_ns, "route-ns");
            // The same namespace the request path passes as `route_ns`.
            assert_eq!(unit.matched_info.rsc.ns, "route-ns");
        }
    }

    #[test]
    fn test_grpc_resolved_rules_compiled_instead_of_spec_rules() {
        // Delegation contract: when resolved_rules is present the gateway
        // compiles it INSTEAD of spec.rules, including a resolved Service
        // that is absent from spec.rules entirely (the flattened child).
        let mut route = build_grpc_route_with_two_rules("test-grpc-deleg", "flat");
        route.spec.resolved_rules = Some(
            serde_json::from_value(serde_json::json!([
                { "matches": [{ "method": { "service": "svc.Resolved" } }],
                  "backendRefs": [{ "name": "resolved-svc", "namespace": "test-grpc-deleg", "port": 8080 }] }
            ]))
            .unwrap(),
        );
        route.pre_parse();

        let route = Arc::new(route);
        let units = parse_route_to_units("test-grpc-deleg/flat", &route);
        assert_eq!(
            unit_backend_names(&units),
            vec!["resolved-svc"],
            "resolved_rules must replace spec.rules, compiling the resolved service"
        );
    }

    #[test]
    fn test_grpc_invalid_rule_indices_ignored_when_resolved_rules_present() {
        // invalid_rule_indices refers to spec.rules; a flattened route must
        // not have its (pre-filtered) resolved_rules dropped by stale indices.
        let mut route = build_grpc_route_with_two_rules("test-grpc-deleg-inv", "flat");
        route.spec.invalid_rule_indices = Some(vec![0]);
        route.spec.resolved_rules = Some(
            serde_json::from_value(serde_json::json!([
                { "matches": [{ "method": { "service": "svc.RA" } }],
                  "backendRefs": [{ "name": "svc-r-a", "port": 8080 }] },
                { "matches": [{ "method": { "service": "svc.RB" } }],
                  "backendRefs": [{ "name": "svc-r-b", "port": 8080 }] }
            ]))
            .unwrap(),
        );
        route.pre_parse();

        let route = Arc::new(route);
        let units = parse_route_to_units("test-grpc-deleg-inv/flat", &route);
        assert_eq!(
            unit_backend_names(&units),
            vec!["svc-r-a", "svc-r-b"],
            "invalid_rule_indices (index 0) must not apply to resolved_rules"
        );

        // Control: without resolved_rules the same indices DO drop rule 0.
        let mut plain = build_grpc_route_with_two_rules("test-grpc-deleg-inv", "plain");
        plain.spec.invalid_rule_indices = Some(vec![0]);
        let plain = Arc::new(plain);
        let units = parse_route_to_units("test-grpc-deleg-inv/plain", &plain);
        assert_eq!(
            unit_backend_names(&units),
            vec!["spec-svc-b"],
            "indices must still apply to spec.rules"
        );
    }

    #[test]
    fn test_grpc_full_set_shares_arc_and_unit_carries_route() {
        let mgr = GlobalGrpcRouteManagers::new();
        let key = "test-grpc-arc/route1";
        let route = std::sync::Arc::new(build_test_grpc_route("test-grpc-arc", "route1"));
        let mut data = std::collections::HashMap::new();
        data.insert(key.to_string(), route.clone());
        mgr.full_set(&data);

        let cached = mgr.route_cache.get(key).expect("cached");
        assert!(std::sync::Arc::ptr_eq(cached.value(), &route), "cache shares Arc");

        let units = mgr.route_units_cache.lock();
        let unit = units.get(key).and_then(|v| v.first()).expect("a unit");
        assert!(
            std::sync::Arc::ptr_eq(unit.route.as_ref().expect("unit route"), &route),
            "unit shares the cache Arc"
        );
    }

    #[test]
    fn metric_handles_are_shared_within_each_rule_only() {
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "namespace": "route-ns", "name": "metric-sharing" },
            "spec": {
                "parentRefs": [{ "name": "test-gw", "kind": "Gateway" }],
                "rules": [
                    {
                        "matches": [
                            { "method": { "service": "svc.RuleA", "method": "One" } },
                            { "method": { "service": "svc.RuleA", "method": "Two" } }
                        ],
                        "backendRefs": [
                            { "name": "rule-a-primary", "port": 8080 },
                            { "name": "rule-a-secondary", "port": 8081 }
                        ]
                    },
                    {
                        "matches": [{ "method": { "service": "svc.RuleB", "method": "Only" } }],
                        "backendRefs": [{ "name": "rule-b", "port": 9090 }]
                    }
                ]
            }
        });
        let mut route: GRPCRoute = serde_json::from_value(json).expect("Failed to build GRPCRoute");
        route.pre_parse();

        let units = parse_route_to_units("route-ns/metric-sharing", &Arc::new(route));
        assert_eq!(units.len(), 3);
        assert!(Arc::ptr_eq(&units[0].metric_handles, &units[1].metric_handles));
        assert!(!Arc::ptr_eq(&units[0].metric_handles, &units[2].metric_handles));

        assert_eq!((units[0].matched_info.rule_id, units[0].matched_info.match_id), (0, 0));
        assert_eq!((units[1].matched_info.rule_id, units[1].matched_info.match_id), (0, 1));
        assert_eq!((units[2].matched_info.rule_id, units[2].matched_info.match_id), (1, 0));
        let matched_methods: Vec<_> = units
            .iter()
            .map(|unit| {
                let method = unit.matched_info.matched.method.as_ref().expect("method match");
                (method.service.as_deref(), method.method.as_deref())
            })
            .collect();
        assert_eq!(
            matched_methods,
            vec![
                (Some("svc.RuleA"), Some("One")),
                (Some("svc.RuleA"), Some("Two")),
                (Some("svc.RuleB"), Some("Only")),
            ]
        );

        let first_rule_metrics = units[0]
            .metric_handles
            .lookup("route-ns", "test-gw")
            .expect("first rule gateway metrics");
        let second_rule_metrics = units[2]
            .metric_handles
            .lookup("route-ns", "test-gw")
            .expect("second rule gateway metrics");
        assert_eq!(first_rule_metrics.counters.len(), 2);
        assert_eq!(second_rule_metrics.counters.len(), 1);
    }
}
