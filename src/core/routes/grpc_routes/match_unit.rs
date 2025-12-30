use crate::types::err::EdError;
use crate::types::{GRPCRouteMatch, GRPCRouteRule};
use crate::types::resources::http_route::ParentReference;
use pingora_proxy::Session;
use regex::Regex;
use std::sync::Arc;

/// gRPC route level information shared across all rule units
#[derive(Clone, Debug)]
pub struct GrpcRouteInfo {
    pub parent_refs: Option<Vec<ParentReference>>,
    pub hostnames: Option<Vec<String>>,
}

/// gRPC route match information
#[derive(Clone)]
pub struct GrpcMatchInfo {
    /// Route namespace
    pub route_ns: String,
    /// Route name
    pub route_name: String,
    /// Rule id in GRPCRoute
    pub rule_id: usize,
    /// Match id at rule id
    pub match_id: usize,
    /// Match item (contains service/method in matched.method)
    pub matched: GRPCRouteMatch,
}

impl GrpcMatchInfo {
    pub fn new(
        route_ns: String,
        route_name: String,
        rule_id: usize,
        match_id: usize,
        matched: GRPCRouteMatch,
    ) -> Self {
        Self {
            route_ns,
            route_name,
            rule_id,
            match_id,
            matched,
        }
    }
}

impl std::fmt::Display for GrpcMatchInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref method_match) = self.matched.method {
            match (&method_match.service, &method_match.method) {
                (Some(s), Some(m)) => write!(
                    f,
                    "{}/{} (rule:{}, match:{}, service:{}, method:{})",
                    self.route_ns, self.route_name, self.rule_id, self.match_id, s, m
                ),
                _ => write!(
                    f,
                    "{}/{} (rule:{}, match:{})",
                    self.route_ns, self.route_name, self.rule_id, self.match_id
                ),
            }
        } else {
            write!(
                f,
                "{}/{} (rule:{}, match:{})",
                self.route_ns, self.route_name, self.rule_id, self.match_id
            )
        }
    }
}

/// gRPC route rule unit
#[derive(Clone)]
pub struct GrpcRouteRuleUnit {
    pub resource_key: String,
    pub matched_info: GrpcMatchInfo,
    pub rule: Arc<GRPCRouteRule>,
    /// Route level information (parent_refs, hostnames, etc.)
    pub route_info: Arc<GrpcRouteInfo>,
}

impl GrpcRouteRuleUnit {
    pub fn new(
        namespace: String,
        name: String,
        rule_id: usize,
        match_id: usize,
        resource_key: String,
        match_item: GRPCRouteMatch,
        rule: Arc<GRPCRouteRule>,
        route_info: Arc<GrpcRouteInfo>,
    ) -> Self {
        Self {
            resource_key,
            matched_info: GrpcMatchInfo::new(
                namespace,
                name,
                rule_id,
                match_id,
                match_item,
            ),
            rule,
            route_info,
        }
    }

    /// Deep match: check hostname, section_name, and headers
    pub fn deep_match(&self, session: &Session, listener_name: &str) -> Result<bool, EdError> {
        let req_header = session.req_header();

        // Check Hostname (if route specifies hostnames)
        if let Some(ref route_hostnames) = self.route_info.hostnames {
            if !route_hostnames.is_empty() {
                let req_hostname = Self::extract_hostname(req_header);
                if !Self::match_hostname(&req_hostname, route_hostnames) {
                    return Ok(false);
                }
            }
        }

        // Check SectionName (if parent_refs specify section_name)
        if let Some(ref parent_refs) = self.route_info.parent_refs {
            // At least one parent_ref must match: section_name is None or equals listener_name
            let matches = parent_refs.iter().any(|pr| {
                pr.section_name.as_ref().map_or(true, |name| name == listener_name)
            });
            
            if !matches {
                return Ok(false);
            }
        }

        // Check Headers (if specified) - ALL must match (AND logic)
        if let Some(header_matches) = &self.matched_info.matched.headers {
            for header_match in header_matches {
                if !Self::match_header(req_header, header_match)? {
                    tracing::trace!(
                        header = %header_match.name,
                        route = %self.identifier(),
                        "gRPC header match failed"
                    );
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    /// Extract hostname from request header
    fn extract_hostname(req_header: &pingora_http::RequestHeader) -> String {
        // Try URI host (HTTP/2), then Host header (HTTP/1.1), then :authority (HTTP/2 fallback)
        req_header.uri.host()
            .map(|h| h.to_string())
            .or_else(|| req_header.headers.get("host").and_then(|h| h.to_str().ok().map(|s| s.to_string())))
            .or_else(|| req_header.headers.get(":authority").and_then(|h| h.to_str().ok().map(|s| s.to_string())))
            .unwrap_or_default()
    }

    /// Match hostname against route hostnames (supports wildcards)
    fn match_hostname(req_hostname: &str, route_hostnames: &[String]) -> bool {
        for pattern in route_hostnames {
            if Self::hostname_matches(req_hostname, pattern) {
                return true;
            }
        }
        false
    }

    /// Check if hostname matches a pattern (supports wildcard *.example.com)
    fn hostname_matches(hostname: &str, pattern: &str) -> bool {
        if pattern.starts_with("*.") {
            // Wildcard match: *.example.com matches foo.example.com but not example.com
            let suffix = &pattern[2..]; // Remove "*."
            if let Some(dot_pos) = hostname.find('.') {
                let hostname_suffix = &hostname[dot_pos + 1..];
                return hostname_suffix == suffix;
            }
            false
        } else {
            // Exact match
            hostname == pattern
        }
    }

    /// Match gRPC header
    fn match_header(
        req_header: &pingora_http::RequestHeader,
        header_match: &crate::types::GRPCHeaderMatch,
    ) -> Result<bool, EdError> {
        let header_value = match req_header.headers.get(&header_match.name) {
            Some(value) => value.to_str().unwrap_or(""),
            None => return Ok(false),
        };

        let match_type = header_match.match_type.as_deref().unwrap_or("Exact");

        match match_type {
            "Exact" => Ok(header_value == header_match.value),
            "RegularExpression" => {
                let re = Regex::new(&header_match.value).map_err(|e| {
                    EdError::RouteMatchError(format!("Invalid regex: {}", e))
                })?;
                Ok(re.is_match(header_value))
            }
            _ => {
                tracing::warn!(
                    match_type = %match_type,
                    "Unsupported gRPC header match type, defaulting to Exact"
                );
                Ok(header_value == header_match.value)
            }
        }
    }

    /// Get route identifier
    pub fn identifier(&self) -> String {
        format!("{}/{}", self.matched_info.route_ns, self.matched_info.route_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hostname_exact_match() {
        assert!(GrpcRouteRuleUnit::hostname_matches("api.example.com", "api.example.com"));
        assert!(!GrpcRouteRuleUnit::hostname_matches("api.example.com", "foo.example.com"));
        assert!(!GrpcRouteRuleUnit::hostname_matches("api.example.com", "example.com"));
    }

    #[test]
    fn test_hostname_wildcard_match() {
        // Wildcard should match subdomain
        assert!(GrpcRouteRuleUnit::hostname_matches("api.example.com", "*.example.com"));
        assert!(GrpcRouteRuleUnit::hostname_matches("foo.example.com", "*.example.com"));
        
        // Wildcard should NOT match the domain itself
        assert!(!GrpcRouteRuleUnit::hostname_matches("example.com", "*.example.com"));
        
        // Wildcard should NOT match different domain
        assert!(!GrpcRouteRuleUnit::hostname_matches("api.other.com", "*.example.com"));
    }

    #[test]
    fn test_hostname_multi_level_subdomain() {
        // Multi-level subdomain
        assert!(GrpcRouteRuleUnit::hostname_matches("foo.bar.example.com", "*.bar.example.com"));
        assert!(!GrpcRouteRuleUnit::hostname_matches("foo.bar.example.com", "*.example.com"));
    }

    #[test]
    fn test_match_hostname_list() {
        let hostnames = vec![
            "api.example.com".to_string(),
            "*.test.com".to_string(),
        ];
        
        // Should match exact
        assert!(GrpcRouteRuleUnit::match_hostname("api.example.com", &hostnames));
        
        // Should match wildcard
        assert!(GrpcRouteRuleUnit::match_hostname("foo.test.com", &hostnames));
        
        // Should not match
        assert!(!GrpcRouteRuleUnit::match_hostname("other.com", &hostnames));
    }
}

