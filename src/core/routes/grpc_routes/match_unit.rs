use crate::core::gateway::gateway::route_match::check_gateway_listener_match;
use crate::core::gateway::gateway::GatewayInfo;
use crate::types::err::EdError;
use crate::types::resources::common::ParentReference;
use crate::types::{GRPCRouteMatch, GRPCRouteRule};
use pingora_proxy::Session;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// gRPC route level information shared across all rule units
#[derive(Clone, Debug)]
pub struct GrpcRouteInfo {
    pub parent_refs: Option<Vec<ParentReference>>,
    pub hostnames: Option<Vec<String>>,
}

/// gRPC route match information
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct GrpcMatchInfo {
    pub route_ns: String,
    pub route_name: String,
    /// Rule id in GRPCRoute
    pub rule_id: usize,
    /// Match id at rule id
    pub match_id: usize,
    /// Matched item (contains service/method in matched.method)
    pub matched: GRPCRouteMatch,
    /// Pre-compiled regex patterns for header matching (index corresponds to matched.headers)
    /// None if match_type is not RegularExpression or if compilation failed
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_header_regexes: Vec<Option<Arc<Regex>>>,
}

impl GrpcMatchInfo {
    pub fn new(route_ns: String, route_name: String, rule_id: usize, match_id: usize, matched: GRPCRouteMatch) -> Self {
        // Pre-compile regex patterns for header matching
        let compiled_header_regexes = if let Some(ref headers) = matched.headers {
            headers
                .iter()
                .map(|header_match| {
                    // Only compile if match_type is RegularExpression
                    if header_match.match_type.as_deref() == Some("RegularExpression") {
                        match Regex::new(&header_match.value) {
                            Ok(re) => Some(Arc::new(re)),
                            Err(e) => {
                                tracing::warn!(
                                    route = %format!("{}/{}", route_ns, route_name),
                                    rule_id = rule_id,
                                    match_id = match_id,
                                    header = %header_match.name,
                                    pattern = %header_match.value,
                                    error = %e,
                                    "Failed to compile header regex pattern, header match will fail"
                                );
                                None
                            }
                        }
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        Self {
            route_ns,
            route_name,
            rule_id,
            match_id,
            matched,
            compiled_header_regexes,
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
    /// Deep match: check hostname, Gateway/sectionName, and headers.
    ///
    /// Returns `Some(GatewayInfo)` of the matched gateway on success, `None` on failure.
    pub fn deep_match(&self, session: &Session, gateway_infos: &[GatewayInfo], hostname: &str) -> Result<Option<GatewayInfo>, EdError> {
        let req_header = session.req_header();

        // Check Hostname (if route specifies hostnames)
        if let Some(ref route_hostnames) = self.route_info.hostnames {
            if !route_hostnames.is_empty() && !Self::match_hostname(hostname, route_hostnames) {
                return Ok(None);
            }
        }

        // Check Gateway/Listener constraints (sectionName, hostname, AllowedRoutes)
        let matched_gi = if let Some(ref parent_refs) = self.route_info.parent_refs {
            match check_gateway_listener_match(
                parent_refs,
                gateway_infos,
                hostname,
                &self.matched_info.route_ns,
                "GRPCRoute",
                &self.matched_info.route_name,
            ) {
                Some(gi) => gi,
                None => return Ok(None),
            }
        } else {
            return Ok(None);
        };

        // Check Headers (if specified) - ALL must match (AND logic)
        if let Some(header_matches) = &self.matched_info.matched.headers {
            for (idx, header_match) in header_matches.iter().enumerate() {
                let compiled_regex = self
                    .matched_info
                    .compiled_header_regexes
                    .get(idx)
                    .and_then(|r| r.as_ref());
                if !Self::match_header(req_header, header_match, compiled_regex)? {
                    return Ok(None);
                }
            }
        }

        Ok(Some(matched_gi))
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
        if let Some(suffix) = pattern.strip_prefix("*.") {
            // Wildcard match: *.example.com matches foo.example.com but not example.com
            // Remove "*."
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
    /// Uses pre-compiled regex if provided, otherwise falls back to runtime compilation
    fn match_header(
        req_header: &pingora_http::RequestHeader,
        header_match: &crate::types::GRPCHeaderMatch,
        compiled_regex: Option<&Arc<Regex>>,
    ) -> Result<bool, EdError> {
        let header_value = match req_header.headers.get(&header_match.name) {
            Some(value) => value.to_str().unwrap_or(""),
            None => return Ok(false),
        };

        let match_type = header_match.match_type.as_deref().unwrap_or("Exact");

        match match_type {
            "Exact" => Ok(header_value == header_match.value),
            "RegularExpression" => {
                // Use pre-compiled regex if available
                if let Some(regex) = compiled_regex {
                    Ok(regex.is_match(header_value))
                } else {
                    // Fallback: compile at runtime (should not happen if pre-compilation succeeded)
                    tracing::warn!(
                        header = %header_match.name,
                        "Using runtime regex compilation for header match (pre-compilation failed)"
                    );
                    let re = Regex::new(&header_match.value)
                        .map_err(|e| EdError::RouteMatchError(format!("Invalid regex: {}", e)))?;
                    Ok(re.is_match(header_value))
                }
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

    /// Get route identifier with rule and match details
    pub fn identifier(&self) -> String {
        format!(
            "{}/{} (rule:{}, match:{})",
            self.matched_info.route_ns,
            self.matched_info.route_name,
            self.matched_info.rule_id,
            self.matched_info.match_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hostname_exact_match() {
        assert!(GrpcRouteRuleUnit::hostname_matches(
            "api.example.com",
            "api.example.com"
        ));
        assert!(!GrpcRouteRuleUnit::hostname_matches(
            "api.example.com",
            "foo.example.com"
        ));
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
        assert!(GrpcRouteRuleUnit::hostname_matches(
            "foo.bar.example.com",
            "*.bar.example.com"
        ));
        assert!(!GrpcRouteRuleUnit::hostname_matches(
            "foo.bar.example.com",
            "*.example.com"
        ));
    }

    #[test]
    fn test_match_hostname_list() {
        let hostnames = vec!["api.example.com".to_string(), "*.test.com".to_string()];

        // Should match exact
        assert!(GrpcRouteRuleUnit::match_hostname("api.example.com", &hostnames));

        // Should match wildcard
        assert!(GrpcRouteRuleUnit::match_hostname("foo.test.com", &hostnames));

        // Should not match
        assert!(!GrpcRouteRuleUnit::match_hostname("other.com", &hostnames));
    }
}
