//! Proxy Rewrite plugin implementation
//!
//! Rewrites requests before forwarding to upstream services.
//! Supports URI, Host, Method, and Headers modification.

use async_trait::async_trait;
use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use regex::Regex;
use std::borrow::Cow;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::str::from_utf8;
use std::sync::LazyLock;

use crate::link_sys::template::{evaluate_template_bounded, TemplateEvaluationError};
use crate::plugins::http::common::{json_request_headers_are_supported, CappedWriter};
use crate::plugins::runtime::body_gate::record_body_read_error_status;
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::request_body::BodyReadError;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    profile_json_value, BodyRequirement, CompiledJsonPatchOperation, CompiledJsonPatchValue, ProxyRewriteConfig,
    StrictJsonError, ABSOLUTE_INSTANCE_MAX_BYTES, DEFAULT_INSTANCE_MAX_BYTES,
};
use edgion_resources::EdgionErrorCode;

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
    fn escape_dollar(value: Cow<'_, str>) -> Cow<'_, str> {
        if !value.contains('$') {
            return value;
        }
        Cow::Owned(value.replace('$', "\x00$"))
    }

    /// Restore sentinel sequences produced by `escape_dollar`.
    #[inline]
    fn unescape_dollar(value: String) -> String {
        if !value.contains('\x00') {
            return value;
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
        // 1. Replace $uri (not encoded - it's already a valid path).
        //    Escape $ in the injected path so later steps cannot treat it as a variable.
        let mut result = template.replace("$uri", &Self::escape_dollar(Cow::Borrowed(original_path)));

        // 2. Replace $arg_<name> — always URL-encoded.
        //    `get_query_param_ref` returns the percent-decoded value, but `$arg_<name>`
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
        if let Cow::Owned(replaced) = ARG_PATTERN.replace_all(&result, |caps: &regex::Captures| {
            let param_name = &caps[1];
            let value = session.get_query_param_ref(param_name).unwrap_or_default();
            let encoded = Self::url_encode(value);
            Self::escape_dollar(Cow::Owned(encoded)).into_owned()
        }) {
            result = replaced;
        }

        // 3. Replace capture groups $1-$9.
        //    Escape $ in each capture value so step 4 cannot treat it as $<name>.
        if let Some(caps) = captures {
            if let Cow::Owned(replaced) = CAPTURE_PATTERN.replace_all(&result, |c: &regex::Captures| {
                let idx: usize = c[1].parse().unwrap_or(0);
                let value = caps.get(idx).map(|m| m.as_str()).unwrap_or("");
                let encoded = match context {
                    // Both contexts encode today. Keeping the match form makes a future
                    // context-specific divergence touch exactly this site.
                    TemplateContext::Uri | TemplateContext::Header => Self::url_encode(value),
                };
                Self::escape_dollar(Cow::Owned(encoded)).into_owned()
            }) {
                result = replaced;
            }
        }

        // 4. Replace path parameters $<name> captured at route match.
        // This must be done last, after $uri, $arg_xxx, and $1-$9 are replaced.
        // Skip if the matched name is "uri" (already handled) or starts with "arg_".
        if result.contains('$') {
            let stage_input = result.as_str();
            if let Cow::Owned(replaced) = PATH_PARAM_PATTERN.replace_all(stage_input, |caps: &regex::Captures| {
                let Some(matched) = caps.get(0) else {
                    return Cow::Borrowed("");
                };
                let original = &stage_input[matched.start()..matched.end()];
                let param_name = &caps[1];
                // Injected dollars stay literal, even beside an original token of the same name.
                if (matched.start() > 0 && stage_input.as_bytes()[matched.start() - 1] == b'\0')
                    || param_name == "uri"
                    || param_name.starts_with("arg_")
                {
                    return Cow::Borrowed(original);
                }
                match session.get_path_param_ref(param_name) {
                    Some(value) => Cow::Owned(match context {
                        TemplateContext::Uri | TemplateContext::Header => Self::url_encode(value),
                    }),
                    None => Cow::Borrowed(original),
                }
            }) {
                result = replaced;
            }
        }

        // Restore any escaped $ signs that were produced by earlier substitution steps.
        Self::unescape_dollar(result)
    }

    /// Build final URI with original query string preserved
    fn build_uri_with_query(&self, mut new_path: String, original_query: Option<&str>) -> String {
        if let Some(q) = original_query.filter(|q| !q.is_empty()) {
            new_path.reserve(q.len() + 1);
            new_path.push(if new_path.contains('?') { '&' } else { '?' });
            new_path.push_str(q);
        }
        new_path
    }

    /// Rewrite URI using simple template or regex
    fn rewrite_uri(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        original_path: &str,
        captures: Option<&regex::Captures>,
    ) {
        // uri takes priority over regex_uri
        if let Some(ref new_uri) = self.config.uri {
            let resolved = self.resolve_variables(new_uri, original_path, session, captures, TemplateContext::Uri);
            let final_uri = self.build_uri_with_query(resolved, session.request_query());

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
                let mut result = Cow::Borrowed(regex_uri.replacement.as_str());
                for (i, cap) in caps.iter().enumerate().skip(1) {
                    if let Some(m) = cap {
                        let mut marker = [0u8; 2 + usize::MAX.ilog10() as usize];
                        let mut remaining = &mut marker[..];
                        write!(&mut remaining, "${i}").expect("dollar plus usize decimal text fits in marker buffer");
                        let remaining_len = remaining.len();
                        let written = marker.len() - remaining_len;
                        let marker = from_utf8(&marker[..written]).expect("dollar and decimal digits are valid ASCII");
                        result = Cow::Owned(result.replace(marker, &Self::escape_dollar(Cow::Borrowed(m.as_str()))));
                    }
                }
                let result = self.resolve_variables(&result, original_path, session, None, TemplateContext::Uri);
                let final_uri = self.build_uri_with_query(result, session.request_query());

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
        if !json_request_headers_are_supported(session.request_header_map()) {
            plugin_log.push("deny media type");
            return Err(Self::reject(415, Some(UNSUPPORTED_MEDIA_TYPE)));
        }

        let known_bodyless = session.declares_body() == Some(false);
        let body_len = session.request_body_available_len(known_bodyless).map_err(|error| {
            record_body_read_error_status(session, &error);
            plugin_log.push_err("body length unavailable");
            Self::reject(
                if error == BodyReadError::CapacityExceeded {
                    503
                } else {
                    500
                },
                None,
            )
        })?;
        if body_len > self.json_body_limit {
            session.record_error(EdgionErrorCode::BodyReadTooLarge);
            plugin_log.push("deny body too large");
            return Err(Self::reject(413, None));
        }

        let body = session.read_request_body(None).await.map_err(|error| {
            record_body_read_error_status(session, &error);
            match error {
                BodyReadError::CapacityExceeded => Self::reject(503, None),
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
            session.record_error(EdgionErrorCode::BodyReadTooLarge);
            plugin_log.push("deny body too large");
            return Err(Self::reject(413, None));
        }

        let mut value = edgion_resources::resources::edgion_plugins::parse_json_instance(&body)
            .map_err(|error| match error {
                StrictJsonError::TooLarge => {
                    session.record_error(EdgionErrorCode::BodyReadTooLarge);
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
                            session.record_error(EdgionErrorCode::BodyReadTooLarge);
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
                session.record_error(EdgionErrorCode::BodyReadTooLarge);
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
                session.record_error(EdgionErrorCode::BodyReadTooLarge);
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
                record_body_read_error_status(session, &error);
                match error {
                    BodyReadError::CapacityExceeded => Self::reject(503, None),
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
                    value: self.resolve_patch_value(value, session, work)?.into_owned(),
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
                    value: self.resolve_patch_value(value, session, work)?.into_owned(),
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
                if !rfc_json_equal(actual, expected.as_ref(), work)? {
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

    fn resolve_patch_value<'a>(
        &self,
        value: &'a CompiledJsonPatchValue,
        session: &dyn PluginSession,
        work: &mut u64,
    ) -> Result<Cow<'a, serde_json::Value>, JsonRewriteFailure> {
        let value = match value {
            CompiledJsonPatchValue::Literal(value) => Cow::Borrowed(value),
            CompiledJsonPatchValue::Template(template) => {
                let value =
                    evaluate_template_bounded(template, session, self.json_body_limit as usize).map_err(|error| {
                        match error {
                            TemplateEvaluationError::MissingVariable => JsonRewriteFailure::BadRequest,
                            TemplateEvaluationError::TooLarge => JsonRewriteFailure::TooLarge,
                            TemplateEvaluationError::SecretNotAllowed
                            | TemplateEvaluationError::UnavailableVariable => JsonRewriteFailure::Internal,
                        }
                    })?;
                Cow::Owned(serde_json::Value::String(value))
            }
        };
        let profile = profile_json_value(&value).map_err(|_| JsonRewriteFailure::BadRequest)?;
        charge_work(work, profile.node_count.saturating_add(profile.text_bytes))?;
        Ok(value)
    }

    fn reject(status: u16, body: Option<&'static str>) -> PluginRunningResult {
        PluginRunningResult::ErrResponse {
            status,
            body: body.map(|b| Bytes::from_static(b.as_bytes())),
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
            (MIN_I64_AS_F64..I64_UPPER_BOUND_AS_F64).contains(&float) && i128::from(float as i64) == integer
        } else {
            const U64_UPPER_BOUND_AS_F64: f64 = 18_446_744_073_709_551_616.0;
            (0.0..U64_UPPER_BOUND_AS_F64).contains(&float) && i128::from(float as u64) == integer
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

fn serialize_json_capped(value: &serde_json::Value, limit: usize) -> Result<Vec<u8>, JsonRewriteFailure> {
    let mut writer = CappedWriter::new(limit);
    match catch_unwind(AssertUnwindSafe(|| serde_json::to_writer(&mut writer, value))) {
        Ok(Ok(())) => Ok(writer.into_inner()),
        Ok(Err(_)) if writer.overflowed() => Err(JsonRewriteFailure::TooLarge),
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

        // Retain the original path for URI and later header substitutions
        let original_path = session.get_path().to_string();

        // Try to match regex and get captures (for variable substitution)
        let captures = self
            .config
            .compiled_regex
            .as_ref()
            .and_then(|r| r.captures(&original_path));

        // 1. URI rewrite (preserves query string)
        self.rewrite_uri(session, plugin_log, &original_path, captures.as_ref());

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
#[path = "../../../../tests/unit/plugins/http/proxy_rewrite/plugin_test.rs"]
mod tests;
