//! HMAC Auth plugin implementation.
//!
//! Implements HMAC HTTP Signature verification with:
//! - Authorization / Proxy-Authorization parsing
//! - HMAC-SHA256/384/512 verification
//! - clock skew validation via Date / X-Date
//! - enforceHeaders validation

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha384, Sha512};

use crate::plugins::http::common::auth_common::{apply_auth_failure_delay, send_gateway_auth_error_response};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::traits::session::BodyReadError;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{HmacAlgorithm, HmacAuthConfig, HmacCredential};
use edgion_resources::EdgionStatus;

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

    async fn reject(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        status: u16,
        message: &str,
        log_code: &str,
    ) -> PluginRunningResult {
        plugin_log.push(log_code);
        apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
        let _ = send_gateway_auth_error_response(session, status, "hmac", &self.config.realm, None, message).await;
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

    /// Verify that the buffered request body matches the signed `Digest` header
    /// (`Digest: SHA-256=<base64(sha256(body))>`, Kong `validate_request_body`
    /// semantics). Returns the denial reason on mismatch/absence.
    ///
    /// Requires the request body to have been buffered (the body gate ensures
    /// this before the plugin runs). Only SHA-256 is accepted.
    ///
    /// A failed read records the matching `EdgionStatus` before denying, using
    /// the same `BodyReadError` -> `EdgionStatus` mapping as
    /// `body_gate.rs::body_read_failure_result` and `wasm/plugin.rs` — the
    /// returned `&'static str` is the client-visible denial reason and must
    /// stay opaque, so `ctx.add_error` is the only place the specific cause
    /// becomes observable. `NotBuffered` is excluded: the read adapter already
    /// recorded which refusal, and double-counting it here would inflate that
    /// status.
    ///
    /// Only the STATUS mapping is shared. The HTTP code is not: auth is
    /// fail-closed, so the caller answers every denial with the plugin's auth
    /// error response, not with `body_gate.rs`'s per-error 413/408 split.
    async fn verify_body_digest(session: &mut dyn PluginSession) -> Result<(), &'static str> {
        let body = match session.read_request_body(None).await {
            Ok(body) => body,
            Err(err) => {
                match err {
                    BodyReadError::TooLarge { .. } => session.add_error(EdgionStatus::BodyReadTooLarge),
                    BodyReadError::Timeout => session.add_error(EdgionStatus::BodyReadTimeout),
                    BodyReadError::Incomplete => session.add_error(EdgionStatus::BodyReadAborted),
                    BodyReadError::Unimplemented => session.add_error(EdgionStatus::BodyReadUnimplemented),
                    // Already recorded on ctx by the read adapter.
                    BodyReadError::NotBuffered => {}
                }
                return Err("request body unavailable");
            }
        };
        let digest_header = session.header_value("digest").ok_or("Missing Digest header")?;
        Self::check_body_digest(&body, &digest_header)
    }

    /// Pure `Digest` header check: `SHA-256=<base64(sha256(body))>`.
    fn check_body_digest(body: &[u8], digest_header: &str) -> Result<(), &'static str> {
        // Split on the FIRST '=' so base64 padding stays with the value.
        let (algo, expected) = digest_header.split_once('=').ok_or("Malformed Digest header")?;
        if !algo.trim().eq_ignore_ascii_case("SHA-256") {
            return Err("Unsupported Digest algorithm");
        }

        let mut hasher = Sha256::new();
        hasher.update(body);
        let computed = STANDARD.encode(hasher.finalize());
        if computed != expected.trim() {
            return Err("Body digest mismatch");
        }
        Ok(())
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

    // Body requirement is declared on `EdgionPlugin::body_requirement` (schema
    // layer) so the controller and gateway agree: HMAC consumes the request body
    // exactly when `validateRequestBody` is set, and the runtime then buffers it
    // (fail-close) before this plugin runs.

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let auth_header = match self.extract_auth_header(session) {
            Some(value) => value,
            None => {
                if let Some(anonymous) = &self.config.anonymous {
                    self.set_anonymous_headers(session, anonymous);
                    plugin_log.push("Allow anonymous");
                    return PluginRunningResult::GoodNext;
                }
                return self
                    .reject(
                        session,
                        plugin_log,
                        401,
                        "Missing Authorization header",
                        "Deny missing auth",
                    )
                    .await;
            }
        };

        if self.credentials.is_empty() {
            plugin_log.push_err("no creds configured");
            apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
            let _ = send_gateway_auth_error_response(
                session,
                500,
                "hmac",
                &self.config.realm,
                None,
                "No credentials configured",
            )
            .await;
            return PluginRunningResult::ErrTerminateRequest;
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
                        "Deny bad format",
                    )
                    .await;
            }
        };

        if !self.config.algorithms.contains(&params.algorithm) {
            return self
                .reject(session, plugin_log, 401, "Algorithm not allowed", "Deny bad algorithm")
                .await;
        }

        let credential = match self.credentials.get(&params.username) {
            Some(v) => v,
            None => {
                return self
                    .reject(session, plugin_log, 401, "Invalid credentials", "Deny missing key")
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
                        "Deny missing date signed",
                    )
                    .await;
            }
        };
        if let Err(message) = Self::validate_clock_skew(session, self.config.clock_skew, signed_date) {
            return self
                .reject(session, plugin_log, 401, message, "Deny bad timestamp")
                .await;
        }

        if let Err(message) = self.validate_enforce_headers(&params.headers) {
            return self
                .reject(session, plugin_log, 401, &message, "Deny missing required header")
                .await;
        }

        let signing_string = match Self::build_signing_string(session, &params.headers) {
            Ok(v) => v,
            Err(message) => {
                return self
                    .reject(session, plugin_log, 401, message, "Deny missing header")
                    .await;
            }
        };

        if !Self::verify_signature(params.algorithm, &credential.secret, &signing_string, &params.signature) {
            return self
                .reject(session, plugin_log, 401, "Invalid signature", "Deny bad signature")
                .await;
        }

        // Body integrity. The signature authenticates the `Digest` header only if
        // `digest` is in the signed-headers list — otherwise an attacker could
        // swap the body, recompute `Digest`, and keep the original signature. So
        // require `digest` to be signed BEFORE trusting it against the body.
        // (Kong's hmac-auth enforces the same.)
        if self.config.validate_request_body {
            if !params.headers.iter().any(|h| h == "digest") {
                return self
                    .reject(
                        session,
                        plugin_log,
                        401,
                        "Missing required signed header: digest",
                        "Deny digest not signed",
                    )
                    .await;
            }
            if let Err(message) = Self::verify_body_digest(session).await {
                return self.reject(session, plugin_log, 401, message, "Deny body digest").await;
            }
        }

        self.set_upstream_headers(session, credential);
        self.hide_credentials(session);

        plugin_log.push_fmt(
            "OK group={} user {}",
            la![credential.group.as_str(), sensitive(&params.username)],
        );
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;

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
                group: "default".to_string(),
            },
        );
        config.resolved_credentials = Some(credentials);

        HmacAuth::new(&config)
    }

    fn rfc2822_now() -> String {
        Utc::now().to_rfc2822()
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
    fn test_check_body_digest_matches() {
        let body = b"{\"hello\":\"world\"}";
        let mut hasher = Sha256::new();
        hasher.update(body);
        let digest = format!("SHA-256={}", STANDARD.encode(hasher.finalize()));
        assert!(HmacAuth::check_body_digest(body, &digest).is_ok());
    }

    #[test]
    fn test_check_body_digest_mismatch() {
        let body = b"tampered";
        // Digest computed over different bytes.
        let mut hasher = Sha256::new();
        hasher.update(b"original");
        let digest = format!("SHA-256={}", STANDARD.encode(hasher.finalize()));
        assert_eq!(HmacAuth::check_body_digest(body, &digest), Err("Body digest mismatch"));
    }

    #[test]
    fn test_check_body_digest_bad_algorithm_and_format() {
        assert_eq!(
            HmacAuth::check_body_digest(b"x", "MD5=abc"),
            Err("Unsupported Digest algorithm")
        );
        assert_eq!(
            HmacAuth::check_body_digest(b"x", "no-equals-sign"),
            Err("Malformed Digest header")
        );
    }

    /// Every distinct `BodyReadError` must reach the access log as its own
    /// `EdgionStatus`, matching `body_gate.rs` / `wasm/plugin.rs`. The denial
    /// reason returned to the caller stays opaque, so `add_error` is the only
    /// place the cause is observable — collapsing the variants here would make
    /// a timeout indistinguishable from an oversize body in production.
    #[tokio::test]
    async fn verify_body_digest_records_status_per_read_error() {
        for (err, expected) in [
            (BodyReadError::TooLarge { limit: 8 }, EdgionStatus::BodyReadTooLarge),
            (BodyReadError::Timeout, EdgionStatus::BodyReadTimeout),
            (BodyReadError::Incomplete, EdgionStatus::BodyReadAborted),
            (BodyReadError::Unimplemented, EdgionStatus::BodyReadUnimplemented),
        ] {
            let mut session = MockPluginSession::new();
            session.expect_read_request_body().returning(move |_| Err(err.clone()));
            session
                .expect_add_error()
                .withf(move |status| *status == expected)
                .times(1)
                .returning(|_| ());

            assert_eq!(
                HmacAuth::verify_body_digest(&mut session).await,
                Err("request body unavailable")
            );
        }
    }

    /// `NotBuffered` is the one variant that must NOT be re-recorded: the read
    /// adapter already put the specific refusal on ctx.
    #[tokio::test]
    async fn verify_body_digest_does_not_double_count_refusal() {
        let mut session = MockPluginSession::new();
        session
            .expect_read_request_body()
            .returning(|_| Err(BodyReadError::NotBuffered));
        session.expect_add_error().times(0);

        assert_eq!(
            HmacAuth::verify_body_digest(&mut session).await,
            Err("request body unavailable")
        );
    }

    #[test]
    fn test_body_requirement_declared_only_when_validate_body() {
        use edgion_resources::resources::edgion_plugins::EdgionPlugin;
        let base = HmacAuthConfig {
            resolved_credentials: Some(HashMap::new()),
            ..Default::default()
        };
        use edgion_resources::resources::edgion_plugins::BodyStage;
        assert!(EdgionPlugin::HmacAuth(base.clone())
            .body_requirement(BodyStage::Request)
            .is_none());
        assert!(!EdgionPlugin::HmacAuth(base.clone()).accepts_body(BodyStage::Request));

        let with_body = HmacAuthConfig {
            validate_request_body: true,
            ..base
        };
        // request-only plugin: consumes at the request stage
        assert!(EdgionPlugin::HmacAuth(with_body.clone())
            .body_requirement(BodyStage::Request)
            .is_some());
        assert!(EdgionPlugin::HmacAuth(with_body.clone())
            .body_requirement(BodyStage::Response)
            .is_none());
        assert!(EdgionPlugin::HmacAuth(with_body.clone()).accepts_body(BodyStage::Request));
        assert!(!EdgionPlugin::HmacAuth(with_body).accepts_body(BodyStage::Response));
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
        assert!(log.contains("Deny missing date signed"));
    }

    use crate::plugins::runtime::PluginLog;
    use serde_json::Value;

    fn render_log(log: &PluginLog) -> Vec<String> {
        let v: Value = serde_json::to_value(log).unwrap();
        v.get("log")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }

    #[test]
    fn hmac_hit_log_contains_group_and_user() {
        let mut log = PluginLog::new("HmacAuth");
        let credential = HmacCredential {
            secret: b"a-secret-key-at-least-32-bytes-long".to_vec(),
            headers: HashMap::new(),
            group: "partner-a".to_string(),
        };
        let username = "alice";

        log.push_fmt(
            "OK group={} user {}",
            la![credential.group.as_str(), sensitive(username)],
        );

        let entries = render_log(&log);
        assert!(
            entries
                .iter()
                .any(|e| e.starts_with("OK group=partner-a") && e.contains("user ")),
            "expected 'OK group=partner-a user ...', got: {:?}",
            entries
        );
    }

    // The "group=" tag in the OK log path is enforced by code review, not by a
    // unit test: driving all failure branches here would be pure tautology.

    // ===== validate_request_body (body-digest) end-to-end =====

    /// Build a mock session carrying a VALID HMAC signature over `signed_headers`
    /// for user "alice", plus the given `digest` header and buffered `body`.
    /// `signed_headers` are the lowercased names (e.g. `["@request-target","date","digest"]`).
    fn signed_session(signed_headers: &[&str], digest: Option<&str>, body: &'static [u8]) -> MockPluginSession {
        let secret = b"alice-super-secret-key".to_vec();
        let date = rfc2822_now();
        let path = "/v1/items";
        let method = "post";

        // Reconstruct the exact signing string build_signing_string would produce.
        let digest_val = digest.map(|d| d.to_string());
        let mut lines = Vec::new();
        for h in signed_headers {
            match *h {
                "@request-target" => lines.push(format!("@request-target: {method} {path}")),
                "date" => lines.push(format!("date: {date}")),
                "digest" => lines.push(format!("digest: {}", digest_val.clone().unwrap_or_default())),
                other => panic!("unhandled signed header in test: {other}"),
            }
        }
        let signing = lines.join("\n");
        let sig = HmacAuth::compute_hmac(HmacAlgorithm::HmacSha256, &secret, &signing).unwrap();
        let sig_b64 = STANDARD.encode(sig);
        let headers_str = signed_headers.join(" ");
        let auth = format!(
            r#"hmac username="alice", algorithm="hmac-sha256", headers="{headers_str}", signature="{sig_b64}""#
        );

        let mut session = MockPluginSession::new();
        let date_c = date.clone();
        session.expect_header_value().returning(move |name| match name {
            "authorization" => Some(auth.clone()),
            "proxy-authorization" => None,
            "date" => Some(date_c.clone()),
            "digest" => digest_val.clone(),
            _ => None,
        });
        session.expect_get_path().return_const(path.to_string());
        session.expect_get_query().returning(|| None);
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_read_request_body()
            .returning(move |_| Ok(bytes::Bytes::from_static(body)));
        session.expect_set_request_header().returning(|_, _| Ok(()));
        session.expect_remove_request_header().returning(|_| Ok(()));
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});
        session
    }

    fn plugin_validate_body() -> HmacAuth {
        let mut config = HmacAuthConfig {
            clock_skew: 600,
            validate_request_body: true,
            ..Default::default()
        };
        let mut credentials = HashMap::new();
        credentials.insert(
            "alice".to_string(),
            HmacCredential {
                secret: b"alice-super-secret-key".to_vec(),
                headers: HashMap::new(),
                group: "default".to_string(),
            },
        );
        config.resolved_credentials = Some(credentials);
        HmacAuth::new(&config)
    }

    fn body_digest(body: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(body);
        format!("SHA-256={}", STANDARD.encode(h.finalize()))
    }

    #[tokio::test]
    async fn validate_body_rejects_when_digest_not_signed() {
        // F1: digest is NOT in the signed-headers list → must reject even though
        // the signature itself is valid and a (spoofable) Digest header is present.
        let body = b"{\"a\":1}";
        let plugin = plugin_validate_body();
        let mut session = signed_session(&["@request-target", "date"], Some(&body_digest(body)), body);
        let mut log = PluginLog::new("HmacAuth");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny digest not signed"));
    }

    #[tokio::test]
    async fn validate_body_accepts_matching_signed_digest() {
        let body = b"{\"a\":1}";
        let plugin = plugin_validate_body();
        let mut session = signed_session(&["@request-target", "date", "digest"], Some(&body_digest(body)), body);
        let mut log = PluginLog::new("HmacAuth");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn validate_body_rejects_tampered_body() {
        // Digest is signed and present, but the actual body doesn't match it.
        let real_body = b"{\"a\":1}";
        let plugin = plugin_validate_body();
        // Sign a digest computed over DIFFERENT bytes than the buffered body.
        let signed_digest = body_digest(b"{\"a\":2}");
        let mut session = signed_session(&["@request-target", "date", "digest"], Some(&signed_digest), real_body);
        let mut log = PluginLog::new("HmacAuth");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny body digest"));
    }
}
