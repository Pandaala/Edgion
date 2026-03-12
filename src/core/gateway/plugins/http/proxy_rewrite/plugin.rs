//! Proxy Rewrite plugin implementation
//!
//! Rewrites requests before forwarding to upstream services.
//! Supports URI, Host, Method, and Headers modification.

use async_trait::async_trait;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use regex::Regex;
use std::sync::LazyLock;

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::ProxyRewriteConfig;

/// Regex for matching $arg_<name> variables
static ARG_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$arg_(\w+)").unwrap());

/// Regex for matching capture group variables $1-$9
static CAPTURE_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$(\d)").unwrap());

/// Regex for matching path parameter variables $<name>
/// Matches $ followed by a letter and any word characters, but NOT $uri or $arg_xxx or $1-$9
static PATH_PARAM_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$([a-zA-Z][a-zA-Z0-9_]*)").unwrap());

/// Characters that should be percent-encoded in query parameter values.
/// Based on RFC 3986, we encode everything except unreserved characters.
/// Unreserved: A-Z a-z 0-9 - . _ ~
const QUERY_VALUE_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

/// Proxy Rewrite plugin
///
/// Rewrites requests before forwarding to upstream services.
pub struct ProxyRewrite {
    name: String,
    config: ProxyRewriteConfig,
}

impl ProxyRewrite {
    /// Create a new ProxyRewrite plugin from configuration
    pub fn new(config: &ProxyRewriteConfig) -> Self {
        let mut config = config.clone();
        // Precompile regex pattern and validate
        config.precompile();

        ProxyRewrite {
            name: "ProxyRewrite".to_string(),
            config,
        }
    }

    /// URL encode a string value for safe inclusion in URI query parameter.
    /// Uses RFC 3986 percent-encoding, keeping only unreserved characters unencoded.
    fn url_encode(value: &str) -> String {
        utf8_percent_encode(value, QUERY_VALUE_ENCODE_SET).to_string()
    }

    /// Resolve variable substitution in a template string
    ///
    /// Supports:
    /// - `$uri` - original request path
    /// - `$arg_<name>` - query parameter value (URL encoded in URI context)
    /// - `$1-$9` - regex capture group references (only when captures is provided)
    /// - `$<name>` - path parameter from route pattern (e.g., `$uid` for route `/api/:uid`)
    fn resolve_variables(
        &self,
        template: &str,
        original_path: &str,
        session: &mut dyn PluginSession,
        captures: Option<&regex::Captures>,
        encode_values: bool,
    ) -> String {
        let mut result = template.to_string();

        // 1. Replace $uri (not encoded - it's already a valid path)
        result = result.replace("$uri", original_path);

        // 2. Replace $arg_<name> (optionally URL encoded)
        result = ARG_PATTERN
            .replace_all(&result, |caps: &regex::Captures| {
                let param_name = &caps[1];
                let value = session.get_query_param(param_name).unwrap_or_default();
                if encode_values {
                    Self::url_encode(&value)
                } else {
                    value
                }
            })
            .to_string();

        // 3. Replace capture groups $1-$9
        if let Some(caps) = captures {
            result = CAPTURE_PATTERN
                .replace_all(&result, |c: &regex::Captures| {
                    let idx: usize = c[1].parse().unwrap_or(0);
                    let value = caps.get(idx).map(|m| m.as_str()).unwrap_or("");
                    if encode_values {
                        Self::url_encode(value)
                    } else {
                        value.to_string()
                    }
                })
                .to_string();
        }

        // 4. Replace path parameters $<name> (lazy extraction from route pattern)
        // This must be done last, after $uri, $arg_xxx, and $1-$9 are replaced
        // Skip if the matched name is "uri" (already handled) or starts with "arg_"
        if result.contains('$') {
            // Collect all matches first to avoid borrowing issues
            let matches: Vec<(String, String)> = PATH_PARAM_PATTERN
                .captures_iter(&result)
                .filter_map(|caps| {
                    let full_match = caps.get(0)?.as_str().to_string();
                    let param_name = caps.get(1)?.as_str();
                    // Skip "uri" and anything starting with "arg_" (already handled)
                    if param_name == "uri" || param_name.starts_with("arg_") {
                        return None;
                    }
                    Some((full_match, param_name.to_string()))
                })
                .collect();

            for (full_match, param_name) in matches {
                if let Some(value) = session.get_path_param(&param_name) {
                    let replacement = if encode_values { Self::url_encode(&value) } else { value };
                    result = result.replace(&full_match, &replacement);
                }
            }
        }

        result
    }

    /// Build final URI with original query string preserved
    fn build_uri_with_query(&self, new_path: &str, original_query: Option<&str>) -> String {
        match original_query {
            Some(q) if !q.is_empty() => {
                // Check if new_path already has query string
                if new_path.contains('?') {
                    format!("{}&{}", new_path, q)
                } else {
                    format!("{}?{}", new_path, q)
                }
            }
            _ => new_path.to_string(),
        }
    }

    /// Rewrite URI using simple template or regex
    fn rewrite_uri(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        original_path: &str,
        original_query: Option<&str>,
        captures: Option<&regex::Captures>,
    ) {
        // uri takes priority over regex_uri
        if let Some(ref new_uri) = self.config.uri {
            // Resolve variables in URI template (encode values for safety)
            let resolved = self.resolve_variables(new_uri, original_path, session, captures, true);
            // Preserve original query string
            let final_uri = self.build_uri_with_query(&resolved, original_query);

            if let Err(e) = session.set_upstream_uri(&final_uri) {
                plugin_log.push(&format!("URI rewrite failed: {}; ", e));
            } else {
                plugin_log.push(&format!("URI: {} -> {}; ", original_path, final_uri));
            }
        } else if let Some(ref regex_uri) = self.config.regex_uri {
            if let Some(caps) = captures {
                // Replace capture groups in replacement template
                let mut result = regex_uri.replacement.clone();
                for (i, cap) in caps.iter().enumerate().skip(1) {
                    if let Some(m) = cap {
                        result = result.replace(&format!("${}", i), m.as_str());
                    }
                }
                // Preserve original query string
                let final_uri = self.build_uri_with_query(&result, original_query);

                if let Err(e) = session.set_upstream_uri(&final_uri) {
                    plugin_log.push(&format!("Regex URI rewrite failed: {}; ", e));
                } else {
                    plugin_log.push(&format!("URI(regex): {} -> {}; ", original_path, final_uri));
                }
            } else {
                // Regex configured but path doesn't match - log for debugging
                plugin_log.push(&format!(
                    "URI regex '{}' not matched path '{}'; ",
                    regex_uri.pattern, original_path
                ));
            }
        }
    }

    /// Rewrite Host header
    fn rewrite_host(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        if let Some(ref host) = self.config.host {
            if let Err(e) = session.set_upstream_host(host) {
                plugin_log.push(&format!("Host rewrite failed: {}; ", e));
            } else {
                plugin_log.push(&format!("Host -> {}; ", host));
            }
        }
    }

    /// Rewrite HTTP method
    fn rewrite_method(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        if let Some(ref method) = self.config.method {
            let method_str = method.as_str();
            if let Err(e) = session.set_upstream_method(method_str) {
                plugin_log.push(&format!("Method rewrite failed: {}; ", e));
            } else {
                plugin_log.push(&format!("Method -> {}; ", method_str));
            }
        }
    }

    /// Modify request headers
    ///
    /// Execution order: add -> set -> remove
    fn modify_headers(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        original_path: &str,
        captures: Option<&regex::Captures>,
    ) {
        if let Some(ref headers) = self.config.headers {
            let mut modified = false;

            // Execution order: add -> set -> remove

            // 1. Add headers (append) - process in order
            if let Some(ref add_headers) = headers.add {
                for entry in add_headers {
                    // Don't encode header values
                    let resolved = self.resolve_variables(&entry.value, original_path, session, captures, false);
                    if let Err(e) = session.append_request_header(&entry.name, &resolved) {
                        plugin_log.push(&format!("Header add {} failed: {}; ", entry.name, e));
                    } else {
                        modified = true;
                    }
                }
            }

            // 2. Set headers (overwrite) - process in order
            if let Some(ref set_headers) = headers.set {
                for entry in set_headers {
                    // Don't encode header values
                    let resolved = self.resolve_variables(&entry.value, original_path, session, captures, false);
                    if let Err(e) = session.set_request_header(&entry.name, &resolved) {
                        plugin_log.push(&format!("Header set {} failed: {}; ", entry.name, e));
                    } else {
                        modified = true;
                    }
                }
            }

            // 3. Remove headers
            if let Some(ref remove_headers) = headers.remove {
                for name in remove_headers {
                    if let Err(e) = session.remove_request_header(name) {
                        plugin_log.push(&format!("Header remove {} failed: {}; ", name, e));
                    } else {
                        modified = true;
                    }
                }
            }

            if modified {
                plugin_log.push("Headers modified; ");
            }
        }
    }
}

#[async_trait]
impl RequestFilter for ProxyRewrite {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Check if configuration is valid
        if !self.config.is_valid() {
            if let Some(err) = self.config.get_validation_error() {
                plugin_log.push(&format!("Config error: {}; ", err));
            }
            return PluginRunningResult::GoodNext;
        }

        // Get original path and query for variable substitution and preservation
        let original_path = session.get_path().to_string();
        let original_query = session.get_query();

        // Try to match regex and get captures (for variable substitution)
        let captures = self
            .config
            .compiled_regex
            .as_ref()
            .and_then(|r| r.captures(&original_path));

        // 1. URI rewrite (preserves query string)
        self.rewrite_uri(
            session,
            plugin_log,
            &original_path,
            original_query.as_deref(),
            captures.as_ref(),
        );

        // 2. Host rewrite
        self.rewrite_host(session, plugin_log);

        // 3. Method rewrite
        self.rewrite_method(session, plugin_log);

        // 4. Headers modification (supports variable substitution)
        self.modify_headers(session, plugin_log, &original_path, captures.as_ref());

        // Continue request chain
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;
    use crate::types::resources::edgion_plugins::{HeaderActions, HeaderEntry, HttpMethod, RegexUri};

    fn create_basic_config() -> ProxyRewriteConfig {
        ProxyRewriteConfig {
            uri: Some("/new/path".to_string()),
            regex_uri: None,
            host: Some("backend.svc".to_string()),
            method: None,
            headers: None,
            compiled_regex: None,
            validation_error: None,
        }
    }

    #[tokio::test]
    async fn test_uri_rewrite_with_query_preserved() {
        let config = create_basic_config();
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session.expect_get_path().return_const("/old/path".to_string());
        mock_session
            .expect_get_query()
            .return_const(Some("foo=bar&baz=qux".to_string()));
        mock_session
            .expect_set_upstream_uri()
            .with(mockall::predicate::eq("/new/path?foo=bar&baz=qux"))
            .returning(|_| Ok(()));
        mock_session
            .expect_set_upstream_host()
            .with(mockall::predicate::eq("backend.svc"))
            .returning(|_| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("URI:"));
        assert!(plugin_log.contains("Host ->"));
    }

    #[tokio::test]
    async fn test_uri_rewrite_no_query() {
        let config = create_basic_config();
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session.expect_get_path().return_const("/old/path".to_string());
        mock_session.expect_get_query().return_const(None::<String>);
        mock_session
            .expect_set_upstream_uri()
            .with(mockall::predicate::eq("/new/path"))
            .returning(|_| Ok(()));
        mock_session
            .expect_set_upstream_host()
            .with(mockall::predicate::eq("backend.svc"))
            .returning(|_| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_regex_uri_rewrite_with_query_preserved() {
        let config = ProxyRewriteConfig {
            uri: None,
            regex_uri: Some(RegexUri {
                pattern: r"^/api/v1/users/(\d+)/profile".to_string(),
                replacement: "/user-service/$1".to_string(),
            }),
            host: None,
            method: None,
            headers: None,
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session
            .expect_get_path()
            .return_const("/api/v1/users/123/profile".to_string());
        mock_session
            .expect_get_query()
            .return_const(Some("detail=true".to_string()));
        mock_session
            .expect_set_upstream_uri()
            .with(mockall::predicate::eq("/user-service/123?detail=true"))
            .returning(|_| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("URI(regex):"));
    }

    #[tokio::test]
    async fn test_regex_not_matched_logs() {
        let config = ProxyRewriteConfig {
            uri: None,
            regex_uri: Some(RegexUri {
                pattern: r"^/api/v1/(.*)".to_string(),
                replacement: "/internal/$1".to_string(),
            }),
            host: None,
            method: None,
            headers: None,
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        // Path doesn't match the regex pattern
        mock_session.expect_get_path().return_const("/other/path".to_string());
        mock_session.expect_get_query().return_const(None::<String>);

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        // Should log that regex didn't match
        assert!(plugin_log.contains("not matched"));
    }

    #[tokio::test]
    async fn test_method_rewrite() {
        let config = ProxyRewriteConfig {
            uri: None,
            regex_uri: None,
            host: None,
            method: Some(HttpMethod::Post),
            headers: None,
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session.expect_get_path().return_const("/test".to_string());
        mock_session.expect_get_query().return_const(None::<String>);
        mock_session
            .expect_set_upstream_method()
            .with(mockall::predicate::eq("POST"))
            .returning(|_| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Method -> POST"));
    }

    #[tokio::test]
    async fn test_headers_modification_with_vec() {
        let config = ProxyRewriteConfig {
            uri: None,
            regex_uri: None,
            host: None,
            method: None,
            headers: Some(HeaderActions {
                add: None,
                set: Some(vec![
                    HeaderEntry {
                        name: "X-First".to_string(),
                        value: "1".to_string(),
                    },
                    HeaderEntry {
                        name: "X-Second".to_string(),
                        value: "2".to_string(),
                    },
                ]),
                remove: Some(vec!["X-Debug".to_string()]),
            }),
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session.expect_get_path().return_const("/test".to_string());
        mock_session.expect_get_query().return_const(None::<String>);

        // Headers should be set in order
        let mut seq = mockall::Sequence::new();
        mock_session
            .expect_set_request_header()
            .with(mockall::predicate::eq("X-First"), mockall::predicate::eq("1"))
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        mock_session
            .expect_set_request_header()
            .with(mockall::predicate::eq("X-Second"), mockall::predicate::eq("2"))
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        mock_session
            .expect_remove_request_header()
            .with(mockall::predicate::eq("X-Debug"))
            .returning(|_| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Headers modified"));
    }

    #[tokio::test]
    async fn test_uri_variable_with_special_chars_encoded() {
        let config = ProxyRewriteConfig {
            uri: Some("/search?q=$arg_keyword".to_string()),
            regex_uri: None,
            host: None,
            method: None,
            headers: None,
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session.expect_get_path().return_const("/old".to_string());
        mock_session.expect_get_query().return_const(None::<String>);
        mock_session
            .expect_get_query_param()
            .with(mockall::predicate::eq("keyword"))
            .returning(|_| Some("hello world&foo=bar".to_string()));
        // Special chars should be URL encoded
        mock_session
            .expect_set_upstream_uri()
            .with(mockall::predicate::eq("/search?q=hello%20world%26foo%3Dbar"))
            .returning(|_| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_uri_with_arg_variable() {
        let config = ProxyRewriteConfig {
            uri: Some("/search?q=$arg_keyword".to_string()),
            regex_uri: None,
            host: None,
            method: None,
            headers: None,
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session.expect_get_path().return_const("/old".to_string());
        mock_session.expect_get_query().return_const(None::<String>);
        mock_session
            .expect_get_query_param()
            .with(mockall::predicate::eq("keyword"))
            .returning(|_| Some("test".to_string()));
        mock_session
            .expect_set_upstream_uri()
            .with(mockall::predicate::eq("/search?q=test"))
            .returning(|_| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_uri_variable() {
        let config = ProxyRewriteConfig {
            uri: Some("/prefix$uri/suffix".to_string()),
            regex_uri: None,
            host: None,
            method: None,
            headers: None,
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session.expect_get_path().return_const("/api/users".to_string());
        mock_session.expect_get_query().return_const(None::<String>);
        mock_session
            .expect_set_upstream_uri()
            .with(mockall::predicate::eq("/prefix/api/users/suffix"))
            .returning(|_| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_header_with_capture_variable() {
        let config = ProxyRewriteConfig {
            uri: None,
            regex_uri: Some(RegexUri {
                pattern: r"^/users/(\d+)/(\w+)".to_string(),
                replacement: "/user/$1/$2".to_string(),
            }),
            host: None,
            method: None,
            headers: Some(HeaderActions {
                add: None,
                set: Some(vec![
                    HeaderEntry {
                        name: "X-User-Id".to_string(),
                        value: "$1".to_string(),
                    },
                    HeaderEntry {
                        name: "X-Action".to_string(),
                        value: "$2".to_string(),
                    },
                ]),
                remove: None,
            }),
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session
            .expect_get_path()
            .return_const("/users/456/profile".to_string());
        mock_session.expect_get_query().return_const(None::<String>);
        mock_session
            .expect_set_upstream_uri()
            .with(mockall::predicate::eq("/user/456/profile"))
            .returning(|_| Ok(()));

        // Headers should get capture group values
        mock_session
            .expect_set_request_header()
            .with(mockall::predicate::eq("X-User-Id"), mockall::predicate::eq("456"))
            .returning(|_, _| Ok(()));
        mock_session
            .expect_set_request_header()
            .with(mockall::predicate::eq("X-Action"), mockall::predicate::eq("profile"))
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_invalid_config() {
        let mut config = ProxyRewriteConfig {
            uri: None,
            regex_uri: Some(RegexUri {
                pattern: r"[invalid".to_string(), // Invalid regex
                replacement: "/test".to_string(),
            }),
            host: None,
            method: None,
            headers: None,
            compiled_regex: None,
            validation_error: None,
        };

        // Verify config validation catches invalid regex
        let is_valid = config.precompile();
        assert!(!is_valid);
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("Invalid regex"));

        // Create plugin and run with invalid config
        let plugin = ProxyRewrite::new(&ProxyRewriteConfig {
            uri: None,
            regex_uri: Some(RegexUri {
                pattern: r"[invalid".to_string(),
                replacement: "/test".to_string(),
            }),
            ..Default::default()
        });
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        // With invalid config, the plugin should skip processing but not fail
        // No mock expectations needed as it returns early

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        // Should gracefully continue without error
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_headers_add() {
        let config = ProxyRewriteConfig {
            uri: None,
            regex_uri: None,
            host: None,
            method: None,
            headers: Some(HeaderActions {
                add: Some(vec![HeaderEntry {
                    name: "X-Custom".to_string(),
                    value: "added-value".to_string(),
                }]),
                set: None,
                remove: None,
            }),
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session.expect_get_path().return_const("/test".to_string());
        mock_session.expect_get_query().return_const(None::<String>);
        mock_session
            .expect_append_request_header()
            .with(
                mockall::predicate::eq("X-Custom"),
                mockall::predicate::eq("added-value"),
            )
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Headers modified"));
    }

    #[test]
    fn test_url_encode() {
        assert_eq!(ProxyRewrite::url_encode("hello world"), "hello%20world");
        assert_eq!(ProxyRewrite::url_encode("foo&bar"), "foo%26bar");
        assert_eq!(ProxyRewrite::url_encode("a=b"), "a%3Db");
        assert_eq!(ProxyRewrite::url_encode("test?query"), "test%3Fquery");
        assert_eq!(ProxyRewrite::url_encode("100%"), "100%25");
        assert_eq!(ProxyRewrite::url_encode("a+b"), "a%2Bb");
        assert_eq!(ProxyRewrite::url_encode("normal"), "normal");
    }

    #[tokio::test]
    async fn test_uri_with_path_param_variable() {
        let config = ProxyRewriteConfig {
            uri: Some("/user-service/$uid/profile".to_string()),
            regex_uri: None,
            host: None,
            method: None,
            headers: None,
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session
            .expect_get_path()
            .return_const("/api/123/profile".to_string());
        mock_session.expect_get_query().return_const(None::<String>);
        // Path param extraction: $uid -> 123
        mock_session
            .expect_get_path_param()
            .with(mockall::predicate::eq("uid"))
            .returning(|_| Some("123".to_string()));
        mock_session
            .expect_set_upstream_uri()
            .with(mockall::predicate::eq("/user-service/123/profile"))
            .returning(|_| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("URI:"));
    }

    #[tokio::test]
    async fn test_header_with_path_param_variable() {
        let config = ProxyRewriteConfig {
            uri: None,
            regex_uri: None,
            host: None,
            method: None,
            headers: Some(HeaderActions {
                add: None,
                set: Some(vec![
                    HeaderEntry {
                        name: "X-User-Id".to_string(),
                        value: "$uid".to_string(),
                    },
                    HeaderEntry {
                        name: "X-Action".to_string(),
                        value: "$action".to_string(),
                    },
                ]),
                remove: None,
            }),
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session
            .expect_get_path()
            .return_const("/users/456/edit".to_string());
        mock_session.expect_get_query().return_const(None::<String>);

        // Path param extraction
        mock_session
            .expect_get_path_param()
            .with(mockall::predicate::eq("uid"))
            .returning(|_| Some("456".to_string()));
        mock_session
            .expect_get_path_param()
            .with(mockall::predicate::eq("action"))
            .returning(|_| Some("edit".to_string()));

        // Headers should get path param values
        mock_session
            .expect_set_request_header()
            .with(mockall::predicate::eq("X-User-Id"), mockall::predicate::eq("456"))
            .returning(|_, _| Ok(()));
        mock_session
            .expect_set_request_header()
            .with(mockall::predicate::eq("X-Action"), mockall::predicate::eq("edit"))
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Headers modified"));
    }

    #[tokio::test]
    async fn test_path_param_not_found_leaves_variable() {
        let config = ProxyRewriteConfig {
            uri: Some("/service/$unknown/data".to_string()),
            regex_uri: None,
            host: None,
            method: None,
            headers: None,
            compiled_regex: None,
            validation_error: None,
        };
        let plugin = ProxyRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ProxyRewrite");

        mock_session.expect_get_path().return_const("/test".to_string());
        mock_session.expect_get_query().return_const(None::<String>);
        // Path param not found
        mock_session
            .expect_get_path_param()
            .with(mockall::predicate::eq("unknown"))
            .returning(|_| None);
        // Variable should remain unchanged when param not found
        mock_session
            .expect_set_upstream_uri()
            .with(mockall::predicate::eq("/service/$unknown/data"))
            .returning(|_| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
    }
}
