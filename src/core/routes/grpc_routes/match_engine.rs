use crate::core::routes::grpc_routes::GrpcRouteRuleUnit;
use crate::types::err::EdError;
use crate::types::GRPCMethodMatch;
use pingora_proxy::Session;
use regex::Regex;
use std::sync::Arc;
use std::collections::HashMap;

/// gRPC match engine for service/method based routing with optimized lookup
pub struct GrpcMatchEngine {
    /// Exact match: (service, method) -> route
    exact_routes: HashMap<(String, String), Arc<GrpcRouteRuleUnit>>,
    
    /// Service-level match: service -> route (method not specified)
    service_routes: HashMap<String, Arc<GrpcRouteRuleUnit>>,
    
    /// Catch-all route (both service and method not specified)
    catch_all_route: Option<Arc<GrpcRouteRuleUnit>>,
    
    /// Regex routes (need sequential traversal)
    regex_routes: Vec<Arc<GrpcRouteRuleUnit>>,
}

impl GrpcMatchEngine {
    pub fn new(routes: Vec<Arc<GrpcRouteRuleUnit>>) -> Self {
        let mut exact_routes = HashMap::new();
        let mut service_routes = HashMap::new();
        let mut catch_all_route = None;
        let mut regex_routes = Vec::new();

        for route in routes {
            if let Some(ref grpc_method_match) = route.matched_info.matched.method {
                use crate::types::GRPCMethodMatchType;
                
                let match_type = grpc_method_match
                    .match_type
                    .as_ref()
                    .unwrap_or(&GRPCMethodMatchType::Exact);

                match match_type {
                    GRPCMethodMatchType::Exact => {
                        // Classify based on service and method presence
                        match (&grpc_method_match.service, &grpc_method_match.method) {
                            (Some(service), Some(method)) => {
                                // Exact match: both service and method specified
                                exact_routes.insert((service.clone(), method.clone()), route);
                            }
                            (Some(service), None) => {
                                // Service-level match: only service specified
                                service_routes.insert(service.clone(), route);
                            }
                            (None, None) => {
                                // Catch-all: neither service nor method specified
                                if catch_all_route.is_none() {
                                    catch_all_route = Some(route);
                                } else {
                                    tracing::warn!(
                                        "Multiple catch-all routes found, using first one"
                                    );
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
                    GRPCMethodMatchType::RegularExpression => {
                        // Regex routes need sequential traversal
                        regex_routes.push(route);
                    }
                }
            }
        }

        Self {
            exact_routes,
            service_routes,
            catch_all_route,
            regex_routes,
        }
    }

    /// Match gRPC route based on service/method with optimized lookup
    pub fn match_route(
        &self,
        session: &Session,
    ) -> Result<Arc<GrpcRouteRuleUnit>, EdError> {
        let path = session.req_header().uri.path();

        // Parse gRPC path: /{service}/{method}
        let (service, method) = parse_grpc_path(path)?;

        // Priority 1: Try exact match (service, method)
        if let Some(route_unit) = self.exact_routes.get(&(service.clone(), method.clone())) {
            if route_unit.deep_match(session)? {
                tracing::debug!(
                    service = %service,
                    method = %method,
                    route = %route_unit.identifier(),
                    match_type = "exact",
                    "gRPC route matched"
                );
                return Ok(route_unit.clone());
            }
        }

        // Priority 2: Try service-level match (service only)
        if let Some(route_unit) = self.service_routes.get(&service) {
            if route_unit.deep_match(session)? {
                tracing::debug!(
                    service = %service,
                    method = %method,
                    route = %route_unit.identifier(),
                    match_type = "service",
                    "gRPC route matched"
                );
                return Ok(route_unit.clone());
            }
        }

        // Priority 3: Try regex match (sequential)
        for route_unit in &self.regex_routes {
            if let Some(ref grpc_method_match) = route_unit.matched_info.matched.method {
                if matches_regex(grpc_method_match, &service, &method)? {
                    if route_unit.deep_match(session)? {
                        tracing::debug!(
                            service = %service,
                            method = %method,
                            route = %route_unit.identifier(),
                            match_type = "regex",
                            "gRPC route matched"
                        );
                        return Ok(route_unit.clone());
                    }
                }
            }
        }

        // Priority 4: Try catch-all match
        if let Some(ref route_unit) = self.catch_all_route {
            if route_unit.deep_match(session)? {
                tracing::debug!(
                    service = %service,
                    method = %method,
                    route = %route_unit.identifier(),
                    match_type = "catch_all",
                    "gRPC route matched"
                );
                return Ok(route_unit.clone());
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

/// RegularExpression match
fn matches_regex(
    matched: &GRPCMethodMatch,
    service: &str,
    method: &str,
) -> Result<bool, EdError> {
    let service_matches = match &matched.service {
        Some(pattern) => {
            let re = Regex::new(pattern).map_err(|e| {
                EdError::RouteMatchError(format!("Invalid service regex: {}", e))
            })?;
            re.is_match(service)
        }
        None => true,
    };

    let method_matches = match &matched.method {
        Some(pattern) => {
            let re = Regex::new(pattern).map_err(|e| {
                EdError::RouteMatchError(format!("Invalid method regex: {}", e))
            })?;
            re.is_match(method)
        }
        None => true,
    };

    Ok(service_matches && method_matches)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

