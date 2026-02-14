//! Shared authentication utilities for Edgion request auth plugins.

use bytes::Bytes;
use pingora_http::ResponseHeader;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::core::plugins::plugin_runtime::PluginSession;

/// Build and send authentication error response (typically 401/403).
///
/// Notes:
/// - `Connection: close` is intentionally not set. Pingora manages connection lifecycle.
/// - `body` should not include status prefix; this function prefixes with `<status> `.
pub async fn send_auth_error_response(
    session: &mut dyn PluginSession,
    status: u16,
    scheme: &str,
    realm: &str,
    body: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut resp = ResponseHeader::build(status, None)?;
    resp.insert_header("Content-Type", "text/plain")?;
    let www_auth = format!("{} realm=\"{}\"", scheme, realm);
    resp.insert_header("WWW-Authenticate", &www_auth)?;

    session.write_response_header(Box::new(resp), false).await?;
    session
        .write_response_body(Some(Bytes::from(format!("{} {}", status, body))), true)
        .await?;
    session.shutdown().await;
    Ok(())
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
            tracing::warn!(
                claim = claim_path,
                "Skipped claim header mapping due to empty header name"
            );
            continue;
        }
        if header_name.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0') {
            tracing::warn!(
                claim = claim_path,
                "Skipped claim header mapping due to invalid header name characters"
            );
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
            tracing::warn!(
                claim = claim_path,
                "Skipped claim header mapping due to control characters"
            );
            continue;
        }

        if header_value.len() > max_header_value_bytes {
            tracing::warn!(
                claim = claim_path,
                len = header_value.len(),
                max = max_header_value_bytes,
                "Skipped claim header mapping due to per-header size limit"
            );
            continue;
        }

        total_bytes = total_bytes.saturating_add(header_name.len() + header_value.len());
        if total_bytes > max_total_header_bytes {
            tracing::warn!(
                max = max_total_header_bytes,
                "Stopped claims-to-headers mapping due to total size limit"
            );
            break;
        }

        let _ = session.set_request_header(header_name, &header_value);
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_claim_path, Claims};
    use serde_json::json;

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
