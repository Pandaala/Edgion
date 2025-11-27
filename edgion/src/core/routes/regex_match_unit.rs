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
    /// Uses common deep match logic shared with HttpRouteRuleUnit
    pub fn deep_match(&self, session: &Session) -> Result<bool, EdError> {
        let req_header = session.req_header();
        HttpRouteRuleUnit::deep_match_common(&self.match_item, req_header, &self.identifier())
    }
    
    pub fn identifier(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

