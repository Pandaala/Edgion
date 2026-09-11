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
    let mut value = http::HeaderValue::from_str(value)?;
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
/// `url_decode` call is load-bearing, not a redundant allocation: simplifying it
/// to a raw string compare would silently stop hiding a credential sent as
/// `api%5Fkey=...`. [`strip_cookie`] deliberately compares raw keys instead —
/// each helper matches the extractor it undoes.
pub fn strip_query_param(session: &mut dyn PluginSession, name: &str) {
    let Some(query) = session.get_query() else {
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
            let keep = url_decode(raw_key) != name;
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
            let _ = set_sensitive_request_header(session, "Cookie", &value);
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
mod tests {
    use super::{
        escape_quoted_string, format_claim_header_value, plan_cookie_strip, resolve_claim_path,
        send_gateway_auth_error_response, set_claims_headers, strip_cookie, strip_query_param,
        AggregateHeaderBudgetExceeded, Claims, CookieStripPlan,
    };
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use mockall::Sequence;
    use parking_lot::Mutex;
    use serde_json::json;
    use std::borrow::Cow;
    use std::collections::HashMap;
    use std::sync::Arc;

    use edgion_resources::resources::edgion_plugins::compile_claim_header_projection;

    #[test]
    fn claim_header_array_formatting_is_bounded_and_preserves_scalar_order() {
        let value = json!(["alpha", {"ignored": true}, 7, false]);
        let expected = "alpha,7,false";

        assert_eq!(
            format_claim_header_value(&value, expected.len(), expected.len())
                .unwrap()
                .as_deref(),
            Some(expected)
        );
        assert!(format_claim_header_value(&value, expected.len() - 1, usize::MAX)
            .unwrap()
            .is_none());
    }

    #[test]
    fn claim_header_array_preserves_empty_comma_utf8_and_unsafe_semantics() {
        let value = json!(["", "a,b", "é", 7, false, {"ignored": true}]);
        let expected = ",a,b,é,7,false";
        assert_eq!(
            format_claim_header_value(&value, expected.len(), expected.len())
                .unwrap()
                .as_deref(),
            Some(expected)
        );

        let unsafe_value = json!(["safe", "bad\nvalue"]);
        assert!(
            format_claim_header_value(&unsafe_value, usize::MAX, 1)
                .unwrap()
                .is_none(),
            "unsafe values must skip rather than consume aggregate order"
        );
    }

    #[test]
    fn formatting_distinguishes_per_value_skip_from_aggregate_stop_before_allocation() {
        let value = json!(["abc", "def"]);
        assert!(
            format_claim_header_value(&value, 6, usize::MAX).unwrap().is_none(),
            "per-value overflow must skip the candidate"
        );
        assert!(matches!(
            format_claim_header_value(&value, 7, 6),
            Err(AggregateHeaderBudgetExceeded)
        ));
    }

    #[test]
    fn claim_header_scalar_value_enforces_exact_byte_cap() {
        let value = json!("é");
        assert_eq!(format_claim_header_value(&value, 2, 2).unwrap().as_deref(), Some("é"));
        assert!(format_claim_header_value(&value, 1, usize::MAX).unwrap().is_none());
    }

    #[test]
    fn failed_header_write_does_not_consume_aggregate_budget() {
        let mapping = HashMap::from([
            ("first".to_string(), "x-a".to_string()),
            ("second".to_string(), "x-b".to_string()),
        ]);
        let plan = compile_claim_header_projection(Some(&mapping)).unwrap();
        let claims = json!({"first": "1", "second": "2"});
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();

        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "x-a" && value == "1" && value.is_sensitive())
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Err(std::io::Error::other("injected write failure").into()));
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "x-b" && value == "2" && value.is_sensitive())
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));

        set_claims_headers(&mut session, &claims, &plan, 1, 4);
    }

    #[test]
    fn aggregate_cap_uses_the_plan_order_at_exact_boundary() {
        let mapping = HashMap::from([
            ("second".to_string(), "x-b".to_string()),
            ("first".to_string(), "x-a".to_string()),
        ]);
        let plan = compile_claim_header_projection(Some(&mapping)).unwrap();
        let claims = json!({"first": "1", "second": "2"});
        let mut session = MockPluginSession::new();

        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "x-a" && value == "1" && value.is_sensitive())
            .times(1)
            .returning(|_, _| Ok(()));

        set_claims_headers(&mut session, &claims, &plan, 1, 4);
    }

    #[test]
    fn shuffled_mapping_construction_and_execution_produce_identical_writes() {
        let source = [
            ("claim_c", "X-C", "c"),
            ("claim_a", "X-A", "a"),
            ("claim_b", "X-B", "b"),
        ];

        for rotation in 0..source.len() {
            let mut mapping = HashMap::new();
            let mut claims = serde_json::Map::new();
            for offset in 0..source.len() {
                let (claim, header, value) = source[(rotation + offset) % source.len()];
                mapping.insert(claim.to_string(), header.to_string());
                claims.insert(claim.to_string(), json!(value));
            }

            let plan = compile_claim_header_projection(Some(&mapping)).unwrap();
            let writes = Arc::new(Mutex::new(Vec::new()));
            let captured = writes.clone();
            let mut session = MockPluginSession::new();
            session
                .expect_set_request_header_value()
                .times(3)
                .returning(move |name, value| {
                    assert!(value.is_sensitive());
                    captured
                        .lock()
                        .push((name.to_string(), value.to_str().unwrap().to_string()));
                    Ok(())
                });

            set_claims_headers(&mut session, &serde_json::Value::Object(claims), &plan, 8, 64);
            assert_eq!(
                *writes.lock(),
                vec![
                    ("x-a".to_string(), "a".to_string()),
                    ("x-b".to_string(), "b".to_string()),
                    ("x-c".to_string(), "c".to_string()),
                ]
            );
        }
    }

    #[test]
    fn aggregate_cap_rejects_the_first_candidate_at_plus_one() {
        let mapping = HashMap::from([("first".to_string(), "x-a".to_string())]);
        let plan = compile_claim_header_projection(Some(&mapping)).unwrap();
        let claims = json!({"first": "1"});
        let mut session = MockPluginSession::new();
        session.expect_set_request_header_value().never();

        set_claims_headers(&mut session, &claims, &plan, 1, 3);
    }

    #[test]
    fn invalid_header_value_does_not_consume_aggregate_budget() {
        let mapping = HashMap::from([
            ("unsafe".to_string(), "x-a".to_string()),
            ("safe".to_string(), "x-b".to_string()),
        ]);
        let plan = compile_claim_header_projection(Some(&mapping)).unwrap();
        let claims = json!({"unsafe": "bad\u{7f}", "safe": "2"});
        let mut session = MockPluginSession::new();

        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "x-b" && value == "2" && value.is_sensitive())
            .times(1)
            .returning(|_, _| Ok(()));

        set_claims_headers(&mut session, &claims, &plan, 4, 7);
    }

    #[tokio::test]
    async fn auth_realm_challenges_escape_data_and_preserve_response_contract() {
        for scheme in ["Basic", "ApiKey", "Bearer", "hmac", "ldap", "TLSCert"] {
            for error_code in [None, Some("invalid_token")] {
                for (realm, escaped) in [
                    ("edgion", "edgion"),
                    ("", ""),
                    ("Partner \"API\"", "Partner \\\"API\\\""),
                    ("path\\", "path\\\\"),
                    ("a, b=c", "a, b=c"),
                    ("café €", "café €"),
                ] {
                    let mut session = MockPluginSession::new();
                    let mut sequence = Sequence::new();
                    let mut expected = format!("{scheme} realm=\"{escaped}\"");
                    if error_code.is_some() {
                        expected.push_str(", error=\"invalid_token\", error_description=\"Denied \\\"token\\\"\\\\\"");
                    }
                    session
                        .expect_write_response_header()
                        .withf(move |response, eos| {
                            !eos && response.status.as_u16() == 401
                                && response.headers.get("content-type").unwrap() == "text/plain"
                                && response.headers.get("www-authenticate").unwrap().as_bytes() == expected.as_bytes()
                        })
                        .times(1)
                        .in_sequence(&mut sequence)
                        .returning(|_, _| Ok(()));
                    session
                        .expect_write_response_body()
                        .withf(|body, eos| *eos && body.as_deref() == Some(b"401 Denied \"token\"\\\r\n".as_slice()))
                        .times(1)
                        .in_sequence(&mut sequence)
                        .returning(|_, _| Ok(()));
                    session
                        .expect_shutdown()
                        .times(1)
                        .in_sequence(&mut sequence)
                        .returning(|| {});
                    send_gateway_auth_error_response(
                        &mut session,
                        401,
                        scheme,
                        realm,
                        error_code,
                        "Denied \"token\"\\\r\n",
                    )
                    .await
                    .unwrap();
                }
            }
        }
    }

    #[tokio::test]
    async fn auth_realm_controls_fail_before_any_response_is_written() {
        for byte in (0..=0x1f).chain(std::iter::once(0x7f)) {
            for error_code in [None, Some("invalid_token")] {
                let mut session = MockPluginSession::new();
                session.expect_write_response_header().never();
                session.expect_write_response_body().never();
                session.expect_shutdown().never();
                let realm = format!("private-realm{}suffix", char::from(byte));
                let error = send_gateway_auth_error_response(&mut session, 401, "Bearer", &realm, error_code, "Denied")
                    .await
                    .unwrap_err();
                assert_eq!(
                    error.to_string(),
                    "realm must not contain ASCII control characters (including tabs and DEL)"
                );
            }
        }
    }

    #[test]
    fn auth_realm_plain_values_do_not_allocate_an_escaped_copy() {
        assert!(matches!(escape_quoted_string("API Gateway café"), Cow::Borrowed(_)));
    }

    #[test]
    fn test_escape_quoted_string() {
        // Plain text is unchanged.
        assert_eq!(escape_quoted_string("Token expired"), "Token expired");
        // Backslash and double-quote are backslash-escaped.
        assert_eq!(escape_quoted_string(r#"a"b\c"#), r#"a\"b\\c"#);
        // Control characters (CR/LF/NUL) are dropped to prevent header injection.
        assert_eq!(escape_quoted_string("a\r\nb\0c"), "abc");
    }

    #[test]
    fn test_resolve_claim_path_dot_notation() {
        let claims = json!({
            "sub": "u1",
            "realm_access": {
                "roles": ["admin", "dev"]
            }
        });

        let sub = resolve_claim_path(&claims, "sub").and_then(|v| v.as_str());
        assert_eq!(sub, Some("u1"));

        let roles = resolve_claim_path(&claims, "realm_access.roles")
            .and_then(|v| v.as_array())
            .map(|v| v.len());
        assert_eq!(roles, Some(2));

        assert!(resolve_claim_path(&claims, "realm_access.missing").is_none());
    }

    #[test]
    fn test_claims_to_value_includes_standard_fields() {
        let claims = Claims {
            iss: Some("https://issuer.example.com".to_string()),
            sub: Some("user-1".to_string()),
            aud: Some(json!(["api"])),
            exp: Some(123),
            iat: Some(100),
            nbf: None,
            extra: json!({"email": "u1@example.com"}),
        };

        let value = claims.to_value();
        assert_eq!(
            value.get("iss").and_then(|v| v.as_str()),
            Some("https://issuer.example.com")
        );
        assert_eq!(value.get("sub").and_then(|v| v.as_str()), Some("user-1"));
        assert_eq!(value.get("exp").and_then(|v| v.as_u64()), Some(123));
        assert_eq!(value.get("email").and_then(|v| v.as_str()), Some("u1@example.com"));
    }

    #[test]
    fn strip_query_param_removes_named_keeps_others() {
        let mut session = MockPluginSession::new();
        session
            .expect_get_query()
            .return_const(Some("api_key=secret&foo=bar".to_string()));
        session
            .expect_set_request_query()
            .withf(|query| query == "foo=bar")
            .times(1)
            .returning(|_| Ok(()));

        strip_query_param(&mut session, "api_key");
    }

    #[test]
    fn strip_query_param_only_param_clears_query() {
        let mut session = MockPluginSession::new();
        session
            .expect_get_query()
            .return_const(Some("api_key=secret".to_string()));
        session
            .expect_set_request_query()
            .withf(|query| query.is_empty())
            .times(1)
            .returning(|_| Ok(()));

        strip_query_param(&mut session, "api_key");
    }

    #[test]
    fn strip_query_param_without_match_leaves_query_untouched() {
        let mut session = MockPluginSession::new();
        session.expect_get_query().return_const(Some("a=1&&b=2&".to_string()));
        session.expect_set_request_query().times(0);

        strip_query_param(&mut session, "api_key");
    }

    #[test]
    fn strip_cookie_removes_named_keeps_others() {
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("Cookie", "api_key=secret; other=1")]);
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "Cookie" && value == "other=1" && value.is_sensitive())
            .times(1)
            .returning(|_, _| Ok(()));

        strip_cookie(&mut session, "api_key");
    }

    #[test]
    fn strip_cookie_only_cookie_removes_header() {
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("Cookie", "api_key=secret")]);
        session
            .expect_remove_request_header()
            .withf(|name| name == "Cookie")
            .times(1)
            .returning(|_| Ok(()));

        strip_cookie(&mut session, "api_key");
    }

    #[test]
    fn strip_cookie_merges_multiple_cookie_lines() {
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("cookie", "api_key=secret; a=1"), ("cookie", "b=2")]);
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "Cookie" && value == "a=1; b=2" && value.is_sensitive())
            .times(1)
            .returning(|_, _| Ok(()));

        strip_cookie(&mut session, "api_key");
    }

    // ── plan_cookie_strip: pure logic, no session mock ───────────────────────
    //
    // These cover the parsing / merging / edge-case rules directly. The mock
    // tests above only need to prove the session wiring (read -> decide ->
    // rewrite), not re-derive the semantics.

    fn plan(lines: &[&str], names: &[&str]) -> CookieStripPlan {
        plan_cookie_strip(lines.iter().copied(), names)
    }

    #[test]
    fn plan_cookie_strip_reports_absent_without_any_cookie_line() {
        assert_eq!(plan(&[], &["api_key"]), CookieStripPlan::Absent);
    }

    #[test]
    fn plan_cookie_strip_removes_header_when_nothing_survives() {
        assert_eq!(plan(&["api_key=secret"], &["api_key"]), CookieStripPlan::RemoveHeader);
        // An empty Cookie line is still a present header with no survivors.
        assert_eq!(plan(&[""], &["api_key"]), CookieStripPlan::RemoveHeader);
        assert_eq!(plan(&["  ;  ; "], &["api_key"]), CookieStripPlan::RemoveHeader);
    }

    #[test]
    fn plan_cookie_strip_keeps_unnamed_pairs_and_trims_whitespace() {
        assert_eq!(
            plan(&["api_key=secret;  other=1 ; third=3"], &["api_key"]),
            CookieStripPlan::Replace("other=1; third=3".to_string())
        );
    }

    #[test]
    fn plan_cookie_strip_merges_multiple_cookie_lines_into_one() {
        assert_eq!(
            plan(&["a=1; api_key=secret", "b=2", "api_key=again; c=3"], &["api_key"]),
            CookieStripPlan::Replace("a=1; b=2; c=3".to_string())
        );
    }

    #[test]
    fn plan_cookie_strip_removes_every_named_cookie_in_one_pass() {
        // Equivalent to stripping each name in sequence, but with a single scan.
        assert_eq!(
            plan(
                &[
                    "edgion_oidc_session=secret; app=keep",
                    "edgion_oidc_state=state; theme=dark"
                ],
                &["edgion_oidc_session", "edgion_oidc_state"],
            ),
            CookieStripPlan::Replace("app=keep; theme=dark".to_string())
        );
    }

    #[test]
    fn plan_cookie_strip_matches_the_raw_key_before_the_first_equals() {
        // Mirrors `get_cookie`: the key is the raw text before the first '=',
        // so a percent-encoded or differently-cased name is a different cookie.
        assert_eq!(
            plan(&["API_KEY=1; api%5Fkey=2; api_key=3"], &["api_key"]),
            CookieStripPlan::Replace("API_KEY=1; api%5Fkey=2".to_string())
        );
        // A valueless pair keeps its whole text when it is not a match.
        assert_eq!(
            plan(&["flag; api_key=3"], &["api_key"]),
            CookieStripPlan::Replace("flag".to_string())
        );
        // ...and is stripped when the bare name matches.
        assert_eq!(
            plan(&["api_key; x=1"], &["api_key"]),
            CookieStripPlan::Replace("x=1".to_string())
        );
    }

    #[test]
    fn plan_cookie_strip_with_no_names_keeps_everything() {
        assert_eq!(
            plan(&["a=1; b=2"], &[]),
            CookieStripPlan::Replace("a=1; b=2".to_string())
        );
    }
}
