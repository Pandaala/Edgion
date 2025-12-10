use crate::types::err::EdError;
use crate::types::{HTTPRouteMatch, HTTPRouteRule, MatchInfo};
use crate::core::routes::match_unit::HttpRouteRuleUnit;
use pingora_proxy::Session;
use regex::Regex;
use std::sync::Arc;

/// Regex-based route rule unit for RegularExpression path matching
#[derive(Clone)]
pub struct HttpRouteRuleRegexUnit {
    pub resource_key: String,
    /// Match info containing namespace, name and match item
    pub matched_info: Arc<MatchInfo>,
    /// Reference to the original rule (for backend_refs, filters, etc.)
    pub rule: Arc<HTTPRouteRule>,
    /// Compiled regex for path matching
    pub path_regex: Regex,
}

impl HttpRouteRuleRegexUnit {
    pub fn new(
        namespace: String,
        name: String,
        rule_id: usize,
        match_id: usize,
        resource_key: String,
        match_item: HTTPRouteMatch,
        rule: Arc<HTTPRouteRule>,
        path_regex: Regex,
    ) -> HttpRouteRuleRegexUnit {
        let rule_plugin_runtime = rule.plugin_runtime.clone();
        Self {
            resource_key,
            matched_info: Arc::new(MatchInfo::new(namespace, name, rule_id, match_id, match_item, rule_plugin_runtime)),
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
        HttpRouteRuleUnit::deep_match_common(&self.matched_info.m, req_header, &self.identifier())
    }
    
    pub fn identifier(&self) -> String {
        format!("{}/{}", self.matched_info.rns, self.matched_info.rn)
    }
}

