//! JWE Decrypt plugin implementation.
//!
//! Phase 1 supports compact JWE:
//! - alg=dir
//! - enc=A256GCM

use std::sync::{Arc, RwLock};
use std::time::Duration;

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use async_trait::async_trait;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;

use crate::core::conf_mgr::sync_runtime::resource_processor::get_secret;
use crate::core::plugins::edgion_plugins::common::auth_common::{send_auth_error_response, set_claims_headers};
use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{JweContentEncryption, JweDecryptConfig};

#[derive(Debug, Clone, Copy)]
enum JweFailure {
    InvalidFormat,
    MissingHeader,
    NoKey,
    KeyLenErr,
    DecryptFail,
    BadAlg,
    AlgDenied,
}

impl JweFailure {
    fn status(self) -> u16 {
        match self {
            JweFailure::InvalidFormat => 400,
            JweFailure::MissingHeader => 400,
            JweFailure::NoKey => 403,
            JweFailure::KeyLenErr => 500,
            JweFailure::DecryptFail => 403,
            JweFailure::BadAlg => 400,
            JweFailure::AlgDenied => 400,
        }
    }

    fn message(self) -> &'static str {
        match self {
            JweFailure::InvalidFormat => "Invalid JWE format",
            JweFailure::MissingHeader => "JWE missing required headers",
            JweFailure::NoKey => "Decryption key not configured",
            JweFailure::KeyLenErr => "Invalid key length for algorithm",
            JweFailure::DecryptFail => "JWE decryption failed",
            JweFailure::BadAlg => "Unsupported encryption algorithm",
            JweFailure::AlgDenied => "Algorithm not allowed",
        }
    }

    fn log_code(self) -> &'static str {
        match self {
            JweFailure::InvalidFormat => "jwe:invalid-format",
            JweFailure::MissingHeader => "jwe:missing-header",
            JweFailure::NoKey => "jwe:no-key",
            JweFailure::KeyLenErr => "jwe:key-len-err",
            JweFailure::DecryptFail => "jwe:decrypt-fail",
            JweFailure::BadAlg => "jwe:bad-alg",
            JweFailure::AlgDenied => "jwe:alg-denied",
        }
    }
}

struct ParsedJwe<'a> {
    protected_segment: &'a str,
    iv_segment: &'a str,
    ciphertext_segment: &'a str,
    tag_segment: &'a str,
    enc: JweContentEncryption,
}

/// JWE Decrypt plugin.
pub struct JweDecrypt {
    name: String,
    config: JweDecryptConfig,
    plugin_namespace: String,
    credential: Arc<RwLock<Option<Vec<u8>>>>,
}

impl JweDecrypt {
    /// Create plugin from config and namespace.
    pub fn new(config: &JweDecryptConfig, plugin_namespace: String) -> Self {
        Self {
            name: "JweDecrypt".to_string(),
            config: config.clone(),
            plugin_namespace,
            credential: Arc::new(RwLock::new(None)),
        }
    }

    fn extract_token(&self, session: &dyn PluginSession) -> Option<String> {
        let raw_value = session.header_value(&self.config.header)?;
        let trimmed = raw_value.trim();

        let token = if let Some(prefix) = &self.config.strip_prefix {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                rest.trim().to_string()
            } else {
                trimmed.to_string()
            }
        } else {
            trimmed.to_string()
        };

        if token.is_empty() {
            None
        } else {
            Some(token)
        }
    }

    fn contains_header_control_chars(value: &str) -> bool {
        value.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0')
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
        failure: JweFailure,
    ) -> PluginRunningResult {
        plugin_log.push(failure.log_code());
        self.apply_auth_failure_delay().await;
        let _ = send_auth_error_response(session, failure.status(), "Bearer", "edgion", failure.message()).await;
        PluginRunningResult::ErrTerminateRequest
    }

    async fn reject_custom(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        status: u16,
        message: &str,
        log_code: &str,
    ) -> PluginRunningResult {
        plugin_log.push(log_code);
        self.apply_auth_failure_delay().await;
        let _ = send_auth_error_response(session, status, "Bearer", "edgion", message).await;
        PluginRunningResult::ErrTerminateRequest
    }

    fn decode_secret_if_needed(&self, secret_bytes: Vec<u8>) -> Result<Vec<u8>, String> {
        if !self.config.base64_secret {
            return Ok(secret_bytes);
        }
        let secret_str =
            String::from_utf8(secret_bytes).map_err(|e| format!("invalid UTF-8 in base64 secret: {}", e))?;
        STANDARD
            .decode(secret_str.trim())
            .map_err(|e| format!("failed to decode base64 secret: {}", e))
    }

    fn load_key(&self) -> Result<Vec<u8>, String> {
        {
            let guard = self.credential.read().map_err(|_| "credential lock poisoned")?;
            if let Some(ref key) = *guard {
                return Ok(key.clone());
            }
        }

        let key = self.load_key_from_secret()?;

        {
            let mut guard = self.credential.write().map_err(|_| "credential lock poisoned")?;
            *guard = Some(key.clone());
        }

        Ok(key)
    }

    fn load_key_from_secret(&self) -> Result<Vec<u8>, String> {
        if let Some(ref resolved) = self.config.resolved_credential {
            let secret_b64 = resolved
                .secret
                .as_ref()
                .ok_or("resolved credential missing 'secret' field")?;
            let secret_bytes = STANDARD
                .decode(secret_b64)
                .map_err(|e| format!("invalid base64 secret in resolved credential: {}", e))?;
            return self.decode_secret_if_needed(secret_bytes);
        }

        let secret_ref = self
            .config
            .secret_ref
            .as_ref()
            .ok_or("secret_ref not configured for JweDecrypt")?;

        let ns = secret_ref.namespace.as_deref().unwrap_or(&self.plugin_namespace);
        let secret = get_secret(Some(ns), &secret_ref.name)
            .ok_or_else(|| format!("secret {}/{} not found", ns, secret_ref.name))?;

        if let Some(data) = &secret.data {
            if let Some(secret_bytes) = data.get("secret") {
                return self.decode_secret_if_needed(secret_bytes.0.clone());
            }
        }
        if let Some(string_data) = &secret.string_data {
            if let Some(secret_str) = string_data.get("secret") {
                return self.decode_secret_if_needed(secret_str.as_bytes().to_vec());
            }
        }

        Err(format!("secret {}/{} missing 'secret' field", ns, secret_ref.name))
    }

    fn parse_compact_jwe<'a>(&self, token: &'a str) -> Result<ParsedJwe<'a>, JweFailure> {
        let segments: Vec<&str> = token.split('.').collect();
        if segments.len() != 5 {
            return Err(JweFailure::InvalidFormat);
        }

        let protected_segment = segments[0];
        let encrypted_key_segment = segments[1];
        let iv_segment = segments[2];
        let ciphertext_segment = segments[3];
        let tag_segment = segments[4];

        if protected_segment.is_empty()
            || iv_segment.is_empty()
            || ciphertext_segment.is_empty()
            || tag_segment.is_empty()
        {
            return Err(JweFailure::InvalidFormat);
        }
        // dir mode must not contain encrypted key.
        if !encrypted_key_segment.is_empty() {
            return Err(JweFailure::InvalidFormat);
        }

        let protected_bytes = URL_SAFE_NO_PAD
            .decode(protected_segment)
            .map_err(|_| JweFailure::InvalidFormat)?;
        let protected_json: serde_json::Value =
            serde_json::from_slice(&protected_bytes).map_err(|_| JweFailure::InvalidFormat)?;

        let alg = protected_json
            .get("alg")
            .and_then(|v| v.as_str())
            .ok_or(JweFailure::MissingHeader)?;
        if alg != self.config.key_management_algorithm.as_str() {
            return Err(JweFailure::BadAlg);
        }

        let enc_raw = protected_json
            .get("enc")
            .and_then(|v| v.as_str())
            .ok_or(JweFailure::MissingHeader)?;
        let enc = JweContentEncryption::parse(enc_raw).ok_or(JweFailure::BadAlg)?;

        if enc != self.config.content_encryption_algorithm {
            return Err(JweFailure::BadAlg);
        }
        if let Some(allowed) = &self.config.allowed_algorithms {
            if !allowed.contains(&enc) {
                return Err(JweFailure::AlgDenied);
            }
        }

        Ok(ParsedJwe {
            protected_segment,
            iv_segment,
            ciphertext_segment,
            tag_segment,
            enc,
        })
    }

    fn decrypt_compact_jwe(&self, token: &str, key: &[u8]) -> Result<String, JweFailure> {
        let parsed = self.parse_compact_jwe(token)?;
        if key.len() != parsed.enc.required_key_len() {
            return Err(JweFailure::KeyLenErr);
        }

        let iv = URL_SAFE_NO_PAD
            .decode(parsed.iv_segment)
            .map_err(|_| JweFailure::InvalidFormat)?;
        let mut ciphertext = URL_SAFE_NO_PAD
            .decode(parsed.ciphertext_segment)
            .map_err(|_| JweFailure::InvalidFormat)?;
        let tag_bytes = URL_SAFE_NO_PAD
            .decode(parsed.tag_segment)
            .map_err(|_| JweFailure::InvalidFormat)?;

        // RFC 7518 A256GCM uses 96-bit IV and 128-bit tag.
        if iv.len() != 12 || tag_bytes.len() != 16 {
            return Err(JweFailure::InvalidFormat);
        }

        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| JweFailure::KeyLenErr)?;
        let nonce = Nonce::from_slice(&iv);
        let tag = Tag::from_slice(&tag_bytes);

        cipher
            .decrypt_in_place_detached(nonce, parsed.protected_segment.as_bytes(), &mut ciphertext, tag)
            .map_err(|_| JweFailure::DecryptFail)?;

        String::from_utf8(ciphertext).map_err(|_| JweFailure::InvalidFormat)
    }
}

#[async_trait]
impl RequestFilter for JweDecrypt {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let token = match self.extract_token(session) {
            Some(token) => token,
            None if self.config.strict => {
                return self
                    .reject_custom(session, plugin_log, 403, "JWE token required", "jwe:no-token")
                    .await;
            }
            None => {
                plugin_log.push("jwe:bypass");
                return PluginRunningResult::GoodNext;
            }
        };

        if token.len() > self.config.max_token_size {
            return self
                .reject_custom(session, plugin_log, 400, "JWE token too large", "jwe:too-large")
                .await;
        }

        let key = match self.load_key() {
            Ok(key) => key,
            Err(err) => {
                tracing::debug!("JweDecrypt: failed to load key: {}", err);
                return self.reject(session, plugin_log, JweFailure::NoKey).await;
            }
        };

        let plaintext = match self.decrypt_compact_jwe(&token, &key) {
            Ok(plaintext) => plaintext,
            Err(err) => return self.reject(session, plugin_log, err).await,
        };

        if Self::contains_header_control_chars(&plaintext) {
            return self.reject(session, plugin_log, JweFailure::InvalidFormat).await;
        }

        if session
            .set_request_header(&self.config.forward_header, &plaintext)
            .is_err()
        {
            return self.reject(session, plugin_log, JweFailure::InvalidFormat).await;
        }

        if let Some(mapping) = &self.config.payload_to_headers {
            if let Ok(payload_json) = serde_json::from_str::<serde_json::Value>(&plaintext) {
                set_claims_headers(
                    session,
                    &payload_json,
                    mapping,
                    self.config.max_header_value_bytes,
                    self.config.max_total_header_bytes,
                );
            }
        }

        if self.config.store_payload_in_ctx {
            let _ = session.set_ctx_var("jwe_payload", &plaintext);
        }

        if self.config.hide_credentials && self.config.header != self.config.forward_header {
            let _ = session.remove_request_header(&self.config.header);
        }

        plugin_log.push("jwe:ok");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;
    use crate::types::resources::edgion_plugins::ResolvedJweCredential;
    use serde_json::json;

    fn test_key() -> Vec<u8> {
        b"0123456789abcdef0123456789abcdef".to_vec()
    }

    fn build_plugin(config_fn: Option<fn(&mut JweDecryptConfig)>) -> JweDecrypt {
        let mut config = JweDecryptConfig {
            strip_prefix: Some("Bearer ".to_string()),
            forward_header: "x-decrypted-auth".to_string(),
            resolved_credential: Some(ResolvedJweCredential {
                secret: Some(STANDARD.encode(test_key())),
            }),
            ..Default::default()
        };
        if let Some(f) = config_fn {
            f(&mut config);
        }
        JweDecrypt::new(&config, "default".to_string())
    }

    fn encrypt_compact_jwe(payload: &str, key: &[u8], enc: &str) -> String {
        let protected = json!({
            "alg": "dir",
            "enc": enc
        })
        .to_string();
        let protected_segment = URL_SAFE_NO_PAD.encode(protected.as_bytes());

        let iv = b"fixed-12-byt";
        let mut ciphertext = payload.as_bytes().to_vec();
        let cipher = Aes256Gcm::new_from_slice(key).unwrap();
        let nonce = Nonce::from_slice(iv);
        let tag = cipher
            .encrypt_in_place_detached(nonce, protected_segment.as_bytes(), &mut ciphertext)
            .unwrap();

        format!(
            "{}..{}.{}.{}",
            protected_segment,
            URL_SAFE_NO_PAD.encode(iv),
            URL_SAFE_NO_PAD.encode(ciphertext),
            URL_SAFE_NO_PAD.encode(tag)
        )
    }

    #[tokio::test]
    async fn test_decrypt_success() {
        let plugin = build_plugin(None);
        let token = encrypt_compact_jwe(r#"{"uid":"100","role":"admin"}"#, &test_key(), "A256GCM");
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("JweDecrypt");

        let header_value = format!("Bearer {}", token);
        session
            .expect_header_value()
            .with(mockall::predicate::eq("authorization"))
            .returning(move |_| Some(header_value.clone()));
        session.expect_set_request_header().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("jwe:ok"));
    }

    #[tokio::test]
    async fn test_missing_token_strict_false_bypass() {
        let plugin = build_plugin(Some(|cfg| cfg.strict = false));
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("JweDecrypt");

        session.expect_header_value().returning(|_| None);

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("jwe:bypass"));
    }

    #[tokio::test]
    async fn test_missing_token_strict_true_reject() {
        let plugin = build_plugin(None);
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("JweDecrypt");

        session.expect_header_value().returning(|_| None);
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("jwe:no-token"));
    }

    #[tokio::test]
    async fn test_token_too_large_reject() {
        let plugin = build_plugin(None);
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("JweDecrypt");

        let large = format!("Bearer {}", "a".repeat(9000));
        session.expect_header_value().returning(move |_| Some(large.clone()));
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("jwe:too-large"));
    }

    #[tokio::test]
    async fn test_bad_algorithm_reject() {
        let plugin = build_plugin(None);
        let token = encrypt_compact_jwe(r#"{"uid":"100"}"#, &test_key(), "A128GCM");
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("JweDecrypt");

        let header_value = format!("Bearer {}", token);
        session
            .expect_header_value()
            .returning(move |_| Some(header_value.clone()));
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("jwe:bad-alg"));
    }
}
