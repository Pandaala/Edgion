//! JWE Decrypt plugin implementation.
//!
//! Supports compact JWE (alg=dir, enc=A256GCM).

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use async_trait::async_trait;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;

use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, set_claims_headers,
};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_common::secret::get_secret;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{JweContentEncryption, JweDecryptConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            JweFailure::KeyLenErr => 403,
            JweFailure::DecryptFail => 403,
            JweFailure::BadAlg => 400,
            JweFailure::AlgDenied => 400,
        }
    }

    fn message(self) -> &'static str {
        match self {
            JweFailure::InvalidFormat => "Invalid JWE format",
            JweFailure::MissingHeader => "JWE missing required headers",
            JweFailure::NoKey | JweFailure::KeyLenErr | JweFailure::DecryptFail => "JWE decryption failed",
            JweFailure::BadAlg => "Unsupported encryption algorithm",
            JweFailure::AlgDenied => "Algorithm not allowed",
        }
    }

    fn log_code(self) -> &'static str {
        match self {
            JweFailure::InvalidFormat => "invalid format",
            JweFailure::MissingHeader => "missing header",
            JweFailure::NoKey => "no key",
            JweFailure::KeyLenErr => "key len err",
            JweFailure::DecryptFail => "decrypt fail",
            JweFailure::BadAlg => "bad alg",
            JweFailure::AlgDenied => "alg denied",
        }
    }
}

struct ParsedJwe<'a> {
    protected_segment: &'a str,
    iv_segment: &'a str,
    ciphertext_segment: &'a str,
    tag_segment: &'a str,
}

enum PreparedDecryptor {
    Ready(Aes256Gcm),
    Missing,
    DecodeFailed,
    InvalidLength,
}

enum KeyPreparationFailure {
    Missing,
    DecodeFailed,
}

enum RequestDecryptor<'a> {
    Ready(&'a Aes256Gcm),
    InvalidLength,
}

/// JWE Decrypt plugin.
pub struct JweDecrypt {
    name: String,
    config: JweDecryptConfig,
    decryptor: PreparedDecryptor,
}

impl JweDecrypt {
    /// Create plugin from config and namespace.
    pub fn new(config: &JweDecryptConfig, plugin_namespace: String) -> Self {
        let mut config = config.clone();
        let decryptor = Self::prepare_decryptor(&mut config, &plugin_namespace);
        Self {
            name: "JweDecrypt".to_string(),
            config,
            decryptor,
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

    async fn reject(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        failure: JweFailure,
    ) -> PluginRunningResult {
        match failure {
            JweFailure::NoKey => {
                plugin_log.push_err("key load fail");
            }
            JweFailure::DecryptFail => {
                plugin_log.push_err("decrypt fail");
            }
            JweFailure::KeyLenErr => {
                plugin_log.push_err("key len err");
            }
            _ => {
                plugin_log.push(failure.log_code());
            }
        }
        apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
        let _ =
            send_gateway_auth_error_response(session, failure.status(), "Bearer", "edgion", None, failure.message())
                .await;
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
        apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
        let _ = send_gateway_auth_error_response(session, status, "Bearer", "edgion", None, message).await;
        PluginRunningResult::ErrTerminateRequest
    }

    fn decode_secret_if_needed(secret_bytes: Vec<u8>, base64_secret: bool) -> Result<Vec<u8>, KeyPreparationFailure> {
        if !base64_secret {
            return Ok(secret_bytes);
        }
        let secret_str = std::str::from_utf8(&secret_bytes).map_err(|_| KeyPreparationFailure::DecodeFailed)?;
        STANDARD
            .decode(secret_str.trim())
            .map_err(|_| KeyPreparationFailure::DecodeFailed)
    }

    fn load_key_material(
        config: &mut JweDecryptConfig,
        plugin_namespace: &str,
    ) -> Result<Vec<u8>, KeyPreparationFailure> {
        if let Some(resolved) = config.resolved_credential.take() {
            let secret_b64 = resolved.secret.ok_or(KeyPreparationFailure::Missing)?;
            let secret_bytes = STANDARD
                .decode(secret_b64)
                .map_err(|_| KeyPreparationFailure::DecodeFailed)?;
            return Self::decode_secret_if_needed(secret_bytes, config.base64_secret);
        }

        let secret_ref = config.secret_ref.as_ref().ok_or(KeyPreparationFailure::Missing)?;

        let ns = secret_ref.namespace.as_deref().unwrap_or(plugin_namespace);
        let secret = get_secret(Some(ns), &secret_ref.name).ok_or(KeyPreparationFailure::Missing)?;

        if let Some(data) = &secret.data {
            if let Some(secret_bytes) = data.get("secret") {
                return Self::decode_secret_if_needed(secret_bytes.0.clone(), config.base64_secret);
            }
        }
        if let Some(string_data) = &secret.string_data {
            if let Some(secret_str) = string_data.get("secret") {
                return Self::decode_secret_if_needed(secret_str.as_bytes().to_vec(), config.base64_secret);
            }
        }

        Err(KeyPreparationFailure::Missing)
    }

    fn prepare_decryptor(config: &mut JweDecryptConfig, plugin_namespace: &str) -> PreparedDecryptor {
        let key = match Self::load_key_material(config, plugin_namespace) {
            Ok(key) => key,
            Err(KeyPreparationFailure::Missing) => return PreparedDecryptor::Missing,
            Err(KeyPreparationFailure::DecodeFailed) => return PreparedDecryptor::DecodeFailed,
        };

        if key.len() != config.content_encryption_algorithm.required_key_len() {
            return PreparedDecryptor::InvalidLength;
        }

        match Aes256Gcm::new_from_slice(&key) {
            Ok(cipher) => PreparedDecryptor::Ready(cipher),
            Err(_) => PreparedDecryptor::InvalidLength,
        }
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
        if protected_json.get("zip").is_some() || protected_json.get("crit").is_some() {
            return Err(JweFailure::InvalidFormat);
        }

        Ok(ParsedJwe {
            protected_segment,
            iv_segment,
            ciphertext_segment,
            tag_segment,
        })
    }

    fn decrypt_compact_jwe(&self, token: &str, decryptor: RequestDecryptor<'_>) -> Result<String, JweFailure> {
        let parsed = self.parse_compact_jwe(token)?;
        let cipher = match decryptor {
            RequestDecryptor::Ready(cipher) => cipher,
            RequestDecryptor::InvalidLength => return Err(JweFailure::KeyLenErr),
        };

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
                    .reject_custom(session, plugin_log, 403, "JWE token required", "no token")
                    .await;
            }
            None => {
                plugin_log.push("bypass");
                return PluginRunningResult::GoodNext;
            }
        };

        if token.len() > self.config.max_token_size {
            return self
                .reject_custom(session, plugin_log, 400, "JWE token too large", "token too large")
                .await;
        }

        let decryptor = match &self.decryptor {
            PreparedDecryptor::Ready(cipher) => RequestDecryptor::Ready(cipher),
            PreparedDecryptor::InvalidLength => RequestDecryptor::InvalidLength,
            PreparedDecryptor::Missing | PreparedDecryptor::DecodeFailed => {
                return self.reject(session, plugin_log, JweFailure::NoKey).await;
            }
        };

        let plaintext = match self.decrypt_compact_jwe(&token, decryptor) {
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

        plugin_log.push("OK");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::http::plugin_store::compile_plugins;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use bytes::Bytes;
    use edgion_resources::resources::edgion_plugins::{EdgionPlugin, RequestFilterEntry, ResolvedJweCredential};
    use edgion_resources::resources::EdgionPlugins;
    use mockall::Sequence;
    use serde_json::json;
    use std::collections::HashMap;

    fn test_key() -> Vec<u8> {
        b"0123456789abcdef0123456789abcdef".to_vec()
    }

    fn config_with_key(key: &[u8]) -> JweDecryptConfig {
        JweDecryptConfig {
            strip_prefix: Some("Bearer ".to_string()),
            forward_header: "x-decrypted-auth".to_string(),
            resolved_credential: Some(ResolvedJweCredential {
                secret: Some(STANDARD.encode(key)),
            }),
            ..Default::default()
        }
    }

    fn build_plugin(config_fn: Option<fn(&mut JweDecryptConfig)>) -> JweDecrypt {
        let mut config = config_with_key(&test_key());
        if let Some(f) = config_fn {
            f(&mut config);
        }
        JweDecrypt::new(&config, "default".to_string())
    }

    fn encrypt_compact_jwe_with_protected(payload: &str, key: &[u8], protected: &str, iv_byte: u8) -> String {
        let protected_segment = URL_SAFE_NO_PAD.encode(protected.as_bytes());
        let iv = [iv_byte; 12];
        let mut ciphertext = payload.as_bytes().to_vec();
        let cipher = Aes256Gcm::new_from_slice(key).unwrap();
        let nonce = Nonce::from_slice(&iv);
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

    fn encrypt_compact_jwe(payload: &str, key: &[u8], enc: &str) -> String {
        let protected = json!({
            "alg": "dir",
            "enc": enc
        })
        .to_string();
        encrypt_compact_jwe_with_protected(payload, key, &protected, 1)
    }

    fn decrypt_with_prepared(plugin: &JweDecrypt, token: &str) -> Result<String, JweFailure> {
        match &plugin.decryptor {
            PreparedDecryptor::Ready(cipher) => plugin.decrypt_compact_jwe(token, RequestDecryptor::Ready(cipher)),
            PreparedDecryptor::InvalidLength => plugin.decrypt_compact_jwe(token, RequestDecryptor::InvalidLength),
            PreparedDecryptor::Missing | PreparedDecryptor::DecodeFailed => Err(JweFailure::NoKey),
        }
    }

    fn expect_auth_response(
        session: &mut MockPluginSession,
        sequence: &mut Sequence,
        status: u16,
        body: &'static [u8],
    ) {
        session
            .expect_write_response_header()
            .withf(move |response, end_of_stream| {
                !end_of_stream
                    && response.status.as_u16() == status
                    && response
                        .headers
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        == Some("text/plain")
                    && response
                        .headers
                        .get("www-authenticate")
                        .and_then(|value| value.to_str().ok())
                        == Some("Bearer realm=\"edgion\"")
            })
            .times(1)
            .in_sequence(sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_write_response_body()
            .withf(move |response_body, end_of_stream| {
                *end_of_stream && response_body.as_deref() == Some(Bytes::from_static(body).as_ref())
            })
            .times(1)
            .in_sequence(sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_shutdown()
            .times(1)
            .in_sequence(sequence)
            .returning(|| {});
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
        assert!(log.contains("no token"));
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
        assert!(log.contains("token too large"));
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
        assert!(log.contains("bad alg"));
    }

    #[test]
    fn direct_constructor_defensively_classifies_key_material_and_drops_retained_credential() {
        let ready = JweDecrypt::new(&config_with_key(&test_key()), "default".to_string());
        assert!(matches!(ready.decryptor, PreparedDecryptor::Ready(_)));
        assert!(ready.config.resolved_credential.is_none());

        let mut missing_config = config_with_key(&test_key());
        missing_config.resolved_credential = Some(ResolvedJweCredential { secret: None });
        let missing = JweDecrypt::new(&missing_config, "default".to_string());
        assert!(matches!(missing.decryptor, PreparedDecryptor::Missing));
        assert!(missing.config.resolved_credential.is_none());

        let mut decode_config = config_with_key(&test_key());
        decode_config.resolved_credential = Some(ResolvedJweCredential {
            secret: Some("not-base64!".to_string()),
        });
        let decode = JweDecrypt::new(&decode_config, "default".to_string());
        assert!(matches!(decode.decryptor, PreparedDecryptor::DecodeFailed));
        assert!(decode.config.resolved_credential.is_none());

        let mut inner_decode_config = config_with_key(&test_key());
        inner_decode_config.base64_secret = true;
        inner_decode_config.resolved_credential = Some(ResolvedJweCredential {
            secret: Some(STANDARD.encode("not-base64!")),
        });
        let inner_decode = JweDecrypt::new(&inner_decode_config, "default".to_string());
        assert!(matches!(inner_decode.decryptor, PreparedDecryptor::DecodeFailed));
        assert!(inner_decode.config.resolved_credential.is_none());

        let invalid_length = JweDecrypt::new(&config_with_key(b"too-short"), "default".to_string());
        assert!(matches!(invalid_length.decryptor, PreparedDecryptor::InvalidLength));
        assert!(invalid_length.config.resolved_credential.is_none());
    }

    #[test]
    fn prepared_cipher_is_send_sync_reusable_and_generation_isolated() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Aes256Gcm>();
        assert_send_sync::<JweDecrypt>();

        let key_one = [0x11; 32];
        let key_two = [0x22; 32];
        let plugin_one = JweDecrypt::new(&config_with_key(&key_one), "default".to_string());
        let plugin_two = JweDecrypt::new(&config_with_key(&key_two), "default".to_string());
        let token_one = encrypt_compact_jwe_with_protected(
            r#"{"generation":"one"}"#,
            &key_one,
            r#"{"alg":"dir","enc":"A256GCM"}"#,
            2,
        );
        let token_two = encrypt_compact_jwe_with_protected(
            r#"{"generation":"two"}"#,
            &key_two,
            r#"{"alg":"dir","enc":"A256GCM"}"#,
            3,
        );

        assert_eq!(
            decrypt_with_prepared(&plugin_one, &token_one).unwrap(),
            r#"{"generation":"one"}"#
        );
        assert_eq!(
            decrypt_with_prepared(&plugin_one, &token_one).unwrap(),
            r#"{"generation":"one"}"#
        );
        assert_eq!(
            decrypt_with_prepared(&plugin_one, &token_two),
            Err(JweFailure::DecryptFail)
        );
        assert_eq!(
            decrypt_with_prepared(&plugin_two, &token_two).unwrap(),
            r#"{"generation":"two"}"#
        );
        assert_eq!(
            decrypt_with_prepared(&plugin_two, &token_one),
            Err(JweFailure::DecryptFail)
        );
        assert!(plugin_one.config.resolved_credential.is_none());
        assert!(plugin_two.config.resolved_credential.is_none());
    }

    #[tokio::test]
    async fn protected_zip_and_crit_are_rejected_on_presence() {
        let cases = [
            r#"{"alg":"dir","enc":"A256GCM","crit":null}"#,
            r#"{"alg":"dir","enc":"A256GCM","crit":[]}"#,
            r#"{"alg":"dir","enc":"A256GCM","crit":["exp"],"exp":true}"#,
            r#"{"alg":"dir","enc":"A256GCM","zip":null}"#,
            r#"{"alg":"dir","enc":"A256GCM","zip":"DEF"}"#,
        ];

        for (index, protected) in cases.into_iter().enumerate() {
            let plugin = build_plugin(None);
            let token =
                encrypt_compact_jwe_with_protected(r#"{"sub":"alice"}"#, &test_key(), protected, index as u8 + 10);
            let header_value = format!("Bearer {token}");
            let mut session = MockPluginSession::new();
            session
                .expect_header_value()
                .withf(|name| name == "authorization")
                .times(1)
                .returning(move |_| Some(header_value.clone()));
            let mut sequence = Sequence::new();
            expect_auth_response(&mut session, &mut sequence, 400, b"400 Invalid JWE format");

            let mut log = PluginLog::new("JweDecrypt");
            let result = plugin.run_request(&mut session, &mut log).await;
            assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
            assert_eq!(log.render_for_test(), "invalid format");
        }
    }

    #[tokio::test]
    async fn unknown_noncritical_header_uses_raw_aad_and_preserves_success_order() {
        let mut config = config_with_key(&test_key());
        config.hide_credentials = true;
        config.store_payload_in_ctx = true;
        config.payload_to_headers = Some(HashMap::from([("sub".to_string(), "x-sub".to_string())]));
        let plugin = JweDecrypt::new(&config, "default".to_string());
        let plaintext = r#"{"sub":"alice"}"#;
        let protected = r#"{ "enc":"A256GCM", "x-private":"accepted", "alg":"dir" }"#;
        let token = encrypt_compact_jwe_with_protected(plaintext, &test_key(), protected, 20);
        let header_value = format!("Bearer {token}");
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|name| name == "authorization")
            .times(1)
            .returning(move |_| Some(header_value.clone()));

        let mut sequence = Sequence::new();
        session
            .expect_set_request_header()
            .withf(move |name, value| name == "x-decrypted-auth" && value == plaintext)
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_set_request_header()
            .withf(|name, value| name == "x-sub" && value == "alice")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_set_ctx_var()
            .withf(move |name, value| name == "jwe_payload" && value == plaintext)
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_remove_request_header()
            .withf(|name| name == "authorization")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));

        let mut log = PluginLog::new("JweDecrypt");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "OK");
    }

    #[tokio::test]
    async fn direct_constructor_unavailable_key_precedes_parsing_with_one_generic_response_and_log() {
        let cases = [
            Some(ResolvedJweCredential { secret: None }),
            Some(ResolvedJweCredential {
                secret: Some("not-base64!".to_string()),
            }),
        ];

        for resolved_credential in cases {
            let mut config = config_with_key(&test_key());
            config.resolved_credential = resolved_credential;
            let plugin = JweDecrypt::new(&config, "default".to_string());
            let mut session = MockPluginSession::new();
            session
                .expect_header_value()
                .withf(|name| name == "authorization")
                .times(1)
                .return_const(Some("Bearer malformed".to_string()));
            let mut sequence = Sequence::new();
            expect_auth_response(&mut session, &mut sequence, 403, b"403 JWE decryption failed");

            let mut log = PluginLog::new("JweDecrypt");
            let result = plugin.run_request(&mut session, &mut log).await;
            assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
            assert_eq!(log.render_for_test(), "err: key load fail");
        }
    }

    #[tokio::test]
    async fn direct_constructor_invalid_key_length_has_one_generic_response_and_distinct_log() {
        let plugin = JweDecrypt::new(&config_with_key(b"too-short"), "default".to_string());
        let token = encrypt_compact_jwe(r#"{"sub":"alice"}"#, &test_key(), "A256GCM");
        let header_value = format!("Bearer {token}");
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|name| name == "authorization")
            .times(1)
            .returning(move |_| Some(header_value.clone()));
        let mut sequence = Sequence::new();
        expect_auth_response(&mut session, &mut sequence, 403, b"403 JWE decryption failed");

        let mut log = PluginLog::new("JweDecrypt");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert_eq!(log.render_for_test(), "err: key len err");
    }

    #[tokio::test]
    async fn direct_constructor_preserves_parse_precedence_for_invalid_key_length() {
        let plugin = JweDecrypt::new(&config_with_key(b"too-short"), "default".to_string());
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|name| name == "authorization")
            .times(1)
            .return_const(Some("Bearer malformed".to_string()));
        let mut sequence = Sequence::new();
        expect_auth_response(&mut session, &mut sequence, 400, b"400 Invalid JWE format");

        let mut log = PluginLog::new("JweDecrypt");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert_eq!(log.render_for_test(), "invalid format");
    }

    #[tokio::test]
    async fn compile_plugins_seals_invalid_resolved_credentials_before_jwe_construction() {
        async fn assert_sealed(config: JweDecryptConfig, expected_error: &str, exact_error: bool) {
            let mut resource = EdgionPlugins::new("invalid-jwe", Default::default());
            resource.metadata.namespace = Some("default".to_string());
            resource.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::JweDecrypt(config))]);

            let compiled = compile_plugins(&resource);
            assert!(!compiled.is_activatable);
            assert!(!compiled.retain_previous_on_invalid_update);
            assert_eq!(compiled.preparse_errors.len(), 1);
            if exact_error {
                assert_eq!(compiled.preparse_errors[0], expected_error);
            } else {
                assert!(
                    compiled.preparse_errors[0].starts_with(expected_error),
                    "unexpected validation error: {}",
                    compiled.preparse_errors[0]
                );
            }
            assert_eq!(compiled.plugin_runtime.request_plugins_count(), 1);

            let filter = compiled
                .plugin_runtime
                .request_plugins_iter()
                .next()
                .expect("sealed reject filter");
            assert_eq!(filter.name(), "RejectInvalidConfig");

            let mut session = MockPluginSession::new();
            let mut log = PluginLog::new(filter.name());
            let result = filter.run_request(&mut session, &mut log).await;
            assert_eq!(
                result,
                PluginRunningResult::ErrResponse {
                    status: 500,
                    body: Some(Bytes::from_static(br#"{"message":"plugin configuration error"}"#)),
                }
            );
            assert_eq!(log.render_for_test(), "err: invalid plugin config\nerr: JweDecrypt");
        }

        let mut invalid_encoding = config_with_key(&test_key());
        invalid_encoding.resolved_credential = Some(ResolvedJweCredential {
            secret: Some("not-base64!".to_string()),
        });
        assert_sealed(
            invalid_encoding,
            "requestPlugins[0] (JweDecrypt): resolvedCredential.secret is not valid base64:",
            false,
        )
        .await;

        let invalid_length = config_with_key(b"too-short");
        assert_sealed(
            invalid_length,
            "requestPlugins[0] (JweDecrypt): resolvedCredential.secret decoded length must be 32 bytes for A256GCM (got 9)",
            true,
        )
        .await;
    }
}
