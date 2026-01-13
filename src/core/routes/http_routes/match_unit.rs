use crate::core::gateway::gateway::config_store::get_global_gateway_config_store;
use crate::core::gateway::gateway::GatewayInfo;
use crate::types::err::EdError;
use crate::types::resources::common::ParentReference;
use crate::types::{HTTPRouteMatch, HTTPRouteRule, MatchInfo};
use pingora_proxy::Session;
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct HttpRouteRuleUnit {
    pub resource_key: String,
    /// Match info containing namespace, name and match item
    pub matched_info: MatchInfo,
    /// Reference to the original rule (for backend_refs, plugins, etc.)
    pub rule: Arc<HTTPRouteRule>,
    /// Compiled regex for path matching (only for RegularExpression path type)
    pub path_regex: Option<Regex>,
    /// Parent references for sectionName matching
    pub parent_refs: Option<Vec<ParentReference>>,
}

impl HttpRouteRuleUnit {
    /// Check if this is a regex route
    pub fn is_regex_route(&self) -> bool {
        self.path_regex.is_some()
    }

    /// Try to match the request path against the regex pattern (if this is a regex route)
    pub fn matches_path(&self, path: &str) -> bool {
        if let Some(ref regex) = self.path_regex {
            regex.is_match(path)
        } else {
            false
        }
    }

    /// Perform deep match (headers, query params, method, sectionName/Gateway)
    /// For use with regex routes or when called directly
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `gateway_info`: Gateway context containing namespace, name, and optional listener_name
    pub fn deep_match(&self, session: &Session, gateway_info: &GatewayInfo) -> Result<bool, EdError> {
        let req_header = session.req_header();
        Self::deep_match_common(
            &self.matched_info.m,
            req_header,
            &self.identifier(),
            &self.parent_refs,
            gateway_info,
        )
    }

    /// Get route identifier
    pub fn identifier(&self) -> String {
        format!("{}/{}", self.matched_info.rns, self.matched_info.rn)
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

    /// Common deep match logic for checking method, headers, query parameters, and Gateway/sectionName
    ///
    /// This function supports two-layer lookup strategy:
    /// - If route has sectionName: match against listener_map
    /// - If route has no sectionName: match against host_map (by request hostname)
    ///
    /// This function is shared between HttpRouteRuleUnit and HttpRouteRuleRegexUnit
    pub(crate) fn deep_match_common(
        match_item: &HTTPRouteMatch,
        req_header: &pingora_http::RequestHeader,
        identifier: &str,
        parent_refs: &Option<Vec<ParentReference>>,
        gateway_info: &GatewayInfo,
    ) -> Result<bool, EdError> {
        let method = req_header.method.as_str();

        // Parse query parameters from URI (if present)
        let query_params = req_header.uri.query().map(Self::parse_query_string).unwrap_or_default();

        // 0. Check Gateway and SectionName matching using two-layer strategy
        if let Some(ref parent_refs) = parent_refs {
            let config_store = get_global_gateway_config_store();
            let gateway_ns = gateway_info.namespace_str();

            let matches = parent_refs.iter().any(|pr| {
                // Get parent gateway namespace (fallback to gateway_info's namespace)
                let parent_ns = pr.namespace.as_deref().unwrap_or(gateway_ns);

                // Check if parent reference matches current gateway
                // This should not happen - routes should only be matched against their parent gateway
                if parent_ns != gateway_ns || pr.name != gateway_info.name {
                    tracing::error!(
                        parent_ns = %parent_ns,
                        parent_name = %pr.name,
                        gateway_ns = %gateway_ns,
                        gateway_name = %gateway_info.name,
                        "Route parent reference does not match current gateway - this should not happen"
                    );
                    return false;
                }

                // Two-layer lookup based on sectionName presence
                match (&pr.section_name, &gateway_info.listener_name) {
                    // Route specifies sectionName - must match listener_map exactly
                    (Some(section_name), Some(listener_name)) => {
                        // Direct comparison: route's sectionName must match current listener
                        section_name == listener_name
                    }
                    // Route specifies sectionName but we don't have listener context
                    // (shouldn't normally happen in EdgionHttp, but verify listener exists)
                    (Some(section_name), None) => {
                        config_store.has_listener(parent_ns, &pr.name, section_name)
                    }
                    // Route doesn't specify sectionName - can attach to any listener
                    // Let HTTPRoute's own hostnames config handle hostname filtering
                    (None, _) => true,
                }
            });

            if !matches {
                return Ok(false);
            }
        }

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

    /// Extract all path patterns from this route with their match types
    /// Returns Vec<(path, is_prefix)>
    pub fn extract_paths(&self) -> Vec<(String, bool)> {
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
