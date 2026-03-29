use crate::core::gateway::routes::grpc::GrpcRouteRuleUnit;
use crate::core::gateway::runtime::store::port_gateway_info::GatewayInfoIndex;
use crate::core::gateway::runtime::GatewayInfo;
use crate::types::err::EdError;
use crate::types::resources::grpc_route::GRPCMethodMatchType;
use pingora_proxy::Session;
use std::collections::HashMap;
use std::sync::Arc;

/// gRPC match engine for service/method based routing with optimized lookup.
///
/// Supports both Exact and RegularExpression match types.
/// Match priority (highest first):
///   1. Exact service + Exact method
///   2. Exact service + Regex method
///   3. Regex service + Exact method
///   4. Regex service + Regex method
///   5. Exact service only (method not specified)
///   6. Regex service only (method not specified)
///   7. Catch-all (neither service nor method specified)
///
/// Within regex tiers, routes are sorted by pattern length (longest first).
pub struct GrpcMatchEngine {
    /// Exact match: service -> method -> routes (nested for zero-alloc &str lookup)
    exact_routes: HashMap<String, HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>>,

    /// Service-level exact match: service -> routes (method not specified)
    service_routes: HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>,

    /// Exact service + Regex method, sorted by method pattern length desc
    exact_service_regex_method: Vec<Arc<GrpcRouteRuleUnit>>,

    /// Regex service + Exact method, sorted by service pattern length desc
    regex_service_exact_method: Vec<Arc<GrpcRouteRuleUnit>>,

    /// Regex service + Regex method, sorted by combined pattern length desc
    regex_service_regex_method: Vec<Arc<GrpcRouteRuleUnit>>,

    /// Regex service only (method not specified), sorted by service pattern length desc
    regex_service_only: Vec<Arc<GrpcRouteRuleUnit>>,

    /// Catch-all routes (both service and method not specified), sorted by specificity
    catch_all_routes: Vec<Arc<GrpcRouteRuleUnit>>,
}

impl GrpcMatchEngine {
    pub fn new(routes: Vec<Arc<GrpcRouteRuleUnit>>) -> Self {
        let mut exact_routes: HashMap<String, HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>> = HashMap::new();
        let mut service_routes: HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>> = HashMap::new();
        let mut exact_service_regex_method = Vec::new();
        let mut regex_service_exact_method = Vec::new();
        let mut regex_service_regex_method = Vec::new();
        let mut regex_service_only = Vec::new();
        let mut catch_all_routes: Vec<Arc<GrpcRouteRuleUnit>> = Vec::new();

        for route in routes {
            if let Some(ref grpc_method_match) = route.matched_info.matched.method {
                let is_regex = grpc_method_match.match_type.as_ref() == Some(&GRPCMethodMatchType::RegularExpression);

                match (&grpc_method_match.service, &grpc_method_match.method, is_regex) {
                    // Exact service + Exact method
                    (Some(service), Some(method), false) => {
                        exact_routes
                            .entry(service.clone())
                            .or_default()
                            .entry(method.clone())
                            .or_default()
                            .push(route);
                    }
                    // Exact service only (no method)
                    (Some(service), None, false) => {
                        service_routes
                            .entry(service.clone())
                            .or_insert_with(Vec::new)
                            .push(route);
                    }
                    // Regex: service + method both present
                    (Some(_service), Some(_method), true) => {
                        // Determine sub-category based on which fields have compiled regexes
                        let has_service_regex = route.matched_info.compiled_service_regex.is_some();
                        let has_method_regex = route.matched_info.compiled_method_regex.is_some();

                        match (has_service_regex, has_method_regex) {
                            (false, true) => exact_service_regex_method.push(route),
                            (true, false) => regex_service_exact_method.push(route),
                            (true, true) => regex_service_regex_method.push(route),
                            // Both failed to compile — treat as exact fallback
                            (false, false) => {
                                tracing::warn!(
                                    route = %route.identifier(),
                                    "RegularExpression match type but no regex compiled, treating as exact"
                                );
                                exact_routes
                                    .entry(_service.clone())
                                    .or_default()
                                    .entry(_method.clone())
                                    .or_default()
                                    .push(route);
                            }
                        }
                    }
                    // Regex: service only (no method)
                    (Some(_), None, true) => {
                        if route.matched_info.compiled_service_regex.is_some() {
                            regex_service_only.push(route);
                        } else {
                            tracing::warn!(
                                route = %route.identifier(),
                                "RegularExpression service but regex compilation failed, skipping"
                            );
                        }
                    }
                    // Catch-all: neither service nor method
                    (None, None, _) => {
                        catch_all_routes.push(route);
                    }
                    // Method without service — not valid per Gateway API spec
                    (None, Some(_), _) => {
                        tracing::warn!(
                            route = %route.identifier(),
                            "Invalid gRPC route: method specified without service"
                        );
                    }
                }
            } else {
                // No method match specified (header-only match) — matches any service/method.
                // Per Gateway API spec, omitting the method field means match all gRPC methods.
                catch_all_routes.push(route);
            }
        }

        // Sort regex routes by pattern length descending (longer = more specific = higher priority),
        // with specificity tie-breaking per Gateway API spec.
        let sort_by_pattern_len = |routes: &mut Vec<Arc<GrpcRouteRuleUnit>>,
                                   service_len: fn(&Arc<GrpcRouteRuleUnit>) -> usize,
                                   method_len: fn(&Arc<GrpcRouteRuleUnit>) -> usize| {
            routes.sort_by(|a, b| {
                let len_a = service_len(a) + method_len(a);
                let len_b = service_len(b) + method_len(b);
                let len_cmp = len_b.cmp(&len_a);
                if len_cmp != std::cmp::Ordering::Equal {
                    return len_cmp;
                }
                a.specificity.cmp_precedence(&b.specificity)
            });
        };

        let service_regex_len = |r: &Arc<GrpcRouteRuleUnit>| -> usize {
            r.matched_info
                .compiled_service_regex
                .as_ref()
                .map(|re| re.as_str().len())
                .unwrap_or(0)
        };
        let method_regex_len = |r: &Arc<GrpcRouteRuleUnit>| -> usize {
            r.matched_info
                .compiled_method_regex
                .as_ref()
                .map(|re| re.as_str().len())
                .unwrap_or(0)
        };
        let service_exact_len = |r: &Arc<GrpcRouteRuleUnit>| -> usize {
            r.matched_info
                .matched
                .method
                .as_ref()
                .and_then(|m| m.service.as_ref())
                .map(|s| s.len())
                .unwrap_or(0)
        };
        let method_exact_len = |r: &Arc<GrpcRouteRuleUnit>| -> usize {
            r.matched_info
                .matched
                .method
                .as_ref()
                .and_then(|m| m.method.as_ref())
                .map(|s| s.len())
                .unwrap_or(0)
        };

        sort_by_pattern_len(&mut exact_service_regex_method, service_exact_len, method_regex_len);
        sort_by_pattern_len(&mut regex_service_exact_method, service_regex_len, method_exact_len);
        sort_by_pattern_len(&mut regex_service_regex_method, service_regex_len, method_regex_len);
        regex_service_only.sort_by(|a, b| {
            let len_a = service_regex_len(a);
            let len_b = service_regex_len(b);
            let len_cmp = len_b.cmp(&len_a);
            if len_cmp != std::cmp::Ordering::Equal {
                return len_cmp;
            }
            a.specificity.cmp_precedence(&b.specificity)
        });

        // Sort exact and service bucket routes by specificity
        for method_map in exact_routes.values_mut() {
            for bucket in method_map.values_mut() {
                bucket.sort_by(|a, b| a.specificity.cmp_precedence(&b.specificity));
            }
        }
        for bucket in service_routes.values_mut() {
            bucket.sort_by(|a, b| a.specificity.cmp_precedence(&b.specificity));
        }
        catch_all_routes.sort_by(|a, b| a.specificity.cmp_precedence(&b.specificity));

        Self {
            exact_routes,
            service_routes,
            exact_service_regex_method,
            regex_service_exact_method,
            regex_service_regex_method,
            regex_service_only,
            catch_all_routes,
        }
    }

    /// Match gRPC route based on service/method with optimized lookup.
    ///
    /// Returns matched route unit and the specific `GatewayInfo` that passed validation.
    pub fn match_route(
        &self,
        session: &Session,
        index: &GatewayInfoIndex,
        hostname: &str,
    ) -> Result<(Arc<GrpcRouteRuleUnit>, GatewayInfo), EdError> {
        let path = session.req_header().uri.path();
        let (service, method) = parse_grpc_path(path)?;

        // Priority 1: Exact service + Exact method
        if let Some(routes) = self.exact_routes.get(service).and_then(|m| m.get(method)) {
            for route_unit in routes {
                if let Some(matched_gi) = route_unit.deep_match(session, index, hostname)? {
                    return Ok((route_unit.clone(), matched_gi));
                }
            }
        }

        // Priority 2: Exact service + Regex method
        for route_unit in &self.exact_service_regex_method {
            let method_match = route_unit.matched_info.matched.method.as_ref();
            let service_matches = method_match
                .and_then(|m| m.service.as_ref())
                .map_or(false, |s| s == &service);
            if !service_matches {
                continue;
            }
            if let Some(ref re) = route_unit.matched_info.compiled_method_regex {
                if re.is_match(&method) {
                    if let Some(matched_gi) = route_unit.deep_match(session, index, hostname)? {
                        return Ok((route_unit.clone(), matched_gi));
                    }
                }
            }
        }

        // Priority 3: Regex service + Exact method
        for route_unit in &self.regex_service_exact_method {
            if let Some(ref re) = route_unit.matched_info.compiled_service_regex {
                if re.is_match(&service) {
                    let method_matches = route_unit
                        .matched_info
                        .matched
                        .method
                        .as_ref()
                        .and_then(|m| m.method.as_ref())
                        .map_or(false, |m| m == &method);
                    if method_matches {
                        if let Some(matched_gi) = route_unit.deep_match(session, index, hostname)? {
                            return Ok((route_unit.clone(), matched_gi));
                        }
                    }
                }
            }
        }

        // Priority 4: Regex service + Regex method
        for route_unit in &self.regex_service_regex_method {
            let service_ok = route_unit
                .matched_info
                .compiled_service_regex
                .as_ref()
                .map_or(false, |re| re.is_match(&service));
            let method_ok = route_unit
                .matched_info
                .compiled_method_regex
                .as_ref()
                .map_or(false, |re| re.is_match(&method));
            if service_ok && method_ok {
                if let Some(matched_gi) = route_unit.deep_match(session, index, hostname)? {
                    return Ok((route_unit.clone(), matched_gi));
                }
            }
        }

        // Priority 5: Exact service only (method not specified)
        if let Some(routes) = self.service_routes.get(service) {
            for route_unit in routes {
                if let Some(matched_gi) = route_unit.deep_match(session, index, hostname)? {
                    return Ok((route_unit.clone(), matched_gi));
                }
            }
        }

        // Priority 6: Regex service only (method not specified)
        for route_unit in &self.regex_service_only {
            if let Some(ref re) = route_unit.matched_info.compiled_service_regex {
                if re.is_match(&service) {
                    if let Some(matched_gi) = route_unit.deep_match(session, index, hostname)? {
                        return Ok((route_unit.clone(), matched_gi));
                    }
                }
            }
        }

        // Priority 7: Catch-all (sorted by specificity)
        for route_unit in &self.catch_all_routes {
            if let Some(matched_gi) = route_unit.deep_match(session, index, hostname)? {
                return Ok((route_unit.clone(), matched_gi));
            }
        }

        Err(EdError::RouteNotFound())
    }
}

/// Parse gRPC path: /{service}/{method}
/// Example: "/helloworld.Greeter/SayHello" → ("helloworld.Greeter", "SayHello")
/// Returns string slices into the original path to avoid heap allocation.
pub fn parse_grpc_path(path: &str) -> Result<(&str, &str), EdError> {
    let trimmed = path.strip_prefix('/').unwrap_or(path);
    if let Some((service, method)) = trimmed.split_once('/') {
        if !service.is_empty() && !method.is_empty() && !method.contains('/') {
            return Ok((service, method));
        }
    }
    Err(EdError::InvalidGrpcPath(path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::routes::grpc::match_unit::GrpcMatchInfo;
    use crate::types::resources::grpc_route::GRPCRouteRule;
    use crate::types::{GRPCHeaderMatch, GRPCMethodMatch, GRPCRouteMatch};

    #[test]
    fn test_parse_grpc_path_valid() {
        let result = parse_grpc_path("/helloworld.Greeter/SayHello");
        assert!(result.is_ok());
        let (service, method) = result.unwrap();
        assert_eq!(service, "helloworld.Greeter");
        assert_eq!(method, "SayHello");
    }

    #[test]
    fn test_parse_grpc_path_invalid() {
        assert!(parse_grpc_path("/invalid").is_err());
        assert!(parse_grpc_path("/").is_err());
        assert!(parse_grpc_path("").is_err());
        assert!(parse_grpc_path("/a/b/c").is_err());
    }

    #[test]
    fn test_parse_grpc_path_empty_parts() {
        assert!(parse_grpc_path("//").is_err());
        assert!(parse_grpc_path("/service/").is_err());
        assert!(parse_grpc_path("//method").is_err());
    }

    fn make_route(
        service: Option<&str>,
        method: Option<&str>,
        match_type: Option<GRPCMethodMatchType>,
        name: &str,
    ) -> Arc<GrpcRouteRuleUnit> {
        use crate::core::gateway::lb::BackendSelector;
        use crate::core::gateway::plugins::PluginRuntime;
        use crate::core::gateway::routes::grpc::match_unit::GrpcMatchSpecificity;
        use crate::core::gateway::routes::grpc::GrpcRouteInfo;

        let grpc_match = GRPCRouteMatch {
            method: Some(GRPCMethodMatch {
                match_type,
                service: service.map(|s| s.to_string()),
                method: method.map(|m| m.to_string()),
            }),
            headers: None,
        };
        let route_info = Arc::new(GrpcRouteInfo {
            parent_refs: None,
            effective_hostnames: vec!["*".to_string()],
        });
        let rule = Arc::new(GRPCRouteRule {
            name: None,
            matches: None,
            filters: None,
            backend_refs: None,
            timeouts: None,
            retry: None,
            session_persistence: None,
            backend_finder: BackendSelector::new(),
            plugin_runtime: Arc::new(PluginRuntime::new()),
            parsed_timeouts: None,
        });
        Arc::new(GrpcRouteRuleUnit {
            resource_key: format!("default/{}", name),
            matched_info: GrpcMatchInfo::new("default".to_string(), name.to_string(), 0, 0, grpc_match, 0, None),
            rule,
            route_info,
            specificity: GrpcMatchSpecificity {
                header_count: 0,
                creation_ts_ms: 0,
                route_key: format!("default/{}", name),
                rule_index: 0,
                match_index: 0,
            },
        })
    }

    #[test]
    fn test_multiple_routes_same_service_method() {
        let route1 = make_route(Some("test.Service"), Some("Method"), None, "route1");
        let route2 = make_route(Some("test.Service"), Some("Method"), None, "route2");

        let engine = GrpcMatchEngine::new(vec![route1, route2]);
        let routes = engine
            .exact_routes
            .get("test.Service")
            .and_then(|m| m.get("Method"))
            .unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].resource_key, "default/route1");
        assert_eq!(routes[1].resource_key, "default/route2");
    }

    #[test]
    fn test_regex_method_routes_classified() {
        // When type=RegularExpression, both service and method are compiled as regex.
        // "test.Service" is a valid regex (matches "testXService" etc.), so both compile.
        let route = make_route(
            Some("test.Service"),
            Some("Say.*"),
            Some(GRPCMethodMatchType::RegularExpression),
            "regex-method",
        );
        assert!(route.matched_info.compiled_method_regex.is_some());
        // type applies to both fields, so service is also compiled as regex
        assert!(route.matched_info.compiled_service_regex.is_some());

        let engine = GrpcMatchEngine::new(vec![route]);
        // Both compiled → goes to regex_service_regex_method
        assert_eq!(engine.regex_service_regex_method.len(), 1);
        assert!(engine.exact_service_regex_method.is_empty());
        assert!(engine.regex_service_exact_method.is_empty());
    }

    #[test]
    fn test_regex_service_routes_classified() {
        // Regex service + Exact method (both fields present, type=Regex → both compiled)
        // When type=Regex and both service+method present, both are compiled as regex
        let route = make_route(
            Some("test\\..*"),
            Some("Method.*"),
            Some(GRPCMethodMatchType::RegularExpression),
            "regex-both",
        );
        // Both should be compiled since match_type applies to both
        assert!(route.matched_info.compiled_service_regex.is_some());
        assert!(route.matched_info.compiled_method_regex.is_some());

        let engine = GrpcMatchEngine::new(vec![route]);
        assert_eq!(engine.regex_service_regex_method.len(), 1);
    }

    #[test]
    fn test_regex_service_only_classified() {
        let route = make_route(
            Some("test\\..*"),
            None,
            Some(GRPCMethodMatchType::RegularExpression),
            "regex-svc-only",
        );
        assert!(route.matched_info.compiled_service_regex.is_some());

        let engine = GrpcMatchEngine::new(vec![route]);
        assert_eq!(engine.regex_service_only.len(), 1);
    }

    #[test]
    fn test_regex_routes_sorted_by_pattern_length() {
        let short = make_route(
            Some("a"),
            Some("b.*"),
            Some(GRPCMethodMatchType::RegularExpression),
            "short",
        );
        let long = make_route(
            Some("a"),
            Some("very-long-pattern-.*"),
            Some(GRPCMethodMatchType::RegularExpression),
            "long",
        );

        let engine = GrpcMatchEngine::new(vec![short, long]);
        // Both service and method compiled as regex → regex_service_regex_method
        assert_eq!(engine.regex_service_regex_method.len(), 2);
        // Longest combined pattern first
        assert_eq!(engine.regex_service_regex_method[0].matched_info.rsc.name, "long");
        assert_eq!(engine.regex_service_regex_method[1].matched_info.rsc.name, "short");
    }

    fn make_header_only_route(
        headers: Vec<(&str, &str)>,
        name: &str,
        rule_index: u32,
        match_index: u32,
    ) -> Arc<GrpcRouteRuleUnit> {
        use crate::core::gateway::lb::BackendSelector;
        use crate::core::gateway::plugins::PluginRuntime;
        use crate::core::gateway::routes::grpc::match_unit::GrpcMatchSpecificity;
        use crate::core::gateway::routes::grpc::GrpcRouteInfo;

        let grpc_match = GRPCRouteMatch {
            method: None,
            headers: if headers.is_empty() {
                None
            } else {
                Some(
                    headers
                        .iter()
                        .map(|(n, v)| GRPCHeaderMatch {
                            match_type: None,
                            name: n.to_string(),
                            value: v.to_string(),
                        })
                        .collect(),
                )
            },
        };
        let route_info = Arc::new(GrpcRouteInfo {
            parent_refs: None,
            effective_hostnames: vec!["*".to_string()],
        });
        let rule = Arc::new(GRPCRouteRule {
            name: None,
            matches: None,
            filters: None,
            backend_refs: None,
            timeouts: None,
            retry: None,
            session_persistence: None,
            backend_finder: BackendSelector::new(),
            plugin_runtime: Arc::new(PluginRuntime::new()),
            parsed_timeouts: None,
        });
        Arc::new(GrpcRouteRuleUnit {
            resource_key: format!("default/{}", name),
            matched_info: GrpcMatchInfo::new("default".to_string(), name.to_string(), 0, 0, grpc_match, 0, None),
            rule,
            route_info,
            specificity: GrpcMatchSpecificity {
                header_count: headers.len() as u16,
                creation_ts_ms: 0,
                route_key: format!("default/{}", name),
                rule_index,
                match_index,
            },
        })
    }

    #[test]
    fn test_header_only_routes_classified_as_catch_all() {
        let route1 = make_header_only_route(vec![("Version", "one")], "header-r1", 0, 0);
        let route2 = make_header_only_route(vec![("Version", "two")], "header-r2", 1, 0);
        let route3 = make_header_only_route(vec![("Version", "two"), ("Color", "orange")], "header-r3", 0, 1);

        let engine = GrpcMatchEngine::new(vec![route1, route2, route3]);
        assert_eq!(engine.catch_all_routes.len(), 3);
        assert!(engine.exact_routes.is_empty());
        assert!(engine.service_routes.is_empty());
    }

    #[test]
    fn test_header_only_routes_sorted_by_specificity() {
        let route_1h = make_header_only_route(vec![("Version", "one")], "one-header", 0, 0);
        let route_2h = make_header_only_route(vec![("Version", "two"), ("Color", "orange")], "two-headers", 0, 1);

        let engine = GrpcMatchEngine::new(vec![route_1h, route_2h]);
        assert_eq!(engine.catch_all_routes.len(), 2);
        assert_eq!(engine.catch_all_routes[0].specificity.header_count, 2);
        assert_eq!(engine.catch_all_routes[1].specificity.header_count, 1);
    }
}
