use crate::types::err::EdError;
use crate::types::{GRPCRouteMatch, GRPCRouteRule};
use pingora_proxy::Session;
use regex::Regex;
use std::sync::Arc;

/// gRPC route match information
#[derive(Clone)]
pub struct GrpcMatchInfo {
    /// Route namespace
    pub rns: String,
    /// Route name
    pub rn: String,
    /// Rule id in GRPCRoute
    pub rule_id: usize,
    /// Match id at rule id
    pub match_id: usize,
    /// gRPC service (parsed from path)
    pub service: Option<String>,
    /// gRPC method (parsed from path)
    pub method: Option<String>,
    /// Match item
    pub m: GRPCRouteMatch,
}

impl GrpcMatchInfo {
    pub fn new(
        rns: String,
        rn: String,
        rule_id: usize,
        match_id: usize,
        service: Option<String>,
        method: Option<String>,
        m: GRPCRouteMatch,
    ) -> Self {
        Self {
            rns,
            rn,
            rule_id,
            match_id,
            service,
            method,
            m,
        }
    }
}

impl std::fmt::Display for GrpcMatchInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.service, &self.method) {
            (Some(s), Some(m)) => write!(
                f,
                "{}/{} (rule:{}, match:{}, service:{}, method:{})",
                self.rns, self.rn, self.rule_id, self.match_id, s, m
            ),
            _ => write!(
                f,
                "{}/{} (rule:{}, match:{})",
                self.rns, self.rn, self.rule_id, self.match_id
            ),
        }
    }
}

/// gRPC route rule unit
#[derive(Clone)]
pub struct GrpcRouteRuleUnit {
    pub resource_key: String,
    pub matched_info: GrpcMatchInfo,
    pub rule: Arc<GRPCRouteRule>,
    /// Hostnames from GRPCRoute spec for hostname filtering
    pub hostnames: Option<Vec<String>>,
}

impl GrpcRouteRuleUnit {
    pub fn new(
        namespace: String,
        name: String,
        rule_id: usize,
        match_id: usize,
        resource_key: String,
        service: Option<String>,
        method: Option<String>,
        match_item: GRPCRouteMatch,
        rule: Arc<GRPCRouteRule>,
        hostnames: Option<Vec<String>>,
    ) -> Self {
        Self {
            resource_key,
            matched_info: GrpcMatchInfo::new(
                namespace,
                name,
                rule_id,
                match_id,
                service,
                method,
                match_item,
            ),
            rule,
            hostnames,
        }
    }

    /// Deep match: check hostname and headers
    pub fn deep_match(&self, session: &Session) -> Result<bool, EdError> {
        let req_header = session.req_header();

        // Check Hostname (if route specifies hostnames)
        if let Some(ref route_hostnames) = self.hostnames {
            if !route_hostnames.is_empty() {
                let req_hostname = Self::extract_hostname(req_header);
                if !Self::match_hostname(&req_hostname, route_hostnames) {
                    tracing::trace!(
                        req_hostname = %req_hostname,
                        route_hostnames = ?route_hostnames,
                        route = %self.identifier(),
                        "gRPC hostname match failed"
                    );
                    return Ok(false);
                }
            }
        }

        // Check Headers (if specified) - ALL must match (AND logic)
        if let Some(header_matches) = &self.matched_info.m.headers {
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
        format!("{}/{}", self.matched_info.rns, self.matched_info.rn)
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

