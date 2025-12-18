use crate::core::routes::grpc_routes::GrpcRouteRuleUnit;
use crate::types::err::EdError;
use crate::types::GRPCMethodMatch;
use pingora_proxy::Session;
use regex::Regex;
use std::sync::Arc;

/// gRPC match engine for service/method based routing
pub struct GrpcMatchEngine {
    /// All GrpcRouteRuleUnit sorted by priority
    routes: Vec<Arc<GrpcRouteRuleUnit>>,
}

impl GrpcMatchEngine {
    pub fn new(routes: Vec<Arc<GrpcRouteRuleUnit>>) -> Self {
        Self { routes }
    }

    /// Match gRPC route based on service/method
    pub fn match_route(
        &self,
        session: &Session,
    ) -> Result<Arc<GrpcRouteRuleUnit>, EdError> {
        let path = session.req_header().uri.path();

        // Parse gRPC path: /{service}/{method}
        let (service, method) = parse_grpc_path(path)?;

        // Iterate through all route rules
        for route_unit in &self.routes {
            if let Some(ref grpc_method_match) = route_unit.matched_info.m.method {
                use crate::types::GRPCMethodMatchType;
                
                let match_type = grpc_method_match
                    .match_type
                    .as_ref()
                    .unwrap_or(&GRPCMethodMatchType::Exact);

                let matched = match match_type {
                    GRPCMethodMatchType::Exact => {
                        matches_exact(grpc_method_match, &service, &method)
                    }
                    GRPCMethodMatchType::RegularExpression => {
                        matches_regex(grpc_method_match, &service, &method)?
                    }
                };

                if matched {
                    // Service/Method matched, perform deep match (headers)
                    if route_unit.deep_match(session)? {
                        tracing::debug!(
                            service = %service,
                            method = %method,
                            route = %route_unit.identifier(),
                            "gRPC route matched"
                        );
                        return Ok(route_unit.clone());
                    }
                }
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

/// Exact match
fn matches_exact(
    grpc_match: &GRPCMethodMatch,
    service: &str,
    method: &str,
) -> bool {
    let service_matches = match &grpc_match.service {
        Some(s) => s == service,
        None => true, // No service specified means match all
    };

    let method_matches = match &grpc_match.method {
        Some(m) => m == method,
        None => true, // No method specified means match all
    };

    service_matches && method_matches
}

/// RegularExpression match
fn matches_regex(
    grpc_match: &GRPCMethodMatch,
    service: &str,
    method: &str,
) -> Result<bool, EdError> {
    let service_matches = match &grpc_match.service {
        Some(pattern) => {
            let re = Regex::new(pattern).map_err(|e| {
                EdError::RouteMatchError(format!("Invalid service regex: {}", e))
            })?;
            re.is_match(service)
        }
        None => true,
    };

    let method_matches = match &grpc_match.method {
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

