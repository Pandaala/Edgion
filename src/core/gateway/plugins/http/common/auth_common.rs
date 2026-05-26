//! Shared authentication utilities for Edgion request auth plugins.

use bytes::Bytes;
use pingora_http::ResponseHeader;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

use crate::core::gateway::plugins::runtime::PluginSession;

/// Build and send a 401/403 auth error response for the gateway plane (Pingora).
///
/// Always sets `WWW-Authenticate: <scheme> realm="<realm>"`. When `error_code` is
/// `Some`, the challenge is extended per RFC 6750 §3.1 with `error="<code>"` and
/// `error_description="<body>"` (the body text escaped as a `quoted-string`).
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
fn escape_quoted_string(value: &str) -> String {
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
    out
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
    mapping: &HashMap<String, String>,
    max_header_value_bytes: usize,
    max_total_header_bytes: usize,
) {
    let mut total_bytes: usize = 0;

    for (claim_path, header_name) in mapping {
        if header_name.is_empty() {
            continue;
        }
        if header_name.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0') {
            continue;
        }

        let Some(value) = resolve_claim_path(claims, claim_path) else {
            continue;
        };

        let header_value = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|v| match v {
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    serde_json::Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(","),
            _ => continue,
        };

        if header_value.is_empty() {
            continue;
        }

        // Header injection protection.
        if header_value.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0') {
            continue;
        }

        if header_value.len() > max_header_value_bytes {
            continue;
        }

        total_bytes = total_bytes.saturating_add(header_name.len() + header_value.len());
        if total_bytes > max_total_header_bytes {
            break;
        }

        let _ = session.set_request_header(header_name, &header_value);
    }
}

#[cfg(test)]
mod tests {
    use super::{escape_quoted_string, resolve_claim_path, Claims};
    use serde_json::json;

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
}
