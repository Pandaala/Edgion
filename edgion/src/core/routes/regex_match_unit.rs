use crate::types::err::EdError;
use crate::types::{HTTPRouteMatch, HTTPRouteRule};
use crate::core::routes::match_unit::HttpRouteRuleUnit;
use pingora_proxy::Session;
use regex::Regex;
use std::sync::Arc;

/// Regex-based route rule unit for RegularExpression path matching
#[derive(Clone)]
pub struct HttpRouteRuleRegexUnit {
    pub namespace: String,
    pub name: String,
    pub resource_key: String,
    /// Single match item from the rule's matches
    pub match_item: HTTPRouteMatch,
    /// Reference to the original rule (for backend_refs, filters, etc.)
    pub rule: Arc<HTTPRouteRule>,
    /// Compiled regex for path matching
    pub path_regex: Regex,
}

impl HttpRouteRuleRegexUnit {
    pub fn new(
        namespace: String,
        name: String,
        resource_key: String,
        match_item: HTTPRouteMatch,
        rule: Arc<HTTPRouteRule>,
        path_regex: Regex,
    ) -> HttpRouteRuleRegexUnit {
        Self {
            namespace,
            name,
            resource_key,
            match_item,
            rule,
            path_regex,
        }
    }
    
    /// Try to match the request path against the regex pattern
    pub fn matches_path(&self, path: &str) -> bool {
        self.path_regex.is_match(path)
    }
    
    /// Perform deep match (headers, query params, method)
    /// Similar to HttpRouteRuleUnit::deep_match but without path matching (already done by regex)
    pub fn deep_match(&self, session: &Session) -> Result<bool, EdError> {
        let req_header = session.req_header();
        let method = req_header.method.as_str();
        
        let query_params = req_header.uri.query()
            .map(|q| HttpRouteRuleUnit::parse_query_string(q))
            .unwrap_or_default();
        
        // Check HTTP Method
        if let Some(match_method) = &self.match_item.method {
            if method != match_method.as_str() {
                tracing::trace!(method=%method,expected=%match_method,"method mismatch");
                return Ok(false);
            }
        }
        
        // Check Headers
        if let Some(header_matches) = &self.match_item.headers {
            for header_match in header_matches {
                if !HttpRouteRuleUnit::match_header(req_header, header_match)? {
                    tracing::trace!(header=%header_match.name,"header mismatch");
                    return Ok(false);
                }
            }
        }
        
        // Check Query Parameters
        if let Some(query_param_matches) = &self.match_item.query_params {
            for query_param_match in query_param_matches {
                if !HttpRouteRuleUnit::match_query_param(&query_params, query_param_match)? {
                    tracing::trace!(param=%query_param_match.name,"query param mismatch");
                    return Ok(false);
                }
            }
        }
        
        tracing::debug!(route=?self.identifier(),"regex deep match ok");
        Ok(true)
    }
    
    pub fn identifier(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

