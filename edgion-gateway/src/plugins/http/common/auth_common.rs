//! Shared authentication utilities for Edgion request auth plugins.

use bytes::Bytes;
use edgion_resources::resources::edgion_plugins::auth_realm::validate_auth_realm;
use edgion_resources::resources::edgion_plugins::ClaimHeaderProjectionPlan;
use pingora_http::ResponseHeader;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt::Write;
use std::time::Duration;

use crate::plugins::runtime::PluginSession;

/// Prepare immutable authentication metadata while preserving source iteration order.
/// Keep raw names for the live setter's validation and mutation policy.
pub(crate) fn prepare_sensitive_headers(
    headers: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, http::HeaderValue)> {
    headers
        .into_iter()
        .filter_map(|(name, value)| {
            if name.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0')
                || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0')
            {
                return None;
            }
            let mut value = http::HeaderValue::from_str(&value).ok()?;
            value.set_sensitive(true);
            Some((name, value))
        })
        .collect()
}

/// Materialize an authentication credential or identity without losing its sensitivity.
pub fn set_sensitive_request_header(
    session: &mut dyn PluginSession,
    name: &str,
    value: &str,
) -> crate::plugins::runtime::PluginSessionResult<()> {
    set_sensitive_request_header_value(session, name, http::HeaderValue::from_str(value)?)
}

fn set_sensitive_request_header_value(
    session: &mut dyn PluginSession,
    name: &str,
    mut value: http::HeaderValue,
) -> crate::plugins::runtime::PluginSessionResult<()> {
    value.set_sensitive(true);
    session.set_request_header_value(name, value)
}

/// Remove a named parameter from the forwarded request query string.
///
/// Surviving pairs retain their original encoding, and only the query is
/// rewritten so the upstream stage still controls raw-vs-normalized path
/// forwarding.
///
/// The parameter name is compared against the percent-decoded key so the match
/// mirrors how `KeyGet::Query` (`get_query_param`) extracted it. The
/// `url_decode` remains load-bearing for names containing `%` or `+`: comparing
/// those names raw would silently stop hiding a credential sent as
/// `api%5Fkey=...`. [`strip_cookie`] deliberately compares raw keys instead —
/// each helper matches the extractor it undoes.
pub fn strip_query_param(session: &mut dyn PluginSession, name: &str) {
    let _ = session.remove_request_query_param(name);
}

/// Remove every occurrence of one raw Cookie name from current headers.
pub fn strip_cookie(
    session: &mut dyn PluginSession,
    name: &str,
) -> Result<crate::request_cookie::CookieEditOutcome, crate::request_cookie::CookieEditError> {
    strip_cookies(session, std::slice::from_ref(&name))
}

/// Remove several Cookie names with one validated synchronous publication.
pub fn strip_cookies(
    session: &mut dyn PluginSession,
    names: &[&str],
) -> Result<crate::request_cookie::CookieEditOutcome, crate::request_cookie::CookieEditError> {
    session.remove_request_cookies(names)
}

/// Use the canonical static plugin-failure path; never emit credential details.
pub(crate) fn cookie_cleanup_failure(
    error: crate::request_cookie::CookieEditError,
) -> edgion_resources::filters::PluginRunningResult {
    edgion_resources::filters::PluginRunningResult::ErrResponse {
        status: match error {
            crate::request_cookie::CookieEditError::Unsanitizable => 400,
            crate::request_cookie::CookieEditError::Internal => 500,
        },
        body: None,
    }
}

/// A generation-owned authentication challenge with only stable fields prepared.
///
/// Invalid direct construction retains a data-free error so the response writer
/// still fails before writing headers or a body. Resource validation remains the
/// normal admission boundary. No request-specific error or description is cached.
pub struct PreparedAuthChallenge {
    base: Result<http::HeaderValue, &'static str>,
}

impl PreparedAuthChallenge {
    pub fn new(scheme: &str, realm: &str) -> Self {
        let base = validate_auth_realm(realm).and_then(|()| {
            let realm = escape_quoted_string(realm);
            let mut base = String::with_capacity(scheme.len() + 9 + realm.len());
            base.push_str(scheme);
            base.push_str(" realm=\"");
            base.push_str(&realm);
            base.push('"');
            http::HeaderValue::try_from(base)
                // Promote Bytes' backing to shared ownership on the cold path.
                // The first response clone then has the same cost as later ones.
                .map(|base| {
                    let shared = base.clone();
                    drop(base);
                    shared
                })
                .map_err(|_| "failed to parse header value")
        });
        Self { base }
    }

    fn challenge(
        &self,
        error_code: Option<&str>,
        body: &str,
    ) -> Result<http::HeaderValue, Box<dyn std::error::Error + Send + Sync>> {
        let base = self.base.as_ref().map_err(|error| *error)?;
        match error_code {
            None => Ok(base.clone()),
            Some(code) => Ok(http::HeaderValue::try_from(build_auth_challenge(
                base.as_bytes(),
                code,
                body,
            ))?),
        }
    }
}

/// Send an auth error with a generation-owned scheme/realm challenge.
///
/// None omits error attributes. Some preserves the caller's error code and uses
/// the escaped body as error_description. Realm validation/escaping occurs once
/// in PreparedAuthChallenge; error-description controls are still stripped per
/// response. Status, body, and connection lifecycle remain request-owned.
pub async fn send_gateway_auth_error_response(
    session: &mut dyn PluginSession,
    status: u16,
    challenge: &PreparedAuthChallenge,
    error_code: Option<&str>,
    body: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let www_auth = challenge.challenge(error_code, body)?;
    let mut resp = ResponseHeader::build(status, None)?;
    resp.insert_header("Content-Type", "text/plain")?;
    resp.insert_header("WWW-Authenticate", www_auth)?;

    session.write_response_header(Box::new(resp), false).await?;
    session
        .write_response_body(Some(build_auth_error_body(status, body)), true)
        .await?;
    session.shutdown().await;
    Ok(())
}

fn build_auth_error_body(status: u16, body: &str) -> Bytes {
    let status_width = status.checked_ilog10().unwrap_or(0) as usize + 1;
    let mut output = String::with_capacity(status_width + 1 + body.len());
    let _ = write!(output, "{status} {body}");
    Bytes::from(output)
}

/// Add request-specific attributes without revalidating or escaping the realm.
fn build_auth_challenge(base: &[u8], code: &str, body: &str) -> Vec<u8> {
    const ERROR_PREFIX: &[u8] = b", error=\"";
    const DESCRIPTION_PREFIX: &[u8] = b", error_description=\"";
    let description = escape_quoted_string(body);
    let capacity = base.len() + ERROR_PREFIX.len() + code.len() + 1 + DESCRIPTION_PREFIX.len() + description.len() + 1;
    let mut challenge = Vec::with_capacity(capacity);
    challenge.extend_from_slice(base);
    challenge.extend_from_slice(ERROR_PREFIX);
    challenge.extend_from_slice(code.as_bytes());
    challenge.push(b'"');
    challenge.extend_from_slice(DESCRIPTION_PREFIX);
    challenge.extend_from_slice(description.as_bytes());
    challenge.push(b'"');
    challenge
}

/// Build and send a plain (non-`WWW-Authenticate`) error response for the gateway plane.
///
/// Mirrors [`send_gateway_auth_error_response`] but skips the auth-challenge header,
/// so it can be used for 503s, non-auth 403s, and other gateway-side error replies that
/// still need the standard `write_response_header` → `write_response_body` → `shutdown`
/// sequence. The body is sent verbatim — the caller decides the content type and exact
/// payload.
///
/// `Connection: close` is intentionally not set — Pingora manages connection lifecycle.
pub async fn send_gateway_plain_error_response(
    session: &mut dyn PluginSession,
    status: u16,
    content_type: &str,
    body: Bytes,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut resp = ResponseHeader::build(status, None)?;
    resp.insert_header("Content-Type", content_type)?;

    session.write_response_header(Box::new(resp), false).await?;
    session.write_response_body(Some(body), true).await?;
    session.shutdown().await;
    Ok(())
}

/// Escape a string for safe inclusion in an HTTP `quoted-string` (RFC 9110 §5.6.4):
/// backslash-escape `\` and `"`, and drop control characters that are not valid in
/// a header value (prevents header injection from dynamic error messages).
fn escape_quoted_string(value: &str) -> Cow<'_, str> {
    if !value
        .bytes()
        .any(|byte| byte == b'\\' || byte == b'"' || byte.is_ascii_control())
    {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {}
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Apply authentication failure delay to mitigate brute-force / credential-stuffing attacks.
///
/// Call this before returning a 401/403 error response. When `delay_ms` is 0, this is a no-op.
pub async fn apply_auth_failure_delay(delay_ms: u64) {
    if delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
}

/// JWT claims container with standard fields + flexible custom fields.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Claims {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aud: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exp: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iat: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nbf: Option<u64>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

impl Claims {
    /// Move typed claims into a unified JSON object.
    pub fn into_value(self) -> serde_json::Value {
        let mut out = serde_json::Map::new();

        if let Some(v) = self.iss {
            out.insert("iss".to_string(), serde_json::Value::String(v));
        }
        if let Some(v) = self.sub {
            out.insert("sub".to_string(), serde_json::Value::String(v));
        }
        if let Some(v) = self.aud {
            out.insert("aud".to_string(), v);
        }
        if let Some(v) = self.exp {
            out.insert(
                "exp".to_string(),
                serde_json::Value::Number(serde_json::Number::from(v)),
            );
        }
        if let Some(v) = self.iat {
            out.insert(
                "iat".to_string(),
                serde_json::Value::Number(serde_json::Number::from(v)),
            );
        }
        if let Some(v) = self.nbf {
            out.insert(
                "nbf".to_string(),
                serde_json::Value::Number(serde_json::Number::from(v)),
            );
        }

        if let serde_json::Value::Object(extra) = self.extra {
            for (k, v) in extra {
                out.entry(k).or_insert(v);
            }
        }

        serde_json::Value::Object(out)
    }
}

/// Resolve a dot-notation path in a JSON value.
///
/// Example: "realm_access.roles" -> value["realm_access"]["roles"].
pub fn resolve_claim_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for key in path.split('.') {
        current = current.get(key)?;
    }
    Some(current)
}

/// Remove every configured projection destination before publishing trusted values.
/// Call this directly for successful paths that have no claims to project.
pub fn clear_claim_headers(
    session: &mut dyn PluginSession,
    plan: &ClaimHeaderProjectionPlan,
) -> crate::plugins::runtime::PluginSessionResult<()> {
    for entry in plan.entries() {
        session.remove_request_header(entry.header_name())?;
    }
    Ok(())
}

/// Fail closed without exposing header names, values, or adapter error details.
pub fn claim_header_failure(
    log: &mut crate::plugins::runtime::PluginLog,
) -> edgion_resources::filters::PluginRunningResult {
    log.push_err("claim header mutation failed");
    edgion_resources::filters::PluginRunningResult::ErrResponse {
        status: 500,
        body: None,
    }
}

/// Replace configured destinations with bounded, trusted claims. Clear all names
/// first so missing/invalid values and an exhausted budget cannot retain input.
pub fn set_claims_headers(
    session: &mut dyn PluginSession,
    claims: &serde_json::Value,
    plan: &ClaimHeaderProjectionPlan,
    max_header_value_bytes: usize,
    max_total_header_bytes: usize,
) -> crate::plugins::runtime::PluginSessionResult<()> {
    clear_claim_headers(session, plan)?;
    project_cleared_claim_headers(session, claims, plan, max_header_value_bytes, max_total_header_bytes)
}

/// Write bounded, trusted claims to destinations already cleared by
/// `clear_claim_headers`. Callers that must clear before other writes use this
/// to avoid a second removal pass.
pub(crate) fn project_cleared_claim_headers(
    session: &mut dyn PluginSession,
    claims: &serde_json::Value,
    plan: &ClaimHeaderProjectionPlan,
    max_header_value_bytes: usize,
    max_total_header_bytes: usize,
) -> crate::plugins::runtime::PluginSessionResult<()> {
    let mut total_bytes: usize = 0;

    for entry in plan.entries() {
        let Some(value) = resolve_claim_path(claims, entry.claim_path()) else {
            continue;
        };
        let remaining_total_bytes = max_total_header_bytes.saturating_sub(total_bytes);
        let remaining_value_bytes = remaining_total_bytes.saturating_sub(entry.header_name().len());
        let header_value = match format_claim_header_value(value, max_header_value_bytes, remaining_value_bytes) {
            Ok(Some(value)) => value,
            Ok(None) => continue,
            Err(AggregateHeaderBudgetExceeded) => break,
        };

        let added_bytes = entry.header_name().len() + header_value.len();

        let header_value = match header_value {
            Cow::Borrowed(value) => http::HeaderValue::from_str(value),
            Cow::Owned(value) => http::HeaderValue::try_from(value),
        };
        if let Ok(value) = header_value {
            set_sensitive_request_header_value(session, entry.header_name(), value)?;
            total_bytes += added_bytes;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct ClaimHeaderValueMeasure {
    bytes: usize,
    has_scalar: bool,
    unsafe_value: bool,
}

#[derive(Debug)]
struct AggregateHeaderBudgetExceeded;

fn scalar_measure(value: &serde_json::Value) -> Option<(usize, bool)> {
    match value {
        serde_json::Value::String(value) => Some((
            value.len(),
            value.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | b'\0')),
        )),
        serde_json::Value::Number(value) => {
            let mut counter = HeaderValueByteCounter::default();
            write!(&mut counter, "{value}").ok()?;
            Some((counter.bytes, false))
        }
        serde_json::Value::Bool(true) => Some((4, false)),
        serde_json::Value::Bool(false) => Some((5, false)),
        _ => None,
    }
}

fn measure_claim_header_value(value: &serde_json::Value) -> Option<ClaimHeaderValueMeasure> {
    if let serde_json::Value::Array(values) = value {
        let mut measure = ClaimHeaderValueMeasure {
            bytes: 0,
            has_scalar: false,
            unsafe_value: false,
        };

        for value in values {
            let Some((scalar_bytes, unsafe_value)) = scalar_measure(value) else {
                continue;
            };
            if measure.has_scalar {
                measure.bytes = measure.bytes.checked_add(1)?;
            }
            measure.bytes = measure.bytes.checked_add(scalar_bytes)?;
            measure.has_scalar = true;
            measure.unsafe_value |= unsafe_value;
        }

        return Some(measure);
    }

    let (bytes, unsafe_value) = scalar_measure(value)?;
    Some(ClaimHeaderValueMeasure {
        bytes,
        has_scalar: true,
        unsafe_value,
    })
}

fn write_scalar(output: &mut impl Write, value: &serde_json::Value) -> std::fmt::Result {
    match value {
        serde_json::Value::String(value) => output.write_str(value),
        serde_json::Value::Number(value) => write!(output, "{value}"),
        serde_json::Value::Bool(value) => write!(output, "{value}"),
        _ => Ok(()),
    }
}

fn materialize_claim_header_value(value: &serde_json::Value, measured_bytes: usize) -> Option<Cow<'_, str>> {
    match value {
        serde_json::Value::String(value) => return Some(Cow::Borrowed(value)),
        serde_json::Value::Bool(true) => return Some(Cow::Borrowed("true")),
        serde_json::Value::Bool(false) => return Some(Cow::Borrowed("false")),
        _ => {}
    }

    let mut output = String::with_capacity(measured_bytes);
    if let serde_json::Value::Array(values) = value {
        let mut has_scalar = false;
        for value in values {
            if !matches!(
                value,
                serde_json::Value::String(_) | serde_json::Value::Number(_) | serde_json::Value::Bool(_)
            ) {
                continue;
            }
            if has_scalar {
                output.push(',');
            }
            write_scalar(&mut output, value).ok()?;
            has_scalar = true;
        }
    } else {
        write_scalar(&mut output, value).ok()?;
    }
    debug_assert_eq!(output.len(), measured_bytes);
    Some(Cow::Owned(output))
}

fn format_claim_header_value(
    value: &serde_json::Value,
    max_value_bytes: usize,
    remaining_aggregate_value_bytes: usize,
) -> Result<Option<Cow<'_, str>>, AggregateHeaderBudgetExceeded> {
    let Some(measure) = measure_claim_header_value(value) else {
        return Ok(None);
    };
    if !measure.has_scalar || measure.bytes == 0 || measure.unsafe_value || measure.bytes > max_value_bytes {
        return Ok(None);
    }
    if measure.bytes > remaining_aggregate_value_bytes {
        return Err(AggregateHeaderBudgetExceeded);
    }

    Ok(materialize_claim_header_value(value, measure.bytes))
}

#[derive(Default)]
struct HeaderValueByteCounter {
    bytes: usize,
}

impl std::fmt::Write for HeaderValueByteCounter {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        self.bytes = self.bytes.checked_add(value.len()).ok_or(std::fmt::Error)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/common/auth_common_test.rs"]
mod tests;
