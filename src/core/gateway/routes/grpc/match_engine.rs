use crate::core::gateway::runtime::GatewayInfo;
use crate::core::gateway::routes::grpc::GrpcRouteRuleUnit;
use crate::types::err::EdError;
use pingora_proxy::Session;
use std::collections::HashMap;
use std::sync::Arc;

/// gRPC match engine for service/method based routing with optimized lookup
pub struct GrpcMatchEngine {
    /// Exact match: (service, method) -> routes
    /// Multiple routes may exist for same service/method with different hostnames or headers
    exact_routes: HashMap<(String, String), Vec<Arc<GrpcRouteRuleUnit>>>,

    /// Service-level match: service -> routes (method not specified)
    /// Multiple routes may exist for same service with different hostnames or headers
    service_routes: HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>,

    /// Catch-all route (both service and method not specified)
    catch_all_route: Option<Arc<GrpcRouteRuleUnit>>,
}

impl GrpcMatchEngine {
    pub fn new(routes: Vec<Arc<GrpcRouteRuleUnit>>) -> Self {
        let mut exact_routes = HashMap::new();
        let mut service_routes = HashMap::new();
        let mut catch_all_route = None;

        for route in routes {
            if let Some(ref grpc_method_match) = route.matched_info.matched.method {
                // Classify based on service and method presence
                // Note: We only support Exact match type (default behavior)
                match (&grpc_method_match.service, &grpc_method_match.method) {
                    (Some(service), Some(method)) => {
                        // Exact match: both service and method specified
                        // Use entry().or_insert_with() to append routes instead of overwriting
                        exact_routes
                            .entry((service.clone(), method.clone()))
                            .or_insert_with(Vec::new)
                            .push(route);
                    }
                    (Some(service), None) => {
                        // Service-level match: only service specified
                        // Use entry().or_insert_with() to append routes instead of overwriting
                        service_routes
                            .entry(service.clone())
                            .or_insert_with(Vec::new)
                            .push(route);
                    }
                    (None, None) => {
                        // Catch-all: neither service nor method specified
                        if catch_all_route.is_none() {
                            catch_all_route = Some(route);
                        } else {
                            tracing::warn!("Multiple catch-all routes found, using first one");
                        }
                    }
                    (None, Some(_)) => {
                        // Invalid: method without service (should not happen)
                        tracing::warn!(
                            route = %route.identifier(),
                            "Invalid gRPC route: method specified without service"
                        );
                    }
                }
            }
        }

        Self {
            exact_routes,
            service_routes,
            catch_all_route,
        }
    }

    /// Match gRPC route based on service/method with optimized lookup.
    ///
    /// Returns matched route unit and the specific `GatewayInfo` that passed validation.
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `gateway_infos`: All gateway/listener contexts available on this listener
    /// - `hostname`: Request hostname for route-level hostname matching
    pub fn match_route(
        &self,
        session: &Session,
        gateway_infos: &[GatewayInfo],
        hostname: &str,
    ) -> Result<(Arc<GrpcRouteRuleUnit>, GatewayInfo), EdError> {
        let path = session.req_header().uri.path();

        let (service, method) = parse_grpc_path(path)?;

        // Priority 1: Try exact match (service, method)
        if let Some(routes) = self.exact_routes.get(&(service.clone(), method.clone())) {
            for route_unit in routes {
                if let Some(matched_gi) = route_unit.deep_match(session, gateway_infos, hostname)? {
                    return Ok((route_unit.clone(), matched_gi));
                }
            }
        }

        // Priority 2: Try service-level match (service only)
        if let Some(routes) = self.service_routes.get(&service) {
            for route_unit in routes {
                if let Some(matched_gi) = route_unit.deep_match(session, gateway_infos, hostname)? {
                    return Ok((route_unit.clone(), matched_gi));
                }
            }
        }

        if let Some(ref route_unit) = self.catch_all_route {
            if let Some(matched_gi) = route_unit.deep_match(session, gateway_infos, hostname)? {
                return Ok((route_unit.clone(), matched_gi));
            }
        }

        Err(EdError::RouteNotFound())
    }
}

/// Parse gRPC path: /{service}/{method}
/// Example: "/helloworld.Greeter/SayHello" → ("helloworld.Greeter", "SayHello")
pub fn parse_grpc_path(path: &str) -> Result<(String, String), EdError> {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        Ok((parts[0].to_string(), parts[1].to_string()))
    } else {
        Err(EdError::InvalidGrpcPath(path.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::routes::grpc::match_unit::GrpcMatchInfo;
    use crate::types::resources::grpc_route::GRPCRouteRule;
    use crate::types::{GRPCMethodMatch, GRPCRouteMatch};

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

    #[test]
    fn test_multiple_routes_same_service_method() {
        // Test that multiple routes with same service/method are stored correctly
        // This verifies the fix for HashMap collision issue
        use crate::core::gateway::lb::BackendSelector;
        use crate::core::gateway::plugins::PluginRuntime;

        // Create route 1: test.Service/Method with hostname api.example.com
        let match1 = GRPCRouteMatch {
            method: Some(GRPCMethodMatch {
                match_type: None, // Use default (Exact)
                service: Some("test.Service".to_string()),
                method: Some("Method".to_string()),
            }),
            headers: None,
        };
        let route_info1 = Arc::new(crate::core::gateway::routes::grpc::GrpcRouteInfo {
            parent_refs: None,
            effective_hostnames: vec!["*".to_string()],
        });
        let rule1 = Arc::new(GRPCRouteRule {
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
        let route1 = Arc::new(GrpcRouteRuleUnit {
            resource_key: "default/route1".to_string(),
            matched_info: GrpcMatchInfo::new("default".to_string(), "route1".to_string(), 0, 0, match1),
            rule: rule1.clone(),
            route_info: route_info1,
        });

        // Create route 2: test.Service/Method with hostname grpc.example.com
        let match2 = GRPCRouteMatch {
            method: Some(GRPCMethodMatch {
                match_type: None, // Use default (Exact)
                service: Some("test.Service".to_string()),
                method: Some("Method".to_string()),
            }),
            headers: None,
        };
        let route_info2 = Arc::new(crate::core::gateway::routes::grpc::GrpcRouteInfo {
            parent_refs: None,
            effective_hostnames: vec!["*".to_string()],
        });
        let route2 = Arc::new(GrpcRouteRuleUnit {
            resource_key: "default/route2".to_string(),
            matched_info: GrpcMatchInfo::new("default".to_string(), "route2".to_string(), 0, 0, match2),
            rule: rule1,
            route_info: route_info2,
        });

        // Create match engine with both routes
        let engine = GrpcMatchEngine::new(vec![route1.clone(), route2.clone()]);

        // Verify both routes are stored in the exact_routes HashMap
        let key = ("test.Service".to_string(), "Method".to_string());
        assert!(engine.exact_routes.contains_key(&key));

        let routes = engine.exact_routes.get(&key).unwrap();
        assert_eq!(routes.len(), 2, "Should have 2 routes for same service/method");

        // Verify route identifiers
        assert_eq!(routes[0].matched_info.route_name, "route1");
        assert_eq!(routes[1].matched_info.route_name, "route2");

        // Verify route identifiers distinguish the two routes
        assert_eq!(routes[0].resource_key, "default/route1");
        assert_eq!(routes[1].resource_key, "default/route2");
    }
}
