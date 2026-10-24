//! JWE Decrypt plugin implementation.
//!
//! Supports compact JWE (alg=dir, enc=A256GCM).

use crate::plugins::http::common::auth_common::set_sensitive_request_header;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, set_claims_headers,
};
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    ClaimHeaderProjectionPlan, JweContentEncryption, JweDecryptConfig, ResolvedJweCredentialError,
};

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
            JweFailure::NoKey => 401,
            JweFailure::KeyLenErr => 401,
            JweFailure::DecryptFail => 401,
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

struct ProtectedHeader {
    alg: Option<serde_json::Value>,
    enc: Option<serde_json::Value>,
    has_zip: bool,
    has_crit: bool,
}

struct ProtectedHeaderVisitor;

#[derive(Deserialize)]
struct ProtectedHeaderName<'a>(#[serde(borrow)] Cow<'a, str>);

enum ProtectedHeaderField {
    Alg,
    Enc,
    Zip,
    Crit,
    Other,
}

impl<'de> Visitor<'de> for ProtectedHeaderVisitor {
    type Value = ProtectedHeader;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JWE protected header JSON object with unique member names")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut seen = HashSet::new();
        let mut alg = None;
        let mut enc = None;
        let mut has_zip = false;
        let mut has_crit = false;

        while let Some(ProtectedHeaderName(field)) = map.next_key::<ProtectedHeaderName<'de>>()? {
            let field_kind = match field.as_ref() {
                "alg" => ProtectedHeaderField::Alg,
                "enc" => ProtectedHeaderField::Enc,
                "zip" => ProtectedHeaderField::Zip,
                "crit" => ProtectedHeaderField::Crit,
                _ => ProtectedHeaderField::Other,
            };
            if !seen.insert(field) {
                return Err(de::Error::custom("duplicate JWE protected header member"));
            }

            match field_kind {
                ProtectedHeaderField::Alg => alg = Some(map.next_value()?),
                ProtectedHeaderField::Enc => enc = Some(map.next_value()?),
                ProtectedHeaderField::Zip => {
                    has_zip = true;
                    map.next_value::<IgnoredAny>()?;
                }
                ProtectedHeaderField::Crit => {
                    has_crit = true;
                    map.next_value::<IgnoredAny>()?;
                }
                ProtectedHeaderField::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }

        Ok(ProtectedHeader {
            alg,
            enc,
            has_zip,
            has_crit,
        })
    }
}

impl<'de> Deserialize<'de> for ProtectedHeader {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(ProtectedHeaderVisitor)
    }
}

#[allow(clippy::large_enum_variant)]
enum PreparedDecryptor {
    Ready(Aes256Gcm),
    Missing,
    DecodeFailed,
    InvalidLength,
}

/// JWE Decrypt plugin.
pub struct JweDecrypt {
    config: JweDecryptConfig,
    claim_header_projection: ClaimHeaderProjectionPlan,
    decryptor: PreparedDecryptor,
}

impl JweDecrypt {
    /// Create plugin from config and namespace.
    pub fn new(config: &JweDecryptConfig, plugin_namespace: String) -> Self {
        let mut config = config.clone();
        let claim_header_projection = config.compile_claim_header_projection().unwrap_or_default();
        let decryptor = Self::prepare_decryptor(&mut config, &plugin_namespace);
        Self {
            config,
            claim_header_projection,
            decryptor,
        }
    }

    fn extract_token<'a>(&self, session: &'a dyn PluginSession) -> Option<&'a str> {
        let raw_value = session.header_str(&self.config.header)?;
        let trimmed = raw_value.trim();

        let token = if let Some(prefix) = &self.config.strip_prefix {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                rest.trim()
            } else {
                trimmed
            }
        } else {
            trimmed
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

    fn prepare_decryptor(config: &mut JweDecryptConfig, _plugin_namespace: &str) -> PreparedDecryptor {
        let key = config.resolved_key_material();
        config.resolved_credential = None;
        let key = match key {
            Ok(key) => key,
            Err(ResolvedJweCredentialError::Missing) => return PreparedDecryptor::Missing,
            Err(ResolvedJweCredentialError::TransportDecodeFailed | ResolvedJweCredentialError::SecretDecodeFailed) => {
                return PreparedDecryptor::DecodeFailed;
            }
            Err(ResolvedJweCredentialError::InvalidLength { .. }) => return PreparedDecryptor::InvalidLength,
        };

        match Aes256Gcm::new_from_slice(&key) {
            Ok(cipher) => PreparedDecryptor::Ready(cipher),
            Err(_) => PreparedDecryptor::InvalidLength,
        }
    }

    fn parse_compact_jwe<'a>(&self, token: &'a str) -> Result<ParsedJwe<'a>, JweFailure> {
        let mut segments = token.split('.');
        let protected_segment = segments.next().ok_or(JweFailure::InvalidFormat)?;
        let encrypted_key_segment = segments.next().ok_or(JweFailure::InvalidFormat)?;
        let iv_segment = segments.next().ok_or(JweFailure::InvalidFormat)?;
        let ciphertext_segment = segments.next().ok_or(JweFailure::InvalidFormat)?;
        let tag_segment = segments.next().ok_or(JweFailure::InvalidFormat)?;
        if segments.next().is_some() {
            return Err(JweFailure::InvalidFormat);
        }

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
        let protected_header: ProtectedHeader =
            serde_json::from_slice(&protected_bytes).map_err(|_| JweFailure::InvalidFormat)?;

        let alg = protected_header
            .alg
            .as_ref()
            .and_then(|v| v.as_str())
            .ok_or(JweFailure::MissingHeader)?;
        if alg != self.config.key_management_algorithm.as_str() {
            return Err(JweFailure::BadAlg);
        }

        let enc_raw = protected_header
            .enc
            .as_ref()
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
        if protected_header.has_zip || protected_header.has_crit {
            return Err(JweFailure::InvalidFormat);
        }

        Ok(ParsedJwe {
            protected_segment,
            iv_segment,
            ciphertext_segment,
            tag_segment,
        })
    }

    fn decrypt_compact_jwe(&self, token: &str, cipher: &Aes256Gcm) -> Result<String, JweFailure> {
        let parsed = self.parse_compact_jwe(token)?;

        let mut iv = [0u8; 12];
        let iv_len = URL_SAFE_NO_PAD
            .decode_slice(parsed.iv_segment, &mut iv)
            .map_err(|_| JweFailure::InvalidFormat)?;
        let mut ciphertext = URL_SAFE_NO_PAD
            .decode(parsed.ciphertext_segment)
            .map_err(|_| JweFailure::InvalidFormat)?;
        let mut tag_bytes = [0u8; 16];
        let tag_len = URL_SAFE_NO_PAD
            .decode_slice(parsed.tag_segment, &mut tag_bytes)
            .map_err(|_| JweFailure::InvalidFormat)?;

        // RFC 7518 A256GCM uses 96-bit IV and 128-bit tag.
        if iv_len != iv.len() || tag_len != tag_bytes.len() {
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
        "JweDecrypt"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let token = match self.extract_token(session) {
            Some(token) => token,
            None if self.config.strict => {
                return self
                    .reject_custom(session, plugin_log, 401, "JWE token required", "no token")
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
            PreparedDecryptor::Ready(cipher) => cipher,
            PreparedDecryptor::InvalidLength => {
                return self.reject(session, plugin_log, JweFailure::KeyLenErr).await;
            }
            PreparedDecryptor::Missing | PreparedDecryptor::DecodeFailed => {
                return self.reject(session, plugin_log, JweFailure::NoKey).await;
            }
        };

        let plaintext = match self.decrypt_compact_jwe(token, decryptor) {
            Ok(plaintext) => plaintext,
            Err(err) => return self.reject(session, plugin_log, err).await,
        };

        if Self::contains_header_control_chars(&plaintext) {
            drop(plaintext);
            return self.reject(session, plugin_log, JweFailure::InvalidFormat).await;
        }

        if set_sensitive_request_header(session, &self.config.forward_header, &plaintext).is_err() {
            drop(plaintext);
            return self.reject(session, plugin_log, JweFailure::InvalidFormat).await;
        }

        if !self.claim_header_projection.entries().is_empty() {
            if let Ok(payload_json) = serde_json::from_str::<serde_json::Value>(&plaintext) {
                set_claims_headers(
                    session,
                    &payload_json,
                    &self.claim_header_projection,
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
#[path = "../../../../tests/unit/plugins/http/jwe_decrypt/plugin_test.rs"]
mod tests;
