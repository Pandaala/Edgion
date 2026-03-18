use crate::core::gateway::runtime::matching::route::check_gateway_listener_match;
use crate::core::gateway::runtime::GatewayInfo;
use crate::types::ctx::EdgionHttpContext;
use crate::types::err::EdError;
use crate::types::resources::common::ParentReference;
use crate::types::{HTTPRouteRule, MatchInfo};
use pingora_proxy::Session;
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;

/// Result of a successful route match, bundling the matched route and the
/// gateway/listener context that satisfied the parentRef constraints.
pub struct RouteMatchResult {
    pub route_unit: Arc<HttpRouteRuleUnit>,
    pub matched_gateway: GatewayInfo,
}

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
    /// Pre-compiled regexes for header RegularExpression matchers (aligned with headers vec)
    pub compiled_header_regexes: Vec<Option<Arc<Regex>>>,
    /// Pre-compiled regexes for query param RegularExpression matchers (aligned with query_params vec)
    pub compiled_query_regexes: Vec<Option<Arc<Regex>>>,
}

impl HttpRouteRuleUnit {
    /// Pre-compile RegularExpression regexes for header and query param matchers.
    /// Call once at route creation time; the returned vecs are positionally aligned
    /// with the `headers` / `query_params` slices in `HTTPRouteMatch`.
    pub fn compile_match_regexes(
        match_item: &crate::types::HTTPRouteMatch,
    ) -> (Vec<Option<Arc<Regex>>>, Vec<Option<Arc<Regex>>>) {
        let header_regexes = match_item.headers.as_ref().map_or_else(Vec::new, |headers| {
            headers
                .iter()
                .map(|hm| {
                    if hm.match_type.as_deref() == Some("RegularExpression") {
                        Regex::new(&hm.value).ok().map(|r| Arc::new(r))
                    } else {
                        None
                    }
                })
                .collect()
        });
        let query_regexes = match_item.query_params.as_ref().map_or_else(Vec::new, |params| {
            params
                .iter()
                .map(|qm| {
                    if qm.match_type.as_deref() == Some("RegularExpression") {
                        Regex::new(&qm.value).ok().map(|r| Arc::new(r))
                    } else {
                        None
                    }
                })
                .collect()
        });
        (header_regexes, query_regexes)
    }

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

    /// Perform deep match (headers, query params, method, sectionName/Gateway).
    ///
    /// Returns `Some(GatewayInfo)` of the matched gateway on success, `None` on failure.
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `ctx`: Request context containing hostname and other request info
    /// - `gateway_infos`: All gateway/listener contexts available on this listener
    pub fn deep_match(
        &self,
        session: &Session,
        ctx: &EdgionHttpContext,
        gateway_infos: &[GatewayInfo],
    ) -> Result<Option<GatewayInfo>, EdError> {
        let req_header = session.req_header();
        Self::deep_match_common(
            &self.matched_info,
            req_header,
            &self.parent_refs,
            ctx,
            gateway_infos,
            &self.compiled_header_regexes,
            &self.compiled_query_regexes,
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

    /// Match HTTP header.
    /// `compiled_regex`: pre-compiled regex for this header matcher (if RegularExpression).
    pub(crate) fn match_header(
        req_header: &pingora_http::RequestHeader,
        header_match: &crate::types::HTTPHeaderMatch,
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
                if let Some(re) = compiled_regex {
                    return Ok(re.is_match(header_value));
                }
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

    /// Match query parameter.
    /// `compiled_regex`: pre-compiled regex for this query matcher (if RegularExpression).
    pub(crate) fn match_query_param(
        query_params: &HashMap<String, String>,
        query_param_match: &crate::types::HTTPQueryParamMatch,
        compiled_regex: Option<&Arc<Regex>>,
    ) -> Result<bool, EdError> {
        let param_value = match query_params.get(&query_param_match.name) {
            Some(value) => value,
            None => return Ok(false),
        };

        let match_type = query_param_match.match_type.as_deref().unwrap_or("Exact");

        match match_type {
            "Exact" => Ok(param_value == &query_param_match.value),
            "RegularExpression" => {
                if let Some(re) = compiled_regex {
                    return Ok(re.is_match(param_value));
                }
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

    /// Common deep match logic for checking Gateway/sectionName, method, headers, and query parameters.
    ///
    /// Returns `Some(GatewayInfo)` of the matched gateway on success, `None` on failure.
    pub(crate) fn deep_match_common(
        matched_info: &MatchInfo,
        req_header: &pingora_http::RequestHeader,
        parent_refs: &Option<Vec<ParentReference>>,
        ctx: &EdgionHttpContext,
        gateway_infos: &[GatewayInfo],
        compiled_header_regexes: &[Option<Arc<Regex>>],
        compiled_query_regexes: &[Option<Arc<Regex>>],
    ) -> Result<Option<GatewayInfo>, EdError> {
        let method = req_header.method.as_str();
        let match_item = &matched_info.m;

        let query_params = req_header.uri.query().map(Self::parse_query_string).unwrap_or_default();

        // 0. Check Gateway/Listener constraints (sectionName, hostname, AllowedRoutes)
        let matched_gi = if let Some(ref parent_refs) = parent_refs {
            match check_gateway_listener_match(
                parent_refs,
                gateway_infos,
                &ctx.request_info.hostname,
                &matched_info.rns,
                "HTTPRoute",
                &matched_info.rn,
            ) {
                Some(gi) => gi,
                None => return Ok(None),
            }
        } else {
            return Ok(None);
        };

        // 1. Check HTTP Method (if specified)
        if let Some(match_method) = &match_item.method {
            if method != match_method.as_str() {
                return Ok(None);
            }
        }

        // 2. Check Headers (if specified) - ALL must match (AND logic)
        if let Some(header_matches) = &match_item.headers {
            for (idx, header_match) in header_matches.iter().enumerate() {
                let pre = compiled_header_regexes.get(idx).and_then(|r| r.as_ref());
                if !Self::match_header(req_header, header_match, pre)? {
                    return Ok(None);
                }
            }
        }

        // 3. Check Query Parameters (if specified) - ALL must match (AND logic)
        if let Some(query_param_matches) = &match_item.query_params {
            for (idx, query_param_match) in query_param_matches.iter().enumerate() {
                let pre = compiled_query_regexes.get(idx).and_then(|r| r.as_ref());
                if !Self::match_query_param(&query_params, query_param_match, pre)? {
                    return Ok(None);
                }
            }
        }

        Ok(Some(matched_gi))
    }

    /// Return the number of header matchers in the route's match item.
    ///
    /// Used for specificity-based sorting: rules with more header matchers are more specific
    /// and should be evaluated before rules with fewer header matchers (per Gateway API spec).
    pub fn header_matcher_count(&self) -> usize {
        self.matched_info.m.headers.as_ref().map(|h| h.len()).unwrap_or(0)
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
