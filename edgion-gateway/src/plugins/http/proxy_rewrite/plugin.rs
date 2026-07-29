//! Proxy Rewrite plugin implementation
//!
//! Rewrites requests before forwarding to upstream services.
//! Supports URI, Host, Method, and Headers modification.

use async_trait::async_trait;
use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use regex::Regex;
use std::io::{self, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::LazyLock;

use crate::link_sys::template::{evaluate_template_bounded, TemplateEvaluationError};
use crate::plugins::http::common::json_request_headers_are_supported;
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::traits::session::BodyReadError;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    profile_json_value, BodyRequirement, CompiledJsonPatchOperation, CompiledJsonPatchValue, ProxyRewriteConfig,
    StrictJsonError, ABSOLUTE_INSTANCE_MAX_BYTES, DEFAULT_INSTANCE_MAX_BYTES,
};
use edgion_resources::EdgionStatus;

const INVALID_REQUEST_BODY: &str = "Invalid request body";
const UNSUPPORTED_MEDIA_TYPE: &str = "Unsupported media type";
const MAX_MUTATION_WORK: u64 = 4_000_000;

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
    json_body_limit: u64,
}

impl ProxyRewrite {
    /// Compile a ProxyRewrite plugin for immutable runtime publication.
    pub fn create(config: &ProxyRewriteConfig, body_requirement: Option<BodyRequirement>) -> Option<Self> {
        let mut config = config.clone();
        if !config.precompile() {
            return None;
        }
        let json_body_limit = body_requirement
            .as_ref()
            .and_then(BodyRequirement::max_body_size_bytes)
            .and_then(Result::ok)
            .filter(|limit| *limit > 0)
            .unwrap_or(DEFAULT_INSTANCE_MAX_BYTES)
            .min(ABSOLUTE_INSTANCE_MAX_BYTES);

        Some(ProxyRewrite {
            name: "ProxyRewrite".to_string(),
            config,
            json_body_limit,
        })
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
        let Some(rewrite) = &self.config.compiled_json_body else {
            return Ok(());
        };
        let content_types = session.request_header_values("content-type");
        let content_encodings = session.request_header_values("content-encoding");
        if !json_request_headers_are_supported(content_types.as_deref(), content_encodings.as_deref()) {
            plugin_log.push("deny media type");
            return Err(Self::reject(415, Some(UNSUPPORTED_MEDIA_TYPE)));
        }

        let known_bodyless = session.declares_body() == Some(false);
        let body_len = session.request_body_available_len(known_bodyless).map_err(|error| {
            Self::record_body_error(session, &error);
            plugin_log.push_err("body length unavailable");
            Self::reject(500, None)
        })?;
        if body_len > self.json_body_limit {
            session.add_error(EdgionStatus::BodyReadTooLarge);
            plugin_log.push("deny body too large");
            return Err(Self::reject(413, None));
        }

        let body = session.read_request_body(None).await.map_err(|error| {
            Self::record_body_error(session, &error);
            match error {
                BodyReadError::BufferLimitExceeded { .. } | BodyReadError::TooLarge { .. } => {
                    plugin_log.push("deny body too large");
                    Self::reject(413, None)
                }
                _ => {
                    plugin_log.push_err("json body unavailable");
                    Self::reject(500, None)
                }
            }
        })?;
        if body.len() as u64 > self.json_body_limit {
            session.add_error(EdgionStatus::BodyReadTooLarge);
            plugin_log.push("deny body too large");
            return Err(Self::reject(413, None));
        }

        let mut value = edgion_resources::resources::edgion_plugins::parse_json_instance(&body)
            .map_err(|error| match error {
                StrictJsonError::TooLarge => {
                    session.add_error(EdgionStatus::BodyReadTooLarge);
                    plugin_log.push("deny body too large");
                    Self::reject(413, None)
                }
                _ => {
                    plugin_log.push("deny malformed json");
                    Self::reject(400, Some(INVALID_REQUEST_BODY))
                }
            })?
            .into_value();

        let mut work = 0_u64;
        for operation in &rewrite.operations {
            self.apply_json_operation(&mut value, operation, session, &mut work)
                .map_err(|failure| {
                    let (status, body) = match failure {
                        JsonRewriteFailure::BadRequest => {
                            plugin_log.push("deny patch operation");
                            (400, Some(INVALID_REQUEST_BODY))
                        }
                        JsonRewriteFailure::TooLarge => {
                            session.add_error(EdgionStatus::BodyReadTooLarge);
                            plugin_log.push("deny mutation size");
                            (413, None)
                        }
                        JsonRewriteFailure::Internal => {
                            plugin_log.push_err("json patch internal");
                            (500, None)
                        }
                    };
                    Self::reject(status, body)
                })?;
            let profile = profile_json_value(&value).map_err(|_| {
                plugin_log.push("deny mutation shape");
                Self::reject(400, Some(INVALID_REQUEST_BODY))
            })?;
            if profile.text_bytes > self.json_body_limit {
                session.add_error(EdgionStatus::BodyReadTooLarge);
                plugin_log.push("deny mutation size");
                return Err(Self::reject(413, None));
            }
            charge_work(&mut work, profile.node_count.saturating_add(profile.text_bytes)).map_err(|_| {
                plugin_log.push("deny mutation work");
                Self::reject(400, Some(INVALID_REQUEST_BODY))
            })?;
        }

        let body = serialize_json_capped(&value, self.json_body_limit as usize).map_err(|failure| match failure {
            JsonRewriteFailure::TooLarge => {
                session.add_error(EdgionStatus::BodyReadTooLarge);
                plugin_log.push("deny output too large");
                Self::reject(413, None)
            }
            _ => {
                plugin_log.push_err("json serialization failed");
                Self::reject(500, None)
            }
        })?;
        session
            .overwrite_request_body(Bytes::from(body))
            .await
            .map_err(|error| {
                Self::record_body_error(session, &error);
                match error {
                    BodyReadError::BufferLimitExceeded { .. } | BodyReadError::TooLarge { .. } => {
                        plugin_log.push("deny mutation size");
                        Self::reject(413, None)
                    }
                    _ => {
                        plugin_log.push_err("body overwrite failed");
                        Self::reject(500, None)
                    }
                }
            })?;
        plugin_log.push("json body rewrite");
        Ok(())
    }

    fn apply_json_operation(
        &self,
        document: &mut serde_json::Value,
        operation: &CompiledJsonPatchOperation,
        session: &dyn PluginSession,
        work: &mut u64,
    ) -> Result<(), JsonRewriteFailure> {
        use json_patch::{
            AddOperation, CopyOperation, MoveOperation, PatchOperation, RemoveOperation, ReplaceOperation,
        };

        let patch = match operation {
            CompiledJsonPatchOperation::Add { path, value } => {
                validate_add_target(document, path)?;
                PatchOperation::Add(AddOperation {
                    path: path.clone(),
                    value: self.resolve_patch_value(value, session, work)?,
                })
            }
            CompiledJsonPatchOperation::Remove { path } => {
                path.resolve(document).map_err(|_| JsonRewriteFailure::BadRequest)?;
                PatchOperation::Remove(RemoveOperation { path: path.clone() })
            }
            CompiledJsonPatchOperation::Replace { path, value } => {
                path.resolve(document).map_err(|_| JsonRewriteFailure::BadRequest)?;
                PatchOperation::Replace(ReplaceOperation {
                    path: path.clone(),
                    value: self.resolve_patch_value(value, session, work)?,
                })
            }
            CompiledJsonPatchOperation::Move { from, path } => {
                from.resolve(document).map_err(|_| JsonRewriteFailure::BadRequest)?;
                validate_add_target(document, path)?;
                PatchOperation::Move(MoveOperation {
                    from: from.clone(),
                    path: path.clone(),
                })
            }
            CompiledJsonPatchOperation::Copy { from, path } => {
                let source = from.resolve(document).map_err(|_| JsonRewriteFailure::BadRequest)?;
                let source_profile = profile_json_value(source).map_err(|_| JsonRewriteFailure::BadRequest)?;
                charge_work(
                    work,
                    source_profile.node_count.saturating_add(source_profile.text_bytes),
                )?;
                validate_add_target(document, path)?;
                PatchOperation::Copy(CopyOperation {
                    from: from.clone(),
                    path: path.clone(),
                })
            }
            CompiledJsonPatchOperation::Test { path, value } => {
                let expected = self.resolve_patch_value(value, session, work)?;
                let actual = path.resolve(document).map_err(|_| JsonRewriteFailure::BadRequest)?;
                if !rfc_json_equal(actual, &expected, work)? {
                    return Err(JsonRewriteFailure::BadRequest);
                }
                return Ok(());
            }
        };

        charge_work(work, patch.path().as_str().len() as u64)?;
        match catch_unwind(AssertUnwindSafe(|| {
            json_patch::patch_unsafe(document, std::slice::from_ref(&patch))
        })) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(JsonRewriteFailure::BadRequest),
            Err(_) => Err(JsonRewriteFailure::Internal),
        }
    }

    fn resolve_patch_value(
        &self,
        value: &CompiledJsonPatchValue,
        session: &dyn PluginSession,
        work: &mut u64,
    ) -> Result<serde_json::Value, JsonRewriteFailure> {
        let value = match value {
            CompiledJsonPatchValue::Literal(value) => value.clone(),
            CompiledJsonPatchValue::Template(template) => {
                let value =
                    evaluate_template_bounded(template, session, self.json_body_limit as usize).map_err(|error| {
                        match error {
                            TemplateEvaluationError::MissingVariable => JsonRewriteFailure::BadRequest,
                            TemplateEvaluationError::TooLarge => JsonRewriteFailure::TooLarge,
                        }
                    })?;
                serde_json::Value::String(value)
            }
        };
        let profile = profile_json_value(&value).map_err(|_| JsonRewriteFailure::BadRequest)?;
        charge_work(work, profile.node_count.saturating_add(profile.text_bytes))?;
        Ok(value)
    }

    fn reject(status: u16, body: Option<&'static str>) -> PluginRunningResult {
        PluginRunningResult::ErrResponse {
            status,
            body: body.map(str::to_owned),
        }
    }

    fn record_body_error(session: &mut dyn PluginSession, error: &BodyReadError) {
        let status = match error {
            BodyReadError::BufferLimitExceeded { .. } | BodyReadError::TooLarge { .. } => {
                Some(EdgionStatus::BodyReadTooLarge)
            }
            BodyReadError::Timeout => Some(EdgionStatus::BodyReadTimeout),
            BodyReadError::Incomplete => Some(EdgionStatus::BodyReadAborted),
            BodyReadError::Unimplemented => Some(EdgionStatus::BodyReadUnimplemented),
            BodyReadError::StorageFailure => Some(EdgionStatus::BodyCaptureStorageFailed),
            BodyReadError::NotBuffered => None,
        };
        if let Some(status) = status {
            session.add_error(status);
        }
    }
}

fn validate_add_target(
    document: &serde_json::Value,
    path: &json_patch::jsonptr::Pointer,
) -> Result<(), JsonRewriteFailure> {
    let Some((parent, _)) = path.split_back() else {
        return Ok(());
    };
    match parent.resolve(document).map_err(|_| JsonRewriteFailure::BadRequest)? {
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => Ok(()),
        _ => Err(JsonRewriteFailure::BadRequest),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonRewriteFailure {
    BadRequest,
    TooLarge,
    Internal,
}

fn charge_work(work: &mut u64, units: u64) -> Result<(), JsonRewriteFailure> {
    *work = work.checked_add(units).ok_or(JsonRewriteFailure::BadRequest)?;
    if *work > MAX_MUTATION_WORK {
        return Err(JsonRewriteFailure::BadRequest);
    }
    Ok(())
}

fn rfc_json_equal(
    left: &serde_json::Value,
    right: &serde_json::Value,
    work: &mut u64,
) -> Result<bool, JsonRewriteFailure> {
    charge_work(work, 1)?;
    match (left, right) {
        (serde_json::Value::Null, serde_json::Value::Null) => Ok(true),
        (serde_json::Value::Bool(left), serde_json::Value::Bool(right)) => Ok(left == right),
        (serde_json::Value::String(left), serde_json::Value::String(right)) => Ok(left == right),
        (serde_json::Value::Number(left), serde_json::Value::Number(right)) => Ok(json_numbers_equal(left, right)),
        (serde_json::Value::Array(left), serde_json::Value::Array(right)) => {
            if left.len() != right.len() {
                return Ok(false);
            }
            for (left, right) in left.iter().zip(right) {
                if !rfc_json_equal(left, right, work)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
            if left.len() != right.len() {
                return Ok(false);
            }
            for (key, left_value) in left {
                let Some(right_value) = right.get(key) else {
                    return Ok(false);
                };
                if !rfc_json_equal(left_value, right_value, work)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn json_numbers_equal(left: &serde_json::Number, right: &serde_json::Number) -> bool {
    fn integer(number: &serde_json::Number) -> Option<i128> {
        number
            .as_i64()
            .map(i128::from)
            .or_else(|| number.as_u64().map(i128::from))
    }

    fn integer_equals_float(integer: i128, float: f64) -> bool {
        if !float.is_finite() || float.fract() != 0.0 {
            return false;
        }
        if integer < 0 {
            const MIN_I64_AS_F64: f64 = -9_223_372_036_854_775_808.0;
            const I64_UPPER_BOUND_AS_F64: f64 = 9_223_372_036_854_775_808.0;
            float >= MIN_I64_AS_F64 && float < I64_UPPER_BOUND_AS_F64 && i128::from(float as i64) == integer
        } else {
            const U64_UPPER_BOUND_AS_F64: f64 = 18_446_744_073_709_551_616.0;
            float >= 0.0 && float < U64_UPPER_BOUND_AS_F64 && i128::from(float as u64) == integer
        }
    }

    match (integer(left), integer(right)) {
        (Some(left), Some(right)) => left == right,
        (Some(integer), None) => right.as_f64().is_some_and(|float| integer_equals_float(integer, float)),
        (None, Some(integer)) => left.as_f64().is_some_and(|float| integer_equals_float(integer, float)),
        (None, None) => left
            .as_f64()
            .zip(right.as_f64())
            .is_some_and(|(left, right)| left == right),
    }
}

struct CappedJsonWriter {
    bytes: Vec<u8>,
    limit: usize,
    overflowed: bool,
}

impl Write for CappedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|length| length > self.limit)
        {
            self.overflowed = true;
            return Err(io::Error::other("JSON output limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialize_json_capped(value: &serde_json::Value, limit: usize) -> Result<Vec<u8>, JsonRewriteFailure> {
    let mut writer = CappedJsonWriter {
        bytes: Vec::with_capacity(limit.min(8 * 1024)),
        limit,
        overflowed: false,
    };
    match catch_unwind(AssertUnwindSafe(|| serde_json::to_writer(&mut writer, value))) {
        Ok(Ok(())) => Ok(writer.bytes),
        Ok(Err(_)) if writer.overflowed => Err(JsonRewriteFailure::TooLarge),
        Ok(Err(_)) | Err(_) => Err(JsonRewriteFailure::Internal),
    }
}

#[async_trait]
impl RequestFilter for ProxyRewrite {
    fn name(&self) -> &str {
        &self.name
    }

    fn requires_materialized_request_body(&self) -> bool {
        false
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
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
    use edgion_resources::resources::edgion_plugins::{JsonBodyRewrite, JsonLiteralValue, JsonPatchOperation};

    fn plugin(config: &ProxyRewriteConfig) -> ProxyRewrite {
        ProxyRewrite::create(config, None).expect("valid ProxyRewrite config")
    }

    #[tokio::test]
    async fn json_body_patch_is_ordered_and_published_once() {
        let plugin = plugin(&ProxyRewriteConfig {
            json_body: Some(JsonBodyRewrite {
                operations: vec![
                    JsonPatchOperation::Test {
                        path: "/count".to_string(),
                        value: JsonLiteralValue::new(serde_json::json!(1.0)),
                        value_template: None,
                    },
                    JsonPatchOperation::Remove {
                        path: "/secret".to_string(),
                    },
                    JsonPatchOperation::Replace {
                        path: "/name".to_string(),
                        value: JsonLiteralValue::new(serde_json::json!("gateway")),
                        value_template: None,
                    },
                    JsonPatchOperation::Add {
                        path: "/items/-".to_string(),
                        value: JsonLiteralValue::new(serde_json::json!("tail")),
                        value_template: None,
                    },
                    JsonPatchOperation::Copy {
                        from: "/profile".to_string(),
                        path: "/profileCopy".to_string(),
                    },
                    JsonPatchOperation::Move {
                        from: "/name".to_string(),
                        path: "/displayName".to_string(),
                    },
                ],
            }),
            ..Default::default()
        });
        let mut session = MockPluginSession::new();
        session.expect_request_header_values().times(2).returning(|name| {
            Some(match name {
                "content-type" => vec!["application/json".to_string()],
                "content-encoding" => Vec::new(),
                _ => unreachable!("unexpected header"),
            })
        });
        session.expect_declares_body().return_const(Some(true));
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(false))
            .return_const(Ok(88));
        session.expect_read_request_body().returning(|_| {
            Ok(Bytes::from_static(
                br#"{"name":"old","secret":"drop","count":1,"items":["head"],"profile":{"tier":"gold"}}"#,
            ))
        });
        session
            .expect_overwrite_request_body()
            .times(1)
            .withf(|body| {
                body.as_ref()
                    == br#"{"count":1,"displayName":"gateway","items":["head","tail"],"profile":{"tier":"gold"},"profileCopy":{"tier":"gold"}}"#
            })
            .returning(|_| Ok(()));
        let mut log = PluginLog::new("ProxyRewrite");

        assert!(plugin.modify_json_body(&mut session, &mut log).await.is_ok());
        assert!(log.contains("json body rewrite"));
    }

    fn json_session(body: &'static [u8], content_type: &'static str) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session.expect_request_header_values().times(2).returning(move |name| {
            Some(match name {
                "content-type" => vec![content_type.to_string()],
                "content-encoding" => Vec::new(),
                _ => unreachable!("unexpected header"),
            })
        });
        session.expect_declares_body().return_const(Some(!body.is_empty()));
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(false))
            .return_const(Ok(body.len() as u64));
        session
            .expect_read_request_body()
            .with(mockall::predicate::eq(None))
            .return_once(move |_| Ok(Bytes::from_static(body)));
        session
    }

    #[tokio::test]
    async fn failed_operation_does_not_publish_partial_patch() {
        let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "replace", "path": "/name", "value": "changed"},
                    {"op": "test", "path": "/version", "value": 2}
                ]
            }
        }))
        .unwrap();
        let plugin = plugin(&config);
        let mut session = json_session(br#"{"name":"original","version":1}"#, "application/json");
        session.expect_overwrite_request_body().times(0);
        let mut log = PluginLog::new("ProxyRewrite");

        assert_eq!(
            plugin.modify_json_body(&mut session, &mut log).await,
            Err(PluginRunningResult::ErrResponse {
                status: 400,
                body: Some(INVALID_REQUEST_BODY.to_string()),
            })
        );
    }

    #[tokio::test]
    async fn value_template_is_a_bounded_json_string_and_missing_is_rejected() {
        let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "add", "path": "/tenant", "valueTemplate": "${header:X-Tenant}"}
                ]
            }
        }))
        .unwrap();
        let plugin = plugin(&config);
        let mut session = json_session(br#"{}"#, "application/problem+json; charset=utf-8");
        session
            .expect_header_value()
            .withf(|name| name == "X-Tenant")
            .return_const(Some(r#"${ctx:not-expanded}"#.to_string()));
        session
            .expect_overwrite_request_body()
            .withf(|body| body.as_ref() == br#"{"tenant":"${ctx:not-expanded}"}"#)
            .return_const(Ok(()));
        let mut log = PluginLog::new("ProxyRewrite");
        assert!(plugin.modify_json_body(&mut session, &mut log).await.is_ok());

        let mut missing = json_session(br#"{}"#, "application/json");
        missing
            .expect_header_value()
            .withf(|name| name == "X-Tenant")
            .return_const(None);
        missing.expect_overwrite_request_body().times(0);
        let mut log = PluginLog::new("ProxyRewrite");
        assert!(matches!(
            plugin.modify_json_body(&mut missing, &mut log).await,
            Err(PluginRunningResult::ErrResponse { status: 400, .. })
        ));
    }

    #[tokio::test]
    async fn media_type_and_output_limits_fail_before_publication() {
        let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "add", "path": "/expanded", "value": "a value too large for this cap"}
                ]
            }
        }))
        .unwrap();
        let plugin = ProxyRewrite::create(
            &config,
            Some(BodyRequirement {
                max_body_size: Some("16".to_string()),
                ..BodyRequirement::fail_close()
            }),
        )
        .unwrap();
        let mut session = json_session(br#"{}"#, "application/json");
        session.expect_add_error().return_const(());
        session.expect_overwrite_request_body().times(0);
        let mut log = PluginLog::new("ProxyRewrite");
        assert!(matches!(
            plugin.modify_json_body(&mut session, &mut log).await,
            Err(PluginRunningResult::ErrResponse { status: 413, .. })
        ));

        let mut media = MockPluginSession::new();
        media.expect_request_header_values().times(2).returning(|name| {
            Some(match name {
                "content-type" => vec!["text/plain".to_string()],
                "content-encoding" => Vec::new(),
                _ => unreachable!("unexpected header"),
            })
        });
        media.expect_read_request_body().times(0);
        media.expect_overwrite_request_body().times(0);
        let mut log = PluginLog::new("ProxyRewrite");
        assert_eq!(
            plugin.modify_json_body(&mut media, &mut log).await,
            Err(PluginRunningResult::ErrResponse {
                status: 415,
                body: Some(UNSUPPORTED_MEDIA_TYPE.to_string()),
            })
        );
    }

    #[test]
    fn rfc_test_numeric_and_structural_equality() {
        let mut work = 0;
        assert!(rfc_json_equal(&serde_json::json!(1), &serde_json::json!(1.0), &mut work).unwrap());
        assert!(!rfc_json_equal(
            &serde_json::json!(9_007_199_254_740_992_u64),
            &serde_json::json!(9_007_199_254_740_993_u64),
            &mut work
        )
        .unwrap());
        assert!(!rfc_json_equal(
            &serde_json::json!(u64::MAX),
            &serde_json::json!(18_446_744_073_709_551_616.0),
            &mut work
        )
        .unwrap());
        assert!(rfc_json_equal(
            &serde_json::json!(10_000_000_000_000_000_000_u64),
            &serde_json::json!(1e19),
            &mut work
        )
        .unwrap());
        assert!(rfc_json_equal(
            &serde_json::json!({"a": 1, "b": [true, null]}),
            &serde_json::json!({"b": [true, null], "a": 1.0}),
            &mut work
        )
        .unwrap());
        assert!(!rfc_json_equal(&serde_json::json!([1, 2]), &serde_json::json!([2, 1]), &mut work).unwrap());
    }

    #[test]
    fn copy_replacing_root_uses_the_final_shape_budget() {
        let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "copy", "from": "/source", "path": ""}
                ]
            }
        }))
        .unwrap();
        let plugin = plugin(&config);
        let source = "s".repeat(10_000);
        let mut document = serde_json::json!({
            "padding": "p".repeat(54_000),
            "source": source,
        });
        let operation = &plugin.config.compiled_json_body.as_ref().unwrap().operations[0];
        let session = MockPluginSession::new();
        let mut work = 0;

        plugin
            .apply_json_operation(&mut document, operation, &session, &mut work)
            .unwrap();
        assert_eq!(document, serde_json::Value::String(source));
    }

    use edgion_resources::resources::edgion_plugins::RegexUri;

    #[tokio::test]
    async fn test_invalid_config() {
        use edgion_resources::resources::edgion_plugins::PluginValidation;
        let mut config = ProxyRewriteConfig {
            regex_uri: Some(RegexUri {
                pattern: r"[invalid".to_string(), // Invalid regex
                replacement: "/test".to_string(),
            }),
            ..Default::default()
        };

        // Verify config validation catches invalid regex
        let is_valid = config.precompile();
        assert!(!is_valid);
        assert!(config.validation_error().unwrap().contains("Invalid regex"));

        // Runtime publication declines an invalid configuration.
        assert!(ProxyRewrite::create(
            &ProxyRewriteConfig {
                uri: None,
                regex_uri: Some(RegexUri {
                    pattern: r"[invalid".to_string(),
                    replacement: "/test".to_string(),
                }),
                ..Default::default()
            },
            None,
        )
        .is_none());
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
        let plugin = plugin(&ProxyRewriteConfig::default());

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
        let plugin = plugin(&ProxyRewriteConfig::default());
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
        let plugin = plugin(&ProxyRewriteConfig::default());
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
        let plugin = plugin(&ProxyRewriteConfig::default());
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
