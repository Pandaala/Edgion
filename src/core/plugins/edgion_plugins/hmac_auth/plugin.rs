//! HMAC Auth plugin implementation.
//!
//! Implements HMAC HTTP Signature verification with:
//! - Authorization / Proxy-Authorization parsing
//! - HMAC-SHA256/384/512 verification
//! - clock skew validation via Date / X-Date
//! - enforceHeaders validation

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Sha256, Sha384, Sha512};

use crate::core::plugins::edgion_plugins::common::auth_common::send_auth_error_response;
use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{HmacAlgorithm, HmacAuthConfig, HmacCredential};

type HmacSha256 = Hmac<Sha256>;
type HmacSha384 = Hmac<Sha384>;
type HmacSha512 = Hmac<Sha512>;

/// Parsed HMAC authorization parameters.
#[derive(Debug)]
struct HmacAuthParams {
    username: String,
    algorithm: HmacAlgorithm,
    headers: Vec<String>,
    signature: Vec<u8>,
}

/// HMAC Auth plugin.
pub struct HmacAuth {
    name: String,
    config: HmacAuthConfig,
    credentials: Arc<HashMap<String, HmacCredential>>,
}

impl HmacAuth {
    /// Create plugin for runtime.
    pub fn create(config: &HmacAuthConfig) -> Box<dyn RequestFilter> {
        Box::new(Self::new(config))
    }

    /// Create plugin instance.
    pub fn new(config: &HmacAuthConfig) -> Self {
        Self {
            name: "HmacAuth".to_string(),
            config: config.clone(),
            credentials: Arc::new(config.resolved_credentials.clone().unwrap_or_default()),
        }
    }

    fn extract_auth_header(&self, session: &dyn PluginSession) -> Option<String> {
        session
            .header_value("authorization")
            .or_else(|| session.header_value("proxy-authorization"))
    }

    async fn apply_auth_failure_delay(&self) {
        if self.config.auth_failure_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.config.auth_failure_delay_ms)).await;
        }
    }

    async fn reject(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        status: u16,
        message: &str,
        log_code: &str,
    ) -> PluginRunningResult {
        plugin_log.push(log_code);
        self.apply_auth_failure_delay().await;
        let _ = send_auth_error_response(session, status, "hmac", &self.config.realm, message).await;
        PluginRunningResult::ErrTerminateRequest
    }

    fn set_anonymous_headers(&self, session: &mut dyn PluginSession, username: &str) {
        let _ = session.set_request_header("X-Anonymous-Consumer", "true");
        let _ = session.set_request_header("X-Consumer-Username", username);
    }

    fn split_auth_params(input: &str) -> Vec<&str> {
        let mut parts = Vec::new();
        let mut in_quotes = false;
        let mut escaped = false;
        let mut start = 0usize;

        for (idx, ch) in input.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if in_quotes && ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                in_quotes = !in_quotes;
                continue;
            }
            if ch == ',' && !in_quotes {
                parts.push(input[start..idx].trim());
                start = idx + 1;
            }
        }

        if start <= input.len() {
            parts.push(input[start..].trim());
        }
        parts
    }

    fn parse_quoted_value(raw: &str) -> Result<String, &'static str> {
        let trimmed = raw.trim();
        if trimmed.len() < 2 || !trimmed.starts_with('"') || !trimmed.ends_with('"') {
            return Err("HMAC parameter value must be quoted");
        }

        let inner = &trimmed[1..trimmed.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut escaped = false;

        for ch in inner.chars() {
            if escaped {
                out.push(ch);
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            out.push(ch);
        }
        if escaped {
            return Err("Invalid escape in quoted value");
        }
        Ok(out)
    }

    /// Parse Authorization header into HMAC parameters.
    fn parse_authorization(auth_header: &str) -> Result<HmacAuthParams, &'static str> {
        let trimmed = auth_header.trim();
        let (scheme, params_str) = trimmed.split_once(' ').ok_or("Invalid HMAC authorization format")?;
        if !scheme.eq_ignore_ascii_case("hmac") {
            return Err("Invalid HMAC authorization format");
        }
        let params_str = params_str.trim();
        if params_str.is_empty() {
            return Err("Invalid HMAC authorization format");
        }

        let mut params: HashMap<String, String> = HashMap::new();
        for part in Self::split_auth_params(params_str) {
            if part.is_empty() {
                continue;
            }
            let (key, value_raw) = part.split_once('=').ok_or("Invalid HMAC authorization format")?;
            let key = key.trim().to_ascii_lowercase();
            if key.is_empty() {
                return Err("Invalid HMAC authorization format");
            }
            if params.contains_key(&key) {
                return Err("Invalid HMAC authorization format");
            }
            let value = Self::parse_quoted_value(value_raw)?;
            params.insert(key, value);
        }

        let username = params
            .remove("username")
            .filter(|v| !v.trim().is_empty())
            .ok_or("Invalid HMAC authorization format")?;
        let algorithm_raw = params.remove("algorithm").ok_or("Invalid HMAC authorization format")?;
        let headers_raw = params.remove("headers").ok_or("Invalid HMAC authorization format")?;
        let signature_raw = params.remove("signature").ok_or("Invalid HMAC authorization format")?;

        let algorithm = HmacAlgorithm::parse(&algorithm_raw).ok_or("Unsupported algorithm")?;
        let headers: Vec<String> = headers_raw.split_whitespace().map(|h| h.to_ascii_lowercase()).collect();
        if headers.is_empty() {
            return Err("Invalid HMAC authorization format");
        }
        let mut seen = HashSet::new();
        for h in &headers {
            if !seen.insert(h.as_str()) {
                return Err("Invalid HMAC authorization format");
            }
        }

        let signature = STANDARD
            .decode(signature_raw)
            .map_err(|_| "Invalid HMAC authorization format")?;

        Ok(HmacAuthParams {
            username,
            algorithm,
            headers,
            signature,
        })
    }

    fn validate_enforce_headers(&self, signed_headers: &[String]) -> Result<(), String> {
        let Some(required) = &self.config.enforce_headers else {
            return Ok(());
        };

        let signed_set: HashSet<&str> = signed_headers.iter().map(|s| s.as_str()).collect();
        for required_header in required {
            let required_lower = required_header.to_ascii_lowercase();
            if !signed_set.contains(required_lower.as_str()) {
                return Err(format!("Missing required signed header: {}", required_header));
            }
        }
        Ok(())
    }

    /// Build signing string from request according to signed headers list.
    fn build_signing_string(session: &dyn PluginSession, headers_list: &[String]) -> Result<String, &'static str> {
        let mut lines = Vec::with_capacity(headers_list.len());

        for header in headers_list {
            let header_name = header.to_ascii_lowercase();
            if header_name == "@request-target" || header_name == "(request-target)" {
                let mut target = session.get_path().to_string();
                if let Some(query) = session.get_query() {
                    if !query.is_empty() {
                        target.push('?');
                        target.push_str(&query);
                    }
                }
                lines.push(format!(
                    "{}: {} {}",
                    header_name,
                    session.get_method().to_ascii_lowercase(),
                    target
                ));
                continue;
            }

            let Some(value) = session.header_value(&header_name) else {
                return Err("Missing signed header");
            };
            lines.push(format!("{}: {}", header_name, value));
        }

        Ok(lines.join("\n"))
    }

    /// Compute HMAC digest bytes.
    #[cfg(test)]
    fn compute_hmac(algorithm: HmacAlgorithm, secret: &[u8], signing_string: &str) -> Result<Vec<u8>, &'static str> {
        match algorithm {
            HmacAlgorithm::HmacSha256 => {
                let mut mac = HmacSha256::new_from_slice(secret).map_err(|_| "Invalid secret")?;
                mac.update(signing_string.as_bytes());
                Ok(mac.finalize().into_bytes().to_vec())
            }
            HmacAlgorithm::HmacSha384 => {
                let mut mac = HmacSha384::new_from_slice(secret).map_err(|_| "Invalid secret")?;
                mac.update(signing_string.as_bytes());
                Ok(mac.finalize().into_bytes().to_vec())
            }
            HmacAlgorithm::HmacSha512 => {
                let mut mac = HmacSha512::new_from_slice(secret).map_err(|_| "Invalid secret")?;
                mac.update(signing_string.as_bytes());
                Ok(mac.finalize().into_bytes().to_vec())
            }
        }
    }

    /// Verify HMAC signature using constant-time comparison.
    fn verify_signature(algorithm: HmacAlgorithm, secret: &[u8], signing_string: &str, actual: &[u8]) -> bool {
        match algorithm {
            HmacAlgorithm::HmacSha256 => {
                let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
                    return false;
                };
                mac.update(signing_string.as_bytes());
                mac.verify_slice(actual).is_ok()
            }
            HmacAlgorithm::HmacSha384 => {
                let Ok(mut mac) = HmacSha384::new_from_slice(secret) else {
                    return false;
                };
                mac.update(signing_string.as_bytes());
                mac.verify_slice(actual).is_ok()
            }
            HmacAlgorithm::HmacSha512 => {
                let Ok(mut mac) = HmacSha512::new_from_slice(secret) else {
                    return false;
                };
                mac.update(signing_string.as_bytes());
                mac.verify_slice(actual).is_ok()
            }
        }
    }

    fn signed_date_header<'a>(&self, signed_headers: &'a [String]) -> Option<&'a str> {
        if let Some(name) = signed_headers.iter().find(|h| h.as_str() == "x-date") {
            return Some(name.as_str());
        }
        signed_headers.iter().find(|h| h.as_str() == "date").map(|h| h.as_str())
    }

    /// Validate Date/X-Date clock skew.
    fn validate_clock_skew(
        session: &dyn PluginSession,
        max_skew: u64,
        signed_date_header: &str,
    ) -> Result<(), &'static str> {
        let date_str = session.header_value(signed_date_header).ok_or("Missing Date header")?;

        let parsed = DateTime::parse_from_rfc2822(&date_str)
            .or_else(|_| DateTime::parse_from_rfc3339(&date_str))
            .map_err(|_| "Invalid Date header")?;

        let request_ts = parsed.timestamp();
        let now_ts = Utc::now().timestamp();
        let diff = now_ts.abs_diff(request_ts);
        if diff > max_skew {
            return Err("Clock skew exceeded");
        }

        Ok(())
    }

    fn set_upstream_headers(&self, session: &mut dyn PluginSession, credential: &HmacCredential) {
        for (name, value) in &credential.headers {
            if name.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0') {
                continue;
            }
            if value.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0') {
                continue;
            }
            let _ = session.set_request_header(name, value);
        }
    }

    fn hide_credentials(&self, session: &mut dyn PluginSession) {
        if !self.config.hide_credentials {
            return;
        }
        let _ = session.remove_request_header("authorization");
        let _ = session.remove_request_header("proxy-authorization");
        let _ = session.remove_request_header("signature");
    }
}

#[async_trait]
impl RequestFilter for HmacAuth {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        if self.config.validate_request_body {
            return self
                .reject(
                    session,
                    plugin_log,
                    500,
                    "validateRequestBody is not supported yet",
                    "hmac:body-unsupported",
                )
                .await;
        }

        let auth_header = match self.extract_auth_header(session) {
            Some(value) => value,
            None => {
                if let Some(anonymous) = &self.config.anonymous {
                    self.set_anonymous_headers(session, anonymous);
                    plugin_log.push("hmac:anonymous");
                    return PluginRunningResult::GoodNext;
                }
                return self
                    .reject(session, plugin_log, 401, "Missing Authorization header", "hmac:no-auth")
                    .await;
            }
        };

        if self.credentials.is_empty() {
            return self
                .reject(session, plugin_log, 500, "No credentials configured", "hmac:no-creds")
                .await;
        }

        let params = match Self::parse_authorization(&auth_header) {
            Ok(v) => v,
            Err(_) => {
                return self
                    .reject(
                        session,
                        plugin_log,
                        401,
                        "Invalid HMAC authorization format",
                        "hmac:bad-format",
                    )
                    .await;
            }
        };

        if !self.config.algorithms.contains(&params.algorithm) {
            return self
                .reject(session, plugin_log, 401, "Algorithm not allowed", "hmac:bad-alg")
                .await;
        }

        let credential = match self.credentials.get(&params.username) {
            Some(v) => v,
            None => {
                return self
                    .reject(session, plugin_log, 401, "Invalid credentials", "hmac:bad-user")
                    .await;
            }
        };

        let signed_date = match self.signed_date_header(&params.headers) {
            Some(name) => name,
            None => {
                return self
                    .reject(
                        session,
                        plugin_log,
                        401,
                        "Missing required signed header: date or x-date",
                        "hmac:missing-date-signed",
                    )
                    .await;
            }
        };
        if let Err(message) = Self::validate_clock_skew(session, self.config.clock_skew, signed_date) {
            return self.reject(session, plugin_log, 401, message, "hmac:clock-skew").await;
        }

        if let Err(message) = self.validate_enforce_headers(&params.headers) {
            return self
                .reject(session, plugin_log, 401, &message, "hmac:missing-required")
                .await;
        }

        let signing_string = match Self::build_signing_string(session, &params.headers) {
            Ok(v) => v,
            Err(message) => {
                return self
                    .reject(session, plugin_log, 401, message, "hmac:missing-header")
                    .await;
            }
        };

        if !Self::verify_signature(params.algorithm, &credential.secret, &signing_string, &params.signature) {
            return self
                .reject(session, plugin_log, 401, "Invalid signature", "hmac:bad-signature")
                .await;
        }

        self.set_upstream_headers(session, credential);
        self.hide_credentials(session);

        plugin_log.push("hmac:ok");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;

    fn create_plugin() -> HmacAuth {
        let mut config = HmacAuthConfig {
            clock_skew: 600,
            enforce_headers: Some(vec!["@request-target".to_string()]),
            ..Default::default()
        };

        let mut credentials = HashMap::new();
        credentials.insert(
            "alice".to_string(),
            HmacCredential {
                secret: b"alice-super-secret-key".to_vec(),
                headers: HashMap::from([("X-Consumer-Username".to_string(), "alice".to_string())]),
            },
        );
        config.resolved_credentials = Some(credentials);

        HmacAuth::new(&config)
    }

    fn rfc2822_now() -> String {
        Utc::now().to_rfc2822()
    }

    #[test]
    fn test_parse_authorization_ok() {
        let header = r#"hmac username="alice", algorithm="hmac-sha256", headers="@request-target host date", signature="YWJjZA==""#;
        let parsed = HmacAuth::parse_authorization(header).expect("should parse");
        assert_eq!(parsed.username, "alice");
        assert_eq!(parsed.algorithm, HmacAlgorithm::HmacSha256);
        assert_eq!(parsed.headers, vec!["@request-target", "host", "date"]);
        assert_eq!(parsed.signature, b"abcd");
    }

    #[test]
    fn test_parse_authorization_invalid_format() {
        let header = r#"hmac username=alice, algorithm="hmac-sha256""#;
        let parsed = HmacAuth::parse_authorization(header);
        assert!(parsed.is_err());
    }

    #[test]
    fn test_build_signing_string_request_target() {
        let mut session = MockPluginSession::new();
        session.expect_get_path().return_const("/v1/items".to_string());
        session.expect_get_query().returning(|| Some("page=1".to_string()));
        session.expect_get_method().return_const("GET".to_string());

        let signing = HmacAuth::build_signing_string(&session, &[String::from("@request-target")]).unwrap();
        assert_eq!(signing, "@request-target: get /v1/items?page=1");
    }

    #[test]
    fn test_verify_signature_success() {
        let secret = b"test-secret";
        let signing = "@request-target: get /ping";
        let digest = HmacAuth::compute_hmac(HmacAlgorithm::HmacSha256, secret, signing).unwrap();
        assert!(HmacAuth::verify_signature(
            HmacAlgorithm::HmacSha256,
            secret,
            signing,
            &digest
        ));
    }

    #[tokio::test]
    async fn test_run_request_missing_auth_with_anonymous() {
        let config = HmacAuthConfig {
            anonymous: Some("guest".to_string()),
            ..Default::default()
        };
        let plugin = HmacAuth::new(&config);

        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("HmacAuth");

        session.expect_header_value().returning(|_| None);
        session.expect_set_request_header().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("hmac:anonymous"));
    }

    #[tokio::test]
    async fn test_run_request_valid_signature() {
        let plugin = create_plugin();
        let date = rfc2822_now();
        let signing = format!("@request-target: get /v1/ping\ndate: {}", date);
        let signature = HmacAuth::compute_hmac(HmacAlgorithm::HmacSha256, b"alice-super-secret-key", &signing).unwrap();
        let signature_b64 = STANDARD.encode(signature);
        let auth_header = format!(
            r#"hmac username="alice", algorithm="hmac-sha256", headers="@request-target date", signature="{}""#,
            signature_b64
        );

        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("HmacAuth");

        session.expect_header_value().returning(move |name| match name {
            "authorization" => Some(auth_header.clone()),
            "proxy-authorization" => None,
            "x-date" => None,
            "date" => Some(date.clone()),
            _ => None,
        });
        session.expect_get_path().return_const("/v1/ping".to_string());
        session.expect_get_query().returning(|| None);
        session.expect_get_method().return_const("GET".to_string());
        session.expect_set_request_header().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("hmac:ok"));
    }

    #[tokio::test]
    async fn test_run_request_invalid_signature() {
        let plugin = create_plugin();
        let date = rfc2822_now();
        let auth_header =
            r#"hmac username="alice", algorithm="hmac-sha256", headers="@request-target date", signature="YmFkLXNpZw==""#
                .to_string();

        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("HmacAuth");

        session.expect_header_value().returning(move |name| match name {
            "authorization" => Some(auth_header.clone()),
            "proxy-authorization" => None,
            "x-date" => None,
            "date" => Some(date.clone()),
            _ => None,
        });
        session.expect_get_path().return_const("/v1/ping".to_string());
        session.expect_get_query().returning(|| None);
        session.expect_get_method().return_const("GET".to_string());
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("hmac:bad-signature"));
    }

    #[tokio::test]
    async fn test_run_request_rejects_when_date_not_signed() {
        let plugin = create_plugin();
        let date = rfc2822_now();
        let auth_header =
            r#"hmac username="alice", algorithm="hmac-sha256", headers="@request-target", signature="YmFkLXNpZw==""#
                .to_string();

        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("HmacAuth");

        session.expect_header_value().returning(move |name| match name {
            "authorization" => Some(auth_header.clone()),
            "proxy-authorization" => None,
            "date" => Some(date.clone()),
            _ => None,
        });
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("hmac:missing-date-signed"));
    }
}
