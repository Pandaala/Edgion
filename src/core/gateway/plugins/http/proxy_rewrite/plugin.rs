//! Proxy Rewrite plugin implementation
//!
//! Rewrites requests before forwarding to upstream services.
//! Supports URI, Host, Method, and Headers modification.

use async_trait::async_trait;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use regex::Regex;
use std::sync::LazyLock;

use crate::core::gateway::plugins::runtime::log::{la, sensitive};
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

    /// Escape `$` signs in a runtime value so that subsequent variable-substitution
    /// steps do not interpret the value as a variable reference.
    ///
    /// We use the NUL byte (`\x00`) as an internal sentinel because it cannot appear
    /// in well-formed HTTP paths or query parameters.  All escaped `\x00$` sequences
    /// are restored to `$` in the final step of `resolve_variables`.
    #[inline]
    fn escape_dollar(value: &str) -> String {
        if !value.contains('$') {
            return value.to_string();
        }
        value.replace('$', "\x00$")
    }

    /// Restore sentinel sequences produced by `escape_dollar`.
    #[inline]
    fn unescape_dollar(value: &str) -> String {
        if !value.contains('\x00') {
            return value.to_string();
        }
        value.replace("\x00$", "$")
    }

    /// Resolve variable substitution in a template string
    ///
    /// Supports:
    /// - `$uri` - original request path
    /// - `$arg_<name>` - query parameter value (URL encoded in URI context)
    /// - `$1-$9` - regex capture group references (only when captures is provided)
    /// - `$<name>` - path parameter from route pattern (e.g., `$uid` for route `/api/:uid`)
    ///
    /// Each injected value has its `$` signs escaped before insertion so that it cannot
    /// be re-interpreted as a variable reference by a later substitution step (cross-
    /// contamination prevention).
    fn resolve_variables(
        &self,
        template: &str,
        original_path: &str,
        session: &mut dyn PluginSession,
        captures: Option<&regex::Captures>,
        encode_values: bool,
    ) -> String {
        let mut result = template.to_string();

        // 1. Replace $uri (not encoded - it's already a valid path).
        //    Escape $ in the injected path so later steps cannot treat it as a variable.
        result = result.replace("$uri", &Self::escape_dollar(original_path));

        // 2. Replace $arg_<name> — always URL-encoded.
        //    `get_query_param` returns the percent-decoded value, but `$arg_<name>`
        //    must always be safe to embed in either a URI or an HTTP header value.
        //    Always re-encoding produces:
        //      - URI context (encode_values=true): correct percent-encoding without
        //        the prior double-encoding bug for raw `%XX` inputs;
        //      - Header context (encode_values=false): ASCII-safe bytes that survive
        //        `HeaderValue::try_from` even when the query value contains control
        //        bytes (e.g. `%0A`) or non-ASCII (e.g. UTF-8). For ordinary ASCII
        //        values this is identity, so the header rewrite UX is unchanged.
        //    Escape $ in each query param value so step 4 does not re-interpret
        //    them as variable references.
        result = ARG_PATTERN
            .replace_all(&result, |caps: &regex::Captures| {
                let param_name = &caps[1];
                let value = session.get_query_param(param_name).unwrap_or_default();
                let encoded = Self::url_encode(&value);
                Self::escape_dollar(&encoded)
            })
            .to_string();

        // 3. Replace capture groups $1-$9.
        //    Escape $ in each capture value so step 4 cannot treat it as $<name>.
        if let Some(caps) = captures {
            result = CAPTURE_PATTERN
                .replace_all(&result, |c: &regex::Captures| {
                    let idx: usize = c[1].parse().unwrap_or(0);
                    let value = caps.get(idx).map(|m| m.as_str()).unwrap_or("");
                    let encoded = if encode_values {
                        Self::url_encode(value)
                    } else {
                        value.to_string()
                    };
                    Self::escape_dollar(&encoded)
                })
                .to_string();
        }

        // 4. Replace path parameters $<name> (lazy extraction from route pattern).
        // This must be done last, after $uri, $arg_xxx, and $1-$9 are replaced.
        // Skip if the matched name is "uri" (already handled) or starts with "arg_".
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
                    // Path params are the last substitution step; no need to escape.
                    result = result.replace(&full_match, &replacement);
                }
            }
        }

        // Restore any escaped $ signs that were produced by earlier substitution steps.
        Self::unescape_dollar(&result)
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

            if let Err(_e) = session.set_upstream_uri(&final_uri) {
                plugin_log.push_err("uri rewrite fail");
            } else {
                plugin_log.push("path rewrite");
            }
        } else if let Some(ref regex_uri) = self.config.regex_uri {
            if let Some(caps) = captures {
                // Replace capture groups $1-$9 directly (path-safe: no encoding of '/').
                // Escape $ in each capture value so that resolve_variables below does not
                // re-interpret the captured text as a variable reference.
                let mut result = regex_uri.replacement.clone();
                for (i, cap) in caps.iter().enumerate().skip(1) {
                    if let Some(m) = cap {
                        result = result.replace(&format!("${}", i), &Self::escape_dollar(m.as_str()));
                    }
                }
                // Resolve remaining variables ($uri, $arg_name, etc.) with proper encoding
                let result = self.resolve_variables(&result, original_path, session, None, true);
                // Preserve original query string
                let final_uri = self.build_uri_with_query(&result, original_query);

                if let Err(_e) = session.set_upstream_uri(&final_uri) {
                    plugin_log.push_err("uri rewrite fail");
                } else {
                    plugin_log.push("path rewrite");
                }
            } else {
                // Regex configured but path doesn't match - log for debugging
                plugin_log.push("regex not matched");
            }
        }
    }

    /// Rewrite Host header
    fn rewrite_host(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        if let Some(ref host) = self.config.host {
            if let Err(_e) = session.set_upstream_host(host) {
                plugin_log.push_err("host rewrite fail");
            } else {
                plugin_log.push("host rewrite");
            }
        }
    }

    /// Rewrite HTTP method
    fn rewrite_method(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        if let Some(ref method) = self.config.method {
            let method_str = method.as_str();
            if let Err(_e) = session.set_upstream_method(method_str) {
                plugin_log.push_err("method rewrite fail");
            } else {
                plugin_log.push_fmt("method rewrite {}", la![sensitive(method_str)]);
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
                    if let Err(_e) = session.append_request_header(&entry.name, &resolved) {
                        plugin_log.push_err("header add fail");
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
                    if let Err(_e) = session.set_request_header(&entry.name, &resolved) {
                        plugin_log.push_err("header set fail");
                    } else {
                        modified = true;
                    }
                }
            }

            // 3. Remove headers
            if let Some(ref remove_headers) = headers.remove {
                for name in remove_headers {
                    if let Err(_e) = session.remove_request_header(name) {
                        plugin_log.push_err("header remove fail");
                    } else {
                        modified = true;
                    }
                }
            }

            if modified {
                plugin_log.push("header rewrite");
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
        use crate::types::resources::edgion_plugins::PluginValidation;
        // Check if configuration is valid
        if self.config.validation_error().is_some() {
            plugin_log.push_err("config invalid");
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
    use crate::types::resources::edgion_plugins::RegexUri;

    #[tokio::test]
    async fn test_invalid_config() {
        use crate::types::resources::edgion_plugins::PluginValidation;
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
        };

        // Verify config validation catches invalid regex
        let is_valid = config.precompile();
        assert!(!is_valid);
        assert!(config.validation_error().unwrap().contains("Invalid regex"));

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

    /// `$arg_<name>` must always url-encode the (decoded) query value, in both
    /// URI and header contexts. This protects header rewrite from
    /// `HeaderValue::try_from` rejecting control bytes and shields downstream
    /// strict HTTP parsers from non-ASCII bytes, while remaining identity for
    /// ordinary ASCII values (so the common `$arg_x = user-123` UX is unchanged).
    #[test]
    fn test_arg_substitution_always_url_encodes() {
        let plugin = ProxyRewrite::new(&ProxyRewriteConfig::default());

        // Mock returns decoded values (matches the new PluginSession contract).
        let mut mock_session = MockPluginSession::new();
        mock_session.expect_get_query_param().returning(|name| match name {
            "user" => Some("user-123".to_string()),      // ordinary ASCII
            "token" => Some("hello\nworld".to_string()), // decoded control byte
            "id" => Some("café".to_string()),            // decoded non-ASCII byte (é = C3 A9) — must be url-encoded
            "raw" => Some("foo+bar=baz".to_string()),    // decoded reserved chars
            _ => None,
        });

        // Header context (encode_values = false).
        let header_out = plugin.resolve_variables(
            "user=$arg_user;token=$arg_token;id=$arg_id;raw=$arg_raw",
            "/orig",
            &mut mock_session,
            None,
            false,
        );
        assert_eq!(
            header_out, "user=user-123;token=hello%0Aworld;id=caf%C3%A9;raw=foo%2Bbar%3Dbaz",
            "header context: $arg_<name> must url-encode regardless of encode_values=false"
        );

        // URI context (encode_values = true) — same outcome for $arg_ since we
        // always encode now. Other branches (capture groups, path params) still
        // honour encode_values; this test only pins down $arg_.
        let uri_out = plugin.resolve_variables(
            "/p?user=$arg_user&token=$arg_token&id=$arg_id&raw=$arg_raw",
            "/orig",
            &mut mock_session,
            None,
            true,
        );
        assert_eq!(
            uri_out, "/p?user=user-123&token=hello%0Aworld&id=caf%C3%A9&raw=foo%2Bbar%3Dbaz",
            "URI context: $arg_<name> stays url-encoded"
        );
    }
}
