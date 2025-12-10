use super::match_engine::RouteEntry;
use crate::types::err::EdError;
use crate::types::{HTTPRouteMatch, HTTPRouteRule, MatchInfo};
use pingora_proxy::Session;
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct HttpRouteRuleUnit {
    pub resource_key: String,
    /// Match info containing namespace, name and match item
    pub matched_info: Arc<MatchInfo>,
    /// Reference to the original rule (for backend_refs, filters, etc.)
    pub rule: Arc<HTTPRouteRule>,
}

impl HttpRouteRuleUnit {
    pub fn new(
        namespace: String,
        name: String,
        rule_id: usize,
        match_id: usize,
        resource_key: String,
        match_item: HTTPRouteMatch,
        rule: Arc<HTTPRouteRule>,
    ) -> HttpRouteRuleUnit {
        let rule_plugin_runtime = rule.plugin_runtime.clone();
        Self {
            resource_key,
            matched_info: Arc::new(MatchInfo::new(namespace, name, rule_id, match_id, match_item, rule_plugin_runtime)),
            rule,
        }
    }
    
    /// Parse query string (already extracted by Pingora)
    pub(crate) fn parse_query_string(query: &str) -> HashMap<String, String> {
        let mut params = HashMap::new();
        
        if query.is_empty() {
            return params;
        }
        
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            
            if let Some(eq_pos) = pair.find('=') {
                let key = Self::url_decode(&pair[..eq_pos]);
                let value = Self::url_decode(&pair[eq_pos + 1..]);
                params.insert(key, value);
            } else {
                let key = Self::url_decode(pair);
                params.insert(key, String::new());
            }
        }
        
        params
    }
    
    /// Simple URL decode (percent-encoding)
    fn url_decode(s: &str) -> String {
        let mut result = String::new();
        let mut chars = s.chars();
        
        while let Some(c) = chars.next() {
            if c == '%' {
                // Try to decode %XX
                let hex: String = chars.by_ref().take(2).collect();
                if hex.len() == 2 {
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        result.push(byte as char);
                        continue;
                    }
                }
                // If decode failed, keep the % and hex as is
                result.push('%');
                result.push_str(&hex);
            } else if c == '+' {
                result.push(' ');
            } else {
                result.push(c);
            }
        }
        
        result
    }
    
    /// Match HTTP header
    pub(crate) fn match_header(
        req_header: &pingora_http::RequestHeader,
        header_match: &crate::types::HTTPHeaderMatch,
    ) -> Result<bool, EdError> {
        let header_value = match req_header.headers.get(&header_match.name) {
            Some(value) => value.to_str().unwrap_or(""),
            None => return Ok(false),
        };
        
        let match_type = header_match.match_type.as_deref().unwrap_or("Exact");
        
        match match_type {
            "Exact" => Ok(header_value == header_match.value),
            "RegularExpression" => {
                let re = Regex::new(&header_match.value)
                    .map_err(|e| EdError::RouteMatchError(format!("Invalid regex: {}", e)))?;
                Ok(re.is_match(header_value))
            }
            _ => {
                tracing::warn!(
                    match_type = %match_type,
                    "Unsupported header match type, defaulting to Exact"
                );
                Ok(header_value == header_match.value)
            }
        }
    }
    
    /// Match query parameter
    pub(crate) fn match_query_param(
        query_params: &HashMap<String, String>,
        query_param_match: &crate::types::HTTPQueryParamMatch,
    ) -> Result<bool, EdError> {
        let param_value = match query_params.get(&query_param_match.name) {
            Some(value) => value,
            None => return Ok(false),
        };
        
        let match_type = query_param_match.match_type.as_deref().unwrap_or("Exact");
        
        match match_type {
            "Exact" => Ok(param_value == &query_param_match.value),
            "RegularExpression" => {
                let re = Regex::new(&query_param_match.value)
                    .map_err(|e| EdError::RouteMatchError(format!("Invalid regex: {}", e)))?;
                Ok(re.is_match(param_value))
            }
            _ => {
                tracing::warn!(
                    match_type = %match_type,
                    "Unsupported query param match type, defaulting to Exact"
                );
                Ok(param_value == &query_param_match.value)
            }
        }
    }
    
    /// Common deep match logic for checking method, headers, and query parameters
    /// This function is shared between HttpRouteRuleUnit and HttpRouteRuleRegexUnit
    pub(crate) fn deep_match_common(
        match_item: &HTTPRouteMatch,
        req_header: &pingora_http::RequestHeader,
        identifier: &str,
    ) -> Result<bool, EdError> {
        let method = req_header.method.as_str();
        
        // Parse query parameters from URI (if present)
        let query_params = req_header.uri.query()
            .map(|q| Self::parse_query_string(q))
            .unwrap_or_default();
        
        // 1. Check HTTP Method (if specified)
        if let Some(match_method) = &match_item.method {
            if method != match_method.as_str() {
                tracing::trace!(
                    method = %method,
                    expected = %match_method,
                    route = %identifier,
                    "HTTP method mismatch"
                );
                return Ok(false);
            }
        }
        
        // 2. Check Headers (if specified) - ALL must match (AND logic)
        if let Some(header_matches) = &match_item.headers {
            for header_match in header_matches {
                if !Self::match_header(req_header, header_match)? {
                    tracing::trace!(
                        header = %header_match.name,
                        route = %identifier,
                        "Header match failed"
                    );
                    return Ok(false);
                }
            }
        }
        
        // 3. Check Query Parameters (if specified) - ALL must match (AND logic)
        if let Some(query_param_matches) = &match_item.query_params {
            for query_param_match in query_param_matches {
                if !Self::match_query_param(&query_params, query_param_match)? {
                    tracing::trace!(
                        param = %query_param_match.name,
                        route = %identifier,
                        "Query parameter match failed"
                    );
                    return Ok(false);
                }
            }
        }
        
        // All conditions matched
        tracing::debug!(
            route = %identifier,
            "Deep match succeeded"
        );
        Ok(true)
    }
}

impl RouteEntry for HttpRouteRuleUnit {
    fn extract_paths(&self) -> Vec<(String, bool)> {
        let mut paths = Vec::new();

        // Extract path from the single match_item
        if let Some(path) = &self.matched_info.m.path {
            if let Some(value) = &path.value {
                let is_prefix = path.match_type.as_deref().map(|t| t == "PathPrefix").unwrap_or(false);
                paths.push((value.clone(), is_prefix));
            }
        }

        paths
    }

    fn identifier(&self) -> String {
        format!("{}/{}", self.matched_info.rns, self.matched_info.rn)
    }

    // Host , Path , Header , QueryParam , Method
    fn deep_match(&self, session: &Session) -> Result<bool, EdError> {
        let req_header = session.req_header();
        Self::deep_match_common(&self.matched_info.m, req_header, &self.identifier())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_query_string_basic() {
        let query = "name=john&age=30";
        let params = HttpRouteRuleUnit::parse_query_string(query);
        
        assert_eq!(params.len(), 2);
        assert_eq!(params.get("name"), Some(&"john".to_string()));
        assert_eq!(params.get("age"), Some(&"30".to_string()));
    }
    
    #[test]
    fn test_parse_query_string_with_encoding() {
        let query = "q=hello+world&filter=%20test";
        let params = HttpRouteRuleUnit::parse_query_string(query);
        
        assert_eq!(params.get("q"), Some(&"hello world".to_string()));
        assert_eq!(params.get("filter"), Some(&" test".to_string()));
    }
    
    #[test]
    fn test_parse_query_string_no_value() {
        let query = "flag";
        let params = HttpRouteRuleUnit::parse_query_string(query);
        
        assert_eq!(params.len(), 1);
        assert_eq!(params.get("flag"), Some(&String::new()));
    }
    
    #[test]
    fn test_parse_query_string_empty() {
        let query = "";
        let params = HttpRouteRuleUnit::parse_query_string(query);
        
        assert_eq!(params.len(), 0);
    }
    
    #[test]
    fn test_url_decode_basic() {
        assert_eq!(HttpRouteRuleUnit::url_decode("hello"), "hello");
        assert_eq!(HttpRouteRuleUnit::url_decode("hello+world"), "hello world");
        assert_eq!(HttpRouteRuleUnit::url_decode("hello%20world"), "hello world");
    }
    
    #[test]
    fn test_url_decode_special_chars() {
        assert_eq!(HttpRouteRuleUnit::url_decode("a%2Bb%3Dc"), "a+b=c");
        assert_eq!(HttpRouteRuleUnit::url_decode("100%25"), "100%");
    }
    
    #[test]
    fn test_url_decode_invalid_encoding() {
        // Invalid hex sequences should be kept as-is
        assert_eq!(HttpRouteRuleUnit::url_decode("test%"), "test%");
        assert_eq!(HttpRouteRuleUnit::url_decode("test%GG"), "test%GG");
    }
}

