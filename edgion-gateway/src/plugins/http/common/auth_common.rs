//! Shared authentication utilities for Edgion request auth plugins.

use crate::plugins::runtime::PluginSessionHeaderExt;
use bytes::Bytes;
use edgion_resources::resources::edgion_plugins::auth_realm::validate_auth_realm;
use edgion_resources::resources::edgion_plugins::ClaimHeaderProjectionPlan;
use edgion_resources::uri_utils::url_decode;
use pingora_http::ResponseHeader;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt::Write;
use std::time::Duration;

use crate::plugins::runtime::PluginSession;

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
    let Some(query) = session.request_query() else {
        return;
    };
    let mut removed = false;
    let remaining: Vec<&str> = query
        .split('&')
        .filter(|pair| {
            if pair.is_empty() {
                return false;
            }
            let raw_key = pair.split('=').next().unwrap_or("");
            let keep = if raw_key.contains(['%', '+']) {
                url_decode(raw_key) != name
            } else {
                raw_key != name
            };
            removed |= !keep;
            keep
        })
        .collect();

    if !removed {
        return;
    }
    let new_query = remaining.join("&");
    let _ = session.set_request_query(&new_query);
}

/// Remove a named cookie from every forwarded `Cookie` header line.
///
/// HTTP/2 may split cookies across multiple fields, so all lines are collected
/// before the survivors are written back as one field — or the header is removed
/// entirely if none survive. Re-serializing all lines into one avoids the data
/// loss that a single-line read + replace-all write would cause when the request
/// carries multiple `Cookie` fields.
///
/// Cookie-name matching is a raw key comparison on the trimmed `name=value`
/// pair (split on the first `=`), mirroring `get_cookie`. The asymmetry with
/// [`strip_query_param`] — decoded key there, raw key here — is deliberate:
/// each side matches the extractor it undoes.
///
/// Assumes `PluginSession::set_request_header_value` has replace-all-lines semantics
/// (today it does — the adapter routes it through `insert_header`). Were it to
/// become append semantics, every surviving cookie would be duplicated.
pub fn strip_cookie(session: &mut dyn PluginSession, name: &str) {
    strip_cookies(session, std::slice::from_ref(&name));
}

/// Remove every cookie named in `names` in a **single** pass over the request's
/// `Cookie` lines, followed by a single header rewrite.
///
/// Calling [`strip_cookie`] once per name would re-scan and re-write the header
/// for each one; the end state is identical, so callers stripping more than one
/// cookie should use this.
pub fn strip_cookies(session: &mut dyn PluginSession, names: &[&str]) {
    // Read borrowed, decide, then mutate: the immutable borrow ends with the
    // plan, which owns its result.
    let plan = plan_cookie_strip(
        session
            .header_all_bytes("cookie")
            .filter_map(|value| std::str::from_utf8(value).ok()),
        names,
    );

    match plan {
        CookieStripPlan::Absent => {}
        CookieStripPlan::RemoveHeader => {
            let _ = session.remove_request_header("Cookie");
        }
        CookieStripPlan::Replace(value) => {
            if let Ok(value) = http::HeaderValue::try_from(value) {
                let _ = set_sensitive_request_header_value(session, "Cookie", value);
            }
        }
    }
}

/// What [`strip_cookies`] should do to the `Cookie` header.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CookieStripPlan {
    /// The request carried no `Cookie` line; nothing to do.
    Absent,
    /// Every pair was stripped; drop the header.
    RemoveHeader,
    /// Rewrite the header to this merged value.
    Replace(String),
}

/// Pure cookie-strip decision over the request's `Cookie` header lines.
///
/// Split out from [`strip_cookies`] so the parsing and merging rules are covered
/// by ordinary unit tests instead of through a session mock. Multiple `Cookie`
/// lines are merged into one, matching the single rewrite the caller performs.
pub(crate) fn plan_cookie_strip<'a>(cookie_lines: impl Iterator<Item = &'a str>, names: &[&str]) -> CookieStripPlan {
    let mut survivors: Vec<&str> = Vec::new();
    let mut saw_cookie = false;

    for line in cookie_lines {
        saw_cookie = true;
        for pair in line.split(';') {
            let trimmed = pair.trim();
            if trimmed.is_empty() {
                continue;
            }
            let key = trimmed.split('=').next().unwrap_or("");
            if !names.contains(&key) {
                survivors.push(trimmed);
            }
        }
    }

    if !saw_cookie {
        CookieStripPlan::Absent
    } else if survivors.is_empty() {
        CookieStripPlan::RemoveHeader
    } else {
        CookieStripPlan::Replace(survivors.join("; "))
    }
}

/// Build and send a 401/403 auth error response for the gateway plane (Pingora).
///
/// Always sets `WWW-Authenticate: <scheme> realm="<realm>"`. When `error_code` is
/// `Some`, the challenge is extended per RFC 6750 §3.1 with `error="<code>"` and
/// `error_description="<body>"` (the body text escaped as a `quoted-string`).
/// Realm quotes and backslashes are escaped; ASCII controls, including HTAB and
/// DEL, are rejected before constructing a response. Configuration validation
/// uses the same realm policy, without echoing the configured value in errors.
///
/// Pass `error_code = None` when the request carried no credentials at all — RFC
/// 6750 §3.1 says a resource server SHOULD NOT include an error code in that case
/// — and for non-Bearer schemes (Basic, ApiKey, ...) that do not define these
/// attributes. Typical Bearer codes: `invalid_token` (token present but invalid),
/// `insufficient_scope` (403), `invalid_request` (malformed request).
///
/// `Connection: close` is intentionally not set — Pingora manages connection lifecycle.
///
/// Admin-plane counterpart: `unified_auth::build_admin_auth_error_response` (returns
/// an axum `Response`; can't be merged — different runtimes and return shapes).
pub async fn send_gateway_auth_error_response(
    session: &mut dyn PluginSession,
    status: u16,
    scheme: &str,
    realm: &str,
    error_code: Option<&str>,
    body: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    validate_auth_realm(realm)?;
    let realm = escape_quoted_string(realm);
    let mut resp = ResponseHeader::build(status, None)?;
    resp.insert_header("Content-Type", "text/plain")?;
    let www_auth = match error_code {
        Some(code) => format!(
            "{} realm=\"{}\", error=\"{}\", error_description=\"{}\"",
            scheme,
            realm,
            code,
            escape_quoted_string(body)
        ),
        None => format!("{} realm=\"{}\"", scheme, realm),
    };
    resp.insert_header("WWW-Authenticate", &www_auth)?;

    session.write_response_header(Box::new(resp), false).await?;
    session
        .write_response_body(Some(Bytes::from(format!("{} {}", status, body))), true)
        .await?;
    session.shutdown().await;
    Ok(())
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
    /// Convert typed claims into a unified JSON object view.
    pub fn to_value(&self) -> serde_json::Value {
        let mut out = serde_json::Map::new();

        if let Some(v) = &self.iss {
            out.insert("iss".to_string(), serde_json::Value::String(v.clone()));
        }
        if let Some(v) = &self.sub {
            out.insert("sub".to_string(), serde_json::Value::String(v.clone()));
        }
        if let Some(v) = &self.aud {
            out.insert("aud".to_string(), v.clone());
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

        if let serde_json::Value::Object(extra) = &self.extra {
            for (k, v) in extra {
                if !out.contains_key(k) {
                    out.insert(k.clone(), v.clone());
                }
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

/// Map claims to upstream request headers with safety checks.
pub fn set_claims_headers(
    session: &mut dyn PluginSession,
    claims: &serde_json::Value,
    plan: &ClaimHeaderProjectionPlan,
    max_header_value_bytes: usize,
    max_total_header_bytes: usize,
) {
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

        if set_sensitive_request_header(session, entry.header_name(), &header_value).is_ok() {
            total_bytes += added_bytes;
        }
    }
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
            if scalar_measure(value).is_none() {
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
