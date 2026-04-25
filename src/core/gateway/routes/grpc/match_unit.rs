use crate::core::gateway::runtime::matching::route::{check_gateway_listener_match, hostname_matches_listener};
use crate::core::gateway::runtime::store::listener_gateway_info::GatewayInfoIndex;
use crate::core::gateway::runtime::GatewayInfo;
use crate::types::err::EdError;
use crate::types::resources::common::ParentReference;
use crate::types::{GRPCRouteMatch, GRPCRouteRule};
use pingora_proxy::Session;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::sync::Arc;

/// Pre-computed match specificity for GRPCRoute Gateway API precedence ordering.
///
/// GRPCRoute precedence (within the same service/method bucket):
///   header_count > creation_ts > route_key > rule_index > match_index
///
/// Service/method-level precedence is handled by the bucket structure in GrpcMatchEngine.
#[derive(Clone, Debug)]
pub struct GrpcMatchSpecificity {
    /// Number of header match conditions
    pub header_count: u16,
    /// Route creation timestamp (older = higher priority)
    pub creation_ts_ms: i64,
    /// Route "{namespace}/{name}" for alphabetical tie-breaking
    pub route_key: String,
    /// Rule index within the route (first wins)
    pub rule_index: u32,
    /// Match index within the rule (first wins)
    pub match_index: u32,
}

impl GrpcMatchSpecificity {
    /// Compare two GrpcMatchSpecificity values per Gateway API precedence.
    /// Returns Ordering where Less means `self` has higher priority.
    pub fn cmp_precedence(&self, other: &Self) -> Ordering {
        // More headers = higher priority
        other
            .header_count
            .cmp(&self.header_count)
            // Older route = higher priority
            .then(self.creation_ts_ms.cmp(&other.creation_ts_ms))
            // Alphabetical by namespace/name
            .then(self.route_key.cmp(&other.route_key))
            // First rule/match wins
            .then(self.rule_index.cmp(&other.rule_index))
            .then(self.match_index.cmp(&other.match_index))
    }
}

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
    /// Resource identifier (kind="GRPCRoute")
    #[schemars(skip)]
    pub rsc: crate::types::Rsc,
    /// Rule id in GRPCRoute
    pub rule_id: usize,
    /// Rule name (from GRPCRouteRule.name, if set)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_name: Option<String>,
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
    /// Pre-compiled regex for service name matching (when match_type is RegularExpression)
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_service_regex: Option<Arc<Regex>>,
    /// Pre-compiled regex for method name matching (when match_type is RegularExpression)
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_method_regex: Option<Arc<Regex>>,
}

impl GrpcMatchInfo {
    pub fn new(
        route_ns: String,
        route_name: String,
        rule_id: usize,
        match_id: usize,
        matched: GRPCRouteMatch,
        sv: u64,
        rule_name: Option<String>,
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

        // Pre-compile regex patterns for service/method matching
        use crate::types::resources::grpc_route::GRPCMethodMatchType;
        let (compiled_service_regex, compiled_method_regex) = if let Some(ref method_match) = matched.method {
            let is_regex = method_match.match_type.as_ref() == Some(&GRPCMethodMatchType::RegularExpression);
            let compile = |field: &Option<String>, label: &str| -> Option<Arc<Regex>> {
                if !is_regex {
                    return None;
                }
                field.as_ref().and_then(|pattern| match Regex::new(pattern) {
                    Ok(re) => Some(Arc::new(re)),
                    Err(e) => {
                        tracing::warn!(
                            route = %format!("{}/{}", route_ns, route_name),
                            rule_id = rule_id,
                            match_id = match_id,
                            field = label,
                            pattern = %pattern,
                            error = %e,
                            "Failed to compile gRPC method regex pattern, match will fail"
                        );
                        None
                    }
                })
            };
            (
                compile(&method_match.service, "service"),
                compile(&method_match.method, "method"),
            )
        } else {
            (None, None)
        };

        Self {
            rsc: crate::types::Rsc {
                kind: "GRPCRoute".to_string(),
                ns: route_ns,
                name: route_name,
            },
            rule_id,
            rule_name,
            match_id,
            sv,
            matched,
            compiled_header_regexes,
            compiled_service_regex,
            compiled_method_regex,
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
    /// Pre-computed match specificity for Gateway API precedence ordering.
    pub specificity: GrpcMatchSpecificity,
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
        index: &GatewayInfoIndex,
        hostname: &str,
    ) -> Result<Option<GatewayInfo>, EdError> {
        let req_header = session.req_header();

        // Check route-level hostname constraint: gRPC routes are NOT bucketed
        // by domain so we verify here that the request host is allowed.
        if !self.matches_hostname(hostname) {
            return Ok(None);
        }

        // Check Gateway/Listener constraints (sectionName + hostname isolation)
        let matched_gi = if let Some(ref parent_refs) = self.route_info.parent_refs {
            match check_gateway_listener_match(parent_refs, index, hostname, &self.matched_info.rsc.ns, "GRPCRoute") {
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

    /// Match gRPC header.
    ///
    /// Controller validates regex patterns at preparse (see ref_grant/validator.rs::check_regex_pattern),
    /// so a missing `compiled_regex` here means controller validation gap or compile-time
    /// invariant violation — fail-closed instead of recompiling on every request.
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
            "RegularExpression" => match compiled_regex {
                Some(regex) => Ok(regex.is_match(header_value)),
                None => Ok(false),
            },
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
        if let Some(ref rule_name) = self.matched_info.rule_name {
            format!(
                "{}/{} (rule:{}[{}], match:{})",
                self.matched_info.rsc.ns,
                self.matched_info.rsc.name,
                self.matched_info.rule_id,
                rule_name,
                self.matched_info.match_id
            )
        } else {
            format!(
                "{}/{} (rule:{}, match:{})",
                self.matched_info.rsc.ns,
                self.matched_info.rsc.name,
                self.matched_info.rule_id,
                self.matched_info.match_id
            )
        }
    }
}
