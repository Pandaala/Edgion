use super::match_engine::RouteEntry;
use crate::types::err::EdError;
use crate::types::{HTTPRouteMatch, HTTPRouteRule};
use pingora_proxy::Session;
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct HttpRouteRuleUnit {
    pub namespace: String,
    pub name: String,
    pub resource_key: String,
    /// Single match item from the rule's matches
    pub match_item: HTTPRouteMatch,
    /// Reference to the original rule (for backend_refs, filters, etc.)
    pub rule: Arc<HTTPRouteRule>,
}

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

impl HttpRouteRuleUnit {
    pub fn new(
        namespace: String,
        name: String,
        resource_key: String,
        match_item: HTTPRouteMatch,
        rule: Arc<HTTPRouteRule>,
    ) -> HttpRouteRuleUnit {
        Self {
            namespace,
            name,
            resource_key,
            match_item,
            rule,
        }
    }
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

/// Matched route result - can be either a normal route or a regex route
#[derive(Clone)]
pub enum MatchedRoute {
    Normal(Arc<HttpRouteRuleUnit>),
    Regex(HttpRouteRuleRegexUnit),
}

impl MatchedRoute {
    /// Get the identifier (namespace/name) of the matched route
    pub fn identifier(&self) -> String {
        match self {
            Self::Normal(unit) => unit.identifier(),
            Self::Regex(unit) => unit.identifier(),
        }
    }
    
    /// Get the rule associated with the matched route
    pub fn rule(&self) -> &Arc<HTTPRouteRule> {
        match self {
            Self::Normal(unit) => &unit.rule,
            Self::Regex(unit) => &unit.rule,
        }
    }
    
    /// Get the namespace of the matched route
    pub fn namespace(&self) -> &str {
        match self {
            Self::Normal(unit) => &unit.namespace,
            Self::Regex(unit) => &unit.namespace,
        }
    }
    
    /// Get the name of the matched route
    pub fn name(&self) -> &str {
        match self {
            Self::Normal(unit) => &unit.name,
            Self::Regex(unit) => &unit.name,
        }
    }
    
    /// Get the resource key of the matched route
    pub fn resource_key(&self) -> &str {
        match self {
            Self::Normal(unit) => &unit.resource_key,
            Self::Regex(unit) => &unit.resource_key,
        }
    }
}

impl HttpRouteRuleUnit {
    
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
}

impl RouteEntry for HttpRouteRuleUnit {
    fn extract_paths(&self) -> Vec<(String, bool)> {
        let mut paths = Vec::new();

        // Extract path from the single match_item
        if let Some(path) = &self.match_item.path {
            if let Some(value) = &path.value {
                let is_prefix = path.match_type.as_deref().map(|t| t == "PathPrefix").unwrap_or(false);
                paths.push((value.clone(), is_prefix));
            }
        }

        paths
    }

    fn identifier(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }


    // Host , Path , Header , QueryParam , Method
    fn deep_match(&self, session: &Session) -> Result<bool, EdError> {
        // Get request info from session
        let req_header = session.req_header();
        let method = req_header.method.as_str();
        
        // Parse query parameters from URI (if present)
        let query_params = req_header.uri.query()
            .map(|q| Self::parse_query_string(q))
            .unwrap_or_default();
        
        // Check the single match_item (all conditions must match - AND logic)
        
        // 1. Check HTTP Method (if specified)
        if let Some(match_method) = &self.match_item.method {
            if method != match_method.as_str() {
                tracing::trace!(
                    method = %method,
                    expected = %match_method,
                    route = %self.identifier(),
                    "HTTP method mismatch"
                );
                return Ok(false);
            }
        }
        
        // 2. Check Headers (if specified) - ALL must match (AND logic)
        if let Some(header_matches) = &self.match_item.headers {
            for header_match in header_matches {
                if !Self::match_header(req_header, header_match)? {
                    tracing::trace!(
                        header = %header_match.name,
                        route = %self.identifier(),
                        "Header match failed"
                    );
                    return Ok(false);
                }
            }
        }
        
        // 3. Check Query Parameters (if specified) - ALL must match (AND logic)
        if let Some(query_param_matches) = &self.match_item.query_params {
            for query_param_match in query_param_matches {
                if !Self::match_query_param(&query_params, query_param_match)? {
                    tracing::trace!(
                        param = %query_param_match.name,
                        route = %self.identifier(),
                        "Query parameter match failed"
                    );
                    return Ok(false);
                }
            }
        }
        
        // All conditions matched
        tracing::debug!(
            route = %self.identifier(),
            "Deep match succeeded"
        );
        Ok(true)
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

