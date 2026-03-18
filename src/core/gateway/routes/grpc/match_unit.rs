use crate::core::gateway::runtime::matching::route::{check_gateway_listener_match, hostname_matches_listener};
use crate::core::gateway::runtime::GatewayInfo;
use crate::types::err::EdError;
use crate::types::resources::common::ParentReference;
use crate::types::{GRPCRouteMatch, GRPCRouteRule};
use pingora_proxy::Session;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const CATCH_ALL_HOSTNAME: &str = "*";

/// gRPC route level information shared across all rule units
#[derive(Clone, Debug)]
pub struct GrpcRouteInfo {
    pub parent_refs: Option<Vec<ParentReference>>,
    /// Effective hostnames for this route (from resolved_hostnames or hostnames).
    /// Unlike HTTP routes (bucketed by domain), gRPC routes share a single
    /// match engine so we must verify hostname match explicitly.
    pub effective_hostnames: Vec<String>,
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
    /// sync_version from gRPC sync (0 = not set)
    #[serde(default, skip_serializing_if = "crate::types::ctx::is_zero")]
    pub sv: u64,
    /// Matched item (contains service/method in matched.method)
    pub matched: GRPCRouteMatch,
    /// Pre-compiled regex patterns for header matching (index corresponds to matched.headers)
    /// None if match_type is not RegularExpression or if compilation failed
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_header_regexes: Vec<Option<Arc<Regex>>>,
}

impl GrpcMatchInfo {
    pub fn new(
        route_ns: String,
        route_name: String,
        rule_id: usize,
        match_id: usize,
        matched: GRPCRouteMatch,
        sv: u64,
    ) -> Self {
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
            sv,
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
    /// Unlike HTTP routes which are bucketed by domain, gRPC routes share a single
    /// match engine indexed by service/method. We must explicitly verify that the
    /// request hostname matches the route's effective hostnames.
    ///
    /// Returns `Some(GatewayInfo)` of the matched gateway on success, `None` on failure.
    pub fn deep_match(
        &self,
        session: &Session,
        gateway_infos: &[GatewayInfo],
        hostname: &str,
    ) -> Result<Option<GatewayInfo>, EdError> {
        let req_header = session.req_header();

        // Check route-level hostname constraint: gRPC routes are NOT bucketed
        // by domain so we verify here that the request host is allowed.
        if !self.matches_hostname(hostname) {
            return Ok(None);
        }

        // Check Gateway/Listener constraints (sectionName, AllowedRoutes)
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
                    let re = Regex::new(&header_match.value)
                        .map_err(|e| EdError::RouteMatchError(format!("Invalid regex: {}", e)))?;
                    Ok(re.is_match(header_value))
                }
            }
            _ => Ok(header_value == header_match.value),
        }
    }

    /// Check if request hostname matches this route's effective hostnames.
    #[inline]
    fn matches_hostname(&self, request_hostname: &str) -> bool {
        let hostnames = &self.route_info.effective_hostnames;
        if hostnames.is_empty() || (hostnames.len() == 1 && hostnames[0] == CATCH_ALL_HOSTNAME) {
            return true;
        }
        for h in hostnames {
            if h.starts_with("*.") {
                if hostname_matches_listener(request_hostname, h) {
                    return true;
                }
            } else if request_hostname.eq_ignore_ascii_case(h) {
                return true;
            }
        }
        false
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
