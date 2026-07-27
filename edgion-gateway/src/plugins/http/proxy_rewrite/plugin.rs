//! Proxy Rewrite plugin implementation
//!
//! Rewrites requests before forwarding to upstream services.
//! Supports URI, Host, Method, and Headers modification.

use async_trait::async_trait;
use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use regex::Regex;
use std::sync::LazyLock;

use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::ProxyRewriteConfig;

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

/// Substitution context for `resolve_variables`.
///
/// Naming the call site's intent (URI vs HTTP header) instead of an opaque bool
/// makes future divergence explicit at the match site.
#[derive(Clone, Copy, Debug)]
enum TemplateContext {
    Uri,
    Header,
}

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

    /// Resolve variable substitution in a template string.
    ///
    /// Supports:
    /// - `$uri` - original request path; never encoded (already a valid path).
    /// - `$arg_<name>` - query parameter value; always URL-encoded.
    /// - `$1-$9` - regex capture group references; always URL-encoded.
    /// - `$<name>` - path parameter from the route pattern (e.g., `$uid` for
    ///   route `/api/:uid`); always URL-encoded.
    ///
    /// All three substitutable branches produce ASCII-safe output regardless of
    /// `TemplateContext`. This guarantees that values are valid both as URI
    /// components and as HTTP header bytes (avoiding `HeaderValue::try_from`
    /// silent rejections and strict upstream rejection of non-ASCII header
    /// values). The `context` parameter is retained as the explicit declaration
    /// site for any future context-specific divergence; today both arms encode.
    ///
    /// Each injected value has its `$` signs escaped before insertion so it
    /// cannot be re-interpreted as a variable reference by a later substitution
    /// step (cross-contamination prevention). The escape sentinel is restored
    /// in the final step.
    fn resolve_variables(
        &self,
        template: &str,
        original_path: &str,
        session: &mut dyn PluginSession,
        captures: Option<&regex::Captures>,
        context: TemplateContext,
    ) -> String {
        let mut result = template.to_string();

        // 1. Replace $uri (not encoded - it's already a valid path).
        //    Escape $ in the injected path so later steps cannot treat it as a variable.
        result = result.replace("$uri", &Self::escape_dollar(original_path));

        // 2. Replace $arg_<name> — always URL-encoded.
        //    `get_query_param` returns the percent-decoded value, but `$arg_<name>`
        //    must always be safe to embed in either a URI or an HTTP header value.
        //    Always re-encoding produces:
        //      - URI context (TemplateContext::Uri): correct percent-encoding without
        //        the prior double-encoding bug for raw `%XX` inputs;
        //      - Header context (TemplateContext::Header): ASCII-safe bytes that survive
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
                    let encoded = match context {
                        // Both contexts encode today. Keeping the match form makes a future
                        // context-specific divergence touch exactly this site.
                        TemplateContext::Uri | TemplateContext::Header => Self::url_encode(value),
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
                    let replacement = match context {
                        TemplateContext::Uri | TemplateContext::Header => Self::url_encode(&value),
                    };
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
            let resolved = self.resolve_variables(new_uri, original_path, session, captures, TemplateContext::Uri);
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
                let result = self.resolve_variables(&result, original_path, session, None, TemplateContext::Uri);
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
                    // Header values: TemplateContext::Header guarantees ASCII-safe output for
                    // $arg_<name>, $1..$9, and $<name>. See resolve_variables doc.
                    let resolved =
                        self.resolve_variables(&entry.value, original_path, session, captures, TemplateContext::Header);
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
                    // Header values: TemplateContext::Header guarantees ASCII-safe output for
                    // $arg_<name>, $1..$9, and $<name>. See resolve_variables doc.
                    let resolved =
                        self.resolve_variables(&entry.value, original_path, session, captures, TemplateContext::Header);
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

    async fn modify_json_body(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> Result<(), PluginRunningResult> {
        let Some(rewrite) = &self.config.json_body else {
            return Ok(());
        };
        let body = session.read_request_body(None).await.map_err(|_| {
            plugin_log.push_err("json body unavailable");
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            }
        })?;
        let mut value: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
            plugin_log.push_err("json body invalid");
            PluginRunningResult::ErrResponse {
                status: 400,
                body: None,
            }
        })?;

        for pointer in &rewrite.remove {
            remove_json_pointer(&mut value, pointer).map_err(|_| {
                plugin_log.push_err("json remove failed");
                PluginRunningResult::ErrResponse {
                    status: 400,
                    body: None,
                }
            })?;
        }
        for (pointer, replacement) in &rewrite.set {
            let target = value.pointer_mut(pointer).ok_or_else(|| {
                plugin_log.push_err("json set failed");
                PluginRunningResult::ErrResponse {
                    status: 400,
                    body: None,
                }
            })?;
            *target = replacement.clone();
        }

        let body = serde_json::to_vec(&value).map_err(|_| PluginRunningResult::ErrResponse {
            status: 500,
            body: None,
        })?;
        session
            .overwrite_request_body(Bytes::from(body))
            .await
            .map_err(|_| PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            })?;
        plugin_log.push("json body rewrite");
        Ok(())
    }
}

fn decode_pointer_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

fn remove_json_pointer(value: &mut serde_json::Value, pointer: &str) -> Result<(), ()> {
    let (parent, leaf) = pointer.rsplit_once('/').ok_or(())?;
    let parent = if parent.is_empty() {
        value
    } else {
        value.pointer_mut(parent).ok_or(())?
    };
    let leaf = decode_pointer_token(leaf);
    match parent {
        serde_json::Value::Object(map) => map.remove(&leaf).map(|_| ()).ok_or(()),
        serde_json::Value::Array(items) => {
            let index = leaf.parse::<usize>().map_err(|_| ())?;
            if index >= items.len() {
                return Err(());
            }
            items.remove(index);
            Ok(())
        }
        _ => Err(()),
    }
}

#[async_trait]
impl RequestFilter for ProxyRewrite {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        use edgion_resources::resources::edgion_plugins::PluginValidation;
        if self.config.validation_error().is_some() {
            plugin_log.push_err("config invalid");
            return PluginRunningResult::GoodNext;
        }

        if let Err(result) = self.modify_json_body(session, plugin_log).await {
            return result;
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

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::edgion_plugins::JsonBodyRewrite;
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn json_body_rewrite_sets_removes_and_overwrites_buffer() {
        let mut set = BTreeMap::new();
        set.insert("/name".to_string(), serde_json::json!("gateway"));
        let plugin = ProxyRewrite::new(&ProxyRewriteConfig {
            json_body: Some(JsonBodyRewrite {
                set,
                remove: vec!["/secret".to_string()],
            }),
            ..Default::default()
        });
        let mut session = MockPluginSession::new();
        session
            .expect_read_request_body()
            .returning(|_| Ok(Bytes::from_static(br#"{"name":"old","secret":"drop"}"#)));
        session
            .expect_overwrite_request_body()
            .withf(|body| body.as_ref() == br#"{"name":"gateway"}"#)
            .returning(|_| Ok(()));
        let mut log = PluginLog::new("ProxyRewrite");

        assert!(plugin.modify_json_body(&mut session, &mut log).await.is_ok());
        assert!(log.contains("json body rewrite"));
    }
    use edgion_resources::resources::edgion_plugins::RegexUri;

    #[tokio::test]
    async fn test_invalid_config() {
        use edgion_resources::resources::edgion_plugins::PluginValidation;
        let mut config = ProxyRewriteConfig {
            uri: None,
            regex_uri: Some(RegexUri {
                pattern: r"[invalid".to_string(), // Invalid regex
                replacement: "/test".to_string(),
            }),
            host: None,
            method: None,
            headers: None,
            json_body: None,
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

        // No mock expectations needed as it returns early
        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

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

        // Mock returns decoded values.
        let mut mock_session = MockPluginSession::new();
        mock_session.expect_get_query_param().returning(|name| match name {
            "user" => Some("user-123".to_string()),      // ordinary ASCII
            "token" => Some("hello\nworld".to_string()), // decoded control byte
            "id" => Some("café".to_string()),            // decoded non-ASCII byte (é = C3 A9) — must be url-encoded
            "raw" => Some("foo+bar=baz".to_string()),    // decoded reserved chars
            _ => None,
        });

        // Header context (TemplateContext::Header).
        let header_out = plugin.resolve_variables(
            "user=$arg_user;token=$arg_token;id=$arg_id;raw=$arg_raw",
            "/orig",
            &mut mock_session,
            None,
            TemplateContext::Header,
        );
        assert_eq!(
            header_out, "user=user-123;token=hello%0Aworld;id=caf%C3%A9;raw=foo%2Bbar%3Dbaz",
            "header context: $arg_<name> must url-encode"
        );

        // URI context (TemplateContext::Uri) — same outcome for $arg_ since we
        // always encode now. Other branches (capture groups, path params) still
        // honour the context; this test only pins down $arg_.
        let uri_out = plugin.resolve_variables(
            "/p?user=$arg_user&token=$arg_token&id=$arg_id&raw=$arg_raw",
            "/orig",
            &mut mock_session,
            None,
            TemplateContext::Uri,
        );
        assert_eq!(
            uri_out, "/p?user=user-123&token=hello%0Aworld&id=caf%C3%A9&raw=foo%2Bbar%3Dbaz",
            "URI context: $arg_<name> stays url-encoded"
        );
    }

    /// `$1..$9` (regex captures) must url-encode in Header context, mirroring
    /// the `$arg_<name>` rule. Without encoding, control bytes and non-ASCII
    /// values cause `HeaderValue::try_from` to silently drop the header.
    #[test]
    fn test_capture_substitution_url_encodes_in_header_context() {
        let plugin = ProxyRewrite::new(&ProxyRewriteConfig::default());
        let mut mock_session = MockPluginSession::new();
        // Captures don't need session lookups, but resolve_variables takes one.
        mock_session.expect_get_query_param().returning(|_| None);
        // Inert: result contains no `$` after capture substitution, so path-param
        // branch is short-circuited by `if result.contains('$')`.
        mock_session.expect_get_path_param().returning(|_| None);

        // Build a fake regex::Captures by matching against a constructed string.
        // The captured text contains: a control byte, a non-ASCII (Latin-1)
        // character, a literal '%', and a reserved char.
        // Use (?s) flag so '.' matches '\n' (the control byte under test).
        let re = regex::Regex::new(r"(?s)^/(.+)$").unwrap();
        // Adversarial input pins the encoder for this function in isolation.
        // Production paths cannot contain a raw \n byte (pingora rejects request-line
        // CTL bytes upstream), so this exercises resolve_variables itself, not an
        // HTTP-realistic capture; the integration test covers the realistic case.
        let probe = "/hello\nworld-é-100%-foo&bar";
        let caps = re.captures(probe).unwrap();

        let out = plugin.resolve_variables(
            "X-Trace=$1",
            "/orig",
            &mut mock_session,
            Some(&caps),
            TemplateContext::Header,
        );
        assert_eq!(
            out, "X-Trace=hello%0Aworld-%C3%A9-100%25-foo%26bar",
            "header context: $1 must url-encode control bytes, non-ASCII, % and reserved chars"
        );
    }

    /// `$<name>` (path parameters) must url-encode in Header context for the
    /// same reason as captures.
    #[test]
    fn test_path_param_substitution_url_encodes_in_header_context() {
        let plugin = ProxyRewrite::new(&ProxyRewriteConfig::default());
        let mut mock_session = MockPluginSession::new();
        mock_session.expect_get_query_param().returning(|_| None);
        // Decoded path-param value: contains non-ASCII, control byte, reserved char.
        mock_session.expect_get_path_param().returning(|name| match name {
            "uid" => Some("café-foo&bar\nbaz".to_string()),
            _ => None,
        });

        let out = plugin.resolve_variables("X-User=$uid", "/orig", &mut mock_session, None, TemplateContext::Header);
        assert_eq!(
            out, "X-User=caf%C3%A9-foo%26bar%0Abaz",
            "header context: $<name> must url-encode the decoded path-param value"
        );
    }

    /// Pin URI-context behavior: `$1..$9` and `$<name>` continue to url-encode
    /// (no behavior change in this context — guards against accidental regression
    /// while we touch resolve_variables).
    #[test]
    fn test_capture_and_path_param_url_encode_in_uri_context() {
        let plugin = ProxyRewrite::new(&ProxyRewriteConfig::default());
        let mut mock_session = MockPluginSession::new();
        mock_session.expect_get_query_param().returning(|_| None);
        mock_session.expect_get_path_param().returning(|name| match name {
            "uid" => Some("café".to_string()),
            _ => None,
        });

        // Capture path
        // Use (?s) flag so '.' matches '\n' for consistency with other capture tests.
        let re = regex::Regex::new(r"(?s)^/(.+)$").unwrap();
        let probe = "/foo&bar";
        let caps = re.captures(probe).unwrap();
        let cap_out = plugin.resolve_variables("/x/$1", "/orig", &mut mock_session, Some(&caps), TemplateContext::Uri);
        assert_eq!(cap_out, "/x/foo%26bar", "URI context: $1 stays url-encoded");

        // Path-param path
        let pp_out = plugin.resolve_variables("/u/$uid", "/orig", &mut mock_session, None, TemplateContext::Uri);
        assert_eq!(pp_out, "/u/caf%C3%A9", "URI context: $<name> stays url-encoded");
    }
}
