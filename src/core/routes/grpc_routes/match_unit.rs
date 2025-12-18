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
        }
    }

    /// Deep match: check headers
    pub fn deep_match(&self, session: &Session) -> Result<bool, EdError> {
        let req_header = session.req_header();

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

