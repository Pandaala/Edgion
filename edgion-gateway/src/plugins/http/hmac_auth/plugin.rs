//! HMAC Auth plugin implementation.
//!
//! Implements HMAC HTTP Signature verification with:
//! - Authorization / Proxy-Authorization parsing
//! - HMAC-SHA256/384/512 verification
//! - clock skew validation via Date / X-Date
//! - enforceHeaders validation

use crate::plugins::http::common::auth_common::{prepare_sensitive_headers, set_sensitive_request_header};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{digest::OutputSizeUser, Hmac, Mac};
#[cfg(test)]
use sha2::Digest;
use sha2::{Sha256, Sha384, Sha512};

use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, PreparedAuthChallenge,
};
use crate::plugins::runtime::body_gate::{
    body_read_failure_result, over_plugin_limit_len, plugin_limit_bytes, record_body_read_error_status,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use crate::request_body::BodyReadError;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    BodyRequirement, HmacAlgorithm, HmacAuthConfig, HmacCredential, HmacMatchConfig,
};
#[cfg(test)]
use edgion_resources::EdgionErrorCode;

type HmacSha256 = Hmac<Sha256>;
type HmacSha384 = Hmac<Sha384>;
type HmacSha512 = Hmac<Sha512>;

/// Parsed HMAC authorization parameters.
struct HmacAuthParams<'a> {
    username: Cow<'a, str>,
    algorithm: HmacAlgorithm,
    headers: Vec<String>,
    signature: Vec<u8>,
}

impl std::fmt::Debug for HmacAuthParams<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HmacAuthParams").finish_non_exhaustive()
    }
}

/// HMAC Auth plugin.
pub struct HmacAuth {
    auth_failure_delay_ms: u64,
    auth_challenge: PreparedAuthChallenge,
    anonymous: Option<String>,
    hide_credentials: bool,
    verifier: HmacVerifier,
}

struct PreparedEnforcedHeader {
    canonical: String,
    configured: String,
}

struct PreparedHmacCredential {
    secret: Vec<u8>,
    headers: Vec<(String, http::HeaderValue)>,
    group: String,
}

impl PreparedHmacCredential {
    fn new(credential: HmacCredential) -> Self {
        Self {
            secret: credential.secret,
            headers: prepare_sensitive_headers(credential.headers),
            group: credential.group,
        }
    }
}

pub(crate) struct HmacVerifier {
    algorithms: Vec<HmacAlgorithm>,
    clock_skew: u64,
    enforce_headers: Option<Vec<PreparedEnforcedHeader>>,
    validate_request_body: bool,
    credentials: Arc<HashMap<String, PreparedHmacCredential>>,
    body_requirement: BodyRequirement,
    logical_limit_bytes: Option<u64>,
}

pub(crate) enum HmacVerifyOutcome {
    Verified { username: String },
    NoMatch(HmacVerifyFailure),
    CaptureFailure(BodyReadError),
    LogicalBodyLimit(BodyReadError),
    DigestReadFailure(BodyReadError),
}

impl std::fmt::Debug for HmacVerifyOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verified { .. } => f.debug_struct("Verified").finish_non_exhaustive(),
            Self::NoMatch(reason) => f.debug_tuple("NoMatch").field(reason).finish(),
            Self::CaptureFailure(reason) => f.debug_tuple("CaptureFailure").field(reason).finish(),
            Self::LogicalBodyLimit(reason) => f.debug_tuple("LogicalBodyLimit").field(reason).finish(),
            Self::DigestReadFailure(reason) => f.debug_tuple("DigestReadFailure").field(reason).finish(),
        }
    }
}

#[derive(Debug)]
pub(crate) enum HmacVerifyFailure {
    MissingAuth,
    NoCredentials,
    BadFormat,
    AlgorithmNotAllowed,
    UnknownCredential,
    MissingSignedDate,
    BadTimestamp(&'static str),
    MissingRequiredHeader(String),
    MissingSignedHeader(&'static str),
    BadSignature,
    DigestNotSigned,
    BadDigest(&'static str),
}

enum BodyDigestError {
    Invalid(&'static str),
    Read(BodyReadError),
}

impl HmacVerifier {
    pub(crate) fn new(config: &HmacMatchConfig, body_requirement: Option<BodyRequirement>) -> Self {
        let intrinsic = config.validate_request_body.then(BodyRequirement::fail_close);
        let body_requirement =
            BodyRequirement::resolve(body_requirement, intrinsic).unwrap_or_else(BodyRequirement::fail_close);
        let logical_limit_bytes = plugin_limit_bytes(&body_requirement);
        Self {
            algorithms: config.algorithms.clone(),
            clock_skew: config.clock_skew,
            enforce_headers: config.enforce_headers.as_ref().map(|headers| {
                headers
                    .iter()
                    .map(|header| PreparedEnforcedHeader {
                        canonical: header.to_ascii_lowercase(),
                        configured: header.clone(),
                    })
                    .collect()
            }),
            validate_request_body: config.validate_request_body,
            credentials: Arc::new(
                config
                    .resolved_credentials
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(username, credential)| (username, PreparedHmacCredential::new(credential)))
                    .collect(),
            ),
            body_requirement,
            logical_limit_bytes,
        }
    }

    pub(crate) fn body_requirement(&self) -> &BodyRequirement {
        &self.body_requirement
    }

    fn credential(&self, username: &str) -> Option<&PreparedHmacCredential> {
        self.credentials.get(username)
    }

    pub(crate) fn verify_headers(&self, session: &dyn PluginSession) -> HmacVerifyOutcome {
        Self::verify_headers_with(
            &self.algorithms,
            self.clock_skew,
            self.enforce_headers.as_deref(),
            self.validate_request_body,
            Some(&self.credentials),
            session,
        )
    }

    fn verify_headers_with(
        algorithms: &[HmacAlgorithm],
        clock_skew: u64,
        enforce_headers: Option<&[PreparedEnforcedHeader]>,
        validate_request_body: bool,
        credentials: Option<&HashMap<String, PreparedHmacCredential>>,
        session: &dyn PluginSession,
    ) -> HmacVerifyOutcome {
        let Some(auth_header) = session
            .header_str("authorization")
            .or_else(|| session.header_str("proxy-authorization"))
        else {
            return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::MissingAuth);
        };
        let Some(credentials) = credentials.filter(|credentials| !credentials.is_empty()) else {
            return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::NoCredentials);
        };
        let Ok(params) = HmacAuth::parse_authorization(auth_header) else {
            return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::BadFormat);
        };
        if !algorithms.contains(&params.algorithm) {
            return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::AlgorithmNotAllowed);
        }
        let Some(credential) = credentials.get(params.username.as_ref()) else {
            return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::UnknownCredential);
        };
        let Some(signed_date) = params
            .headers
            .iter()
            .find(|header| header.as_str() == "x-date")
            .or_else(|| params.headers.iter().find(|header| header.as_str() == "date"))
        else {
            return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::MissingSignedDate);
        };
        if let Err(message) = HmacAuth::validate_clock_skew(session, clock_skew, signed_date) {
            return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::BadTimestamp(message));
        }
        if let Some(required) = enforce_headers {
            let signed: HashSet<&str> = params.headers.iter().map(String::as_str).collect();
            if let Some(header) = required
                .iter()
                .find(|header| !signed.contains(header.canonical.as_str()))
            {
                return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::MissingRequiredHeader(format!(
                    "Missing required signed header: {}",
                    header.configured
                )));
            }
        }
        let signing_string = match HmacAuth::build_signing_string(session, &params.headers) {
            Ok(value) => value,
            Err(message) => return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::MissingSignedHeader(message)),
        };
        if !HmacAuth::verify_signature(params.algorithm, &credential.secret, &signing_string, &params.signature) {
            return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::BadSignature);
        }
        drop(signing_string);
        drop(params.signature);
        if validate_request_body && !params.headers.iter().any(|header| header == "digest") {
            return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::DigestNotSigned);
        }
        drop(params.headers);
        HmacVerifyOutcome::Verified {
            username: params.username.into_owned(),
        }
    }

    #[cfg(test)]
    pub(crate) fn credentials_ptr(&self) -> *const () {
        Arc::as_ptr(&self.credentials).cast()
    }

    pub(crate) async fn verify(&self, session: &mut dyn PluginSession) -> HmacVerifyOutcome {
        let username = match self.verify_headers(session) {
            HmacVerifyOutcome::Verified { username } => username,
            other => return other,
        };
        if self.validate_request_body {
            let expected = match HmacAuth::expected_body_digest(session) {
                Ok(value) => value,
                Err(message) => return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::BadDigest(message)),
            };
            let body_len = match session.ensure_request_body(None).await {
                Ok(len) => len,
                Err(error) => return HmacVerifyOutcome::CaptureFailure(error),
            };
            if let Some(error) = over_plugin_limit_len(self.logical_limit_bytes, body_len) {
                return HmacVerifyOutcome::LogicalBodyLimit(error);
            }
            match HmacAuth::verify_body_digest_expected(session, &expected).await {
                Ok(()) => {}
                Err(BodyDigestError::Invalid(message)) => {
                    return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::BadDigest(message));
                }
                Err(BodyDigestError::Read(error)) => return HmacVerifyOutcome::DigestReadFailure(error),
            }
        }
        HmacVerifyOutcome::Verified { username }
    }
}

impl HmacAuth {
    /// Create plugin for runtime.
    pub fn create(config: &HmacAuthConfig, body_requirement: Option<BodyRequirement>) -> Box<dyn RequestFilter> {
        Box::new(Self::with_body_requirement(config, body_requirement))
    }

    /// Create plugin instance.
    pub fn new(config: &HmacAuthConfig) -> Self {
        Self::with_body_requirement(config, None)
    }

    fn with_body_requirement(config: &HmacAuthConfig, body_requirement: Option<BodyRequirement>) -> Self {
        let verifier = HmacVerifier::new(&config.match_config, body_requirement);
        Self {
            auth_failure_delay_ms: config.auth_failure_delay_ms,
            auth_challenge: PreparedAuthChallenge::new("hmac", &config.realm),
            anonymous: config.anonymous.clone(),
            hide_credentials: config.hide_credentials,
            verifier,
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
        apply_auth_failure_delay(self.auth_failure_delay_ms).await;
        let _ = send_gateway_auth_error_response(session, status, &self.auth_challenge, None, message).await;
        PluginRunningResult::ErrTerminateRequest
    }

    fn set_anonymous_headers(&self, session: &mut dyn PluginSession, username: &str) {
        let _ = set_sensitive_request_header(session, "X-Anonymous-Consumer", "true");
        let _ = set_sensitive_request_header(session, "X-Consumer-Username", username);
    }

    fn split_auth_params(input: &str) -> impl Iterator<Item = &str> {
        let mut in_quotes = false;
        let mut escaped = false;
        input
            .split(move |ch| {
                if escaped {
                    escaped = false;
                    return false;
                }
                if in_quotes && ch == '\\' {
                    escaped = true;
                    return false;
                }
                if ch == '"' {
                    in_quotes = !in_quotes;
                    return false;
                }
                ch == ',' && !in_quotes
            })
            .map(str::trim)
    }

    fn parse_quoted_value(raw: &str) -> Result<Cow<'_, str>, &'static str> {
        let trimmed = raw.trim();
        if trimmed.len() < 2 || !trimmed.starts_with('"') || !trimmed.ends_with('"') {
            return Err("HMAC parameter value must be quoted");
        }

        let inner = &trimmed[1..trimmed.len() - 1];
        if !inner.contains('\\') {
            return Ok(Cow::Borrowed(inner));
        }
        let mut out = String::with_capacity(inner.len());
        let mut remaining = inner;
        while let Some(pos) = remaining.find('\\') {
            out.push_str(&remaining[..pos]);
            remaining = &remaining[pos + 1..];
            let ch = remaining.chars().next().ok_or("Invalid escape in quoted value")?;
            out.push(ch);
            remaining = &remaining[ch.len_utf8()..];
        }
        out.push_str(remaining);
        Ok(Cow::Owned(out))
    }

    /// Parse Authorization header into HMAC parameters.
    fn parse_authorization(auth_header: &str) -> Result<HmacAuthParams<'_>, &'static str> {
        let trimmed = auth_header.trim();
        let (scheme, params_str) = trimmed.split_once(' ').ok_or("Invalid HMAC authorization format")?;
        if !scheme.eq_ignore_ascii_case("hmac") {
            return Err("Invalid HMAC authorization format");
        }
        let params_str = params_str.trim();
        if params_str.is_empty() {
            return Err("Invalid HMAC authorization format");
        }

        let mut params: HashMap<Cow<'_, str>, Cow<'_, str>> = HashMap::new();
        for part in Self::split_auth_params(params_str) {
            if part.is_empty() {
                continue;
            }
            let (key, value_raw) = part.split_once('=').ok_or("Invalid HMAC authorization format")?;
            let key = key.trim();
            let key = if key.bytes().any(|byte| byte.is_ascii_uppercase()) {
                Cow::Owned(key.to_ascii_lowercase())
            } else {
                Cow::Borrowed(key)
            };
            if key.is_empty() {
                return Err("Invalid HMAC authorization format");
            }
            if params.contains_key(key.as_ref()) {
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
        // Required values own their storage or borrow the original header, not this map.
        drop(params);

        let algorithm = HmacAlgorithm::parse(&algorithm_raw).ok_or("Unsupported algorithm")?;
        drop(algorithm_raw);
        let headers: Vec<String> = headers_raw.split_whitespace().map(|h| h.to_ascii_lowercase()).collect();
        drop(headers_raw);
        if headers.is_empty() {
            return Err("Invalid HMAC authorization format");
        }
        let mut seen = HashSet::new();
        for h in &headers {
            if !seen.insert(h.as_str()) {
                return Err("Invalid HMAC authorization format");
            }
        }
        drop(seen);

        let signature = STANDARD
            .decode(signature_raw.as_bytes())
            .map_err(|_| "Invalid HMAC authorization format")?;

        Ok(HmacAuthParams {
            username,
            algorithm,
            headers,
            signature,
        })
    }

    /// Build signing string from request according to signed headers list.
    fn build_signing_string(session: &dyn PluginSession, headers_list: &[String]) -> Result<String, &'static str> {
        let estimated_capacity = headers_list
            .iter()
            .fold(0usize, |capacity, header| {
                capacity.saturating_add(header.len()).saturating_add(2)
            })
            .saturating_add(headers_list.len().saturating_sub(1));
        let mut signing = String::with_capacity(estimated_capacity);

        for (index, header_name) in headers_list.iter().enumerate() {
            if index != 0 {
                signing.push('\n');
            }
            if header_name == "@request-target" || header_name == "(request-target)" {
                let path = session.get_path();
                let query = session.request_query();
                let method = session.get_method();

                signing.push_str(header_name);
                signing.push_str(": ");
                for ch in method.chars() {
                    signing.push(ch.to_ascii_lowercase());
                }
                signing.push(' ');
                signing.push_str(path);
                if let Some(query) = query.filter(|query| !query.is_empty()) {
                    signing.push('?');
                    signing.push_str(query);
                }
                continue;
            }

            let Some(value) = session.header_str(header_name) else {
                return Err("Missing signed header");
            };
            signing.push_str(header_name);
            signing.push_str(": ");
            signing.push_str(value);
        }

        Ok(signing)
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
    /// Reject malformed public lengths before key setup and input processing.
    fn verify_signature(algorithm: HmacAlgorithm, secret: &[u8], signing_string: &str, actual: &[u8]) -> bool {
        match algorithm {
            HmacAlgorithm::HmacSha256 => {
                if actual.len() != <HmacSha256 as OutputSizeUser>::output_size() {
                    return false;
                }
                let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
                    return false;
                };
                mac.update(signing_string.as_bytes());
                mac.verify_slice(actual).is_ok()
            }
            HmacAlgorithm::HmacSha384 => {
                if actual.len() != <HmacSha384 as OutputSizeUser>::output_size() {
                    return false;
                }
                let Ok(mut mac) = HmacSha384::new_from_slice(secret) else {
                    return false;
                };
                mac.update(signing_string.as_bytes());
                mac.verify_slice(actual).is_ok()
            }
            HmacAlgorithm::HmacSha512 => {
                if actual.len() != <HmacSha512 as OutputSizeUser>::output_size() {
                    return false;
                }
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
    /// Requires the request body to have been captured before hashing. Only
    /// SHA-256 is accepted.
    ///
    /// A failed read records the matching `EdgionErrorCode` before denying, using
    /// the same `BodyReadError` -> `EdgionErrorCode` mapping as
    /// `body_gate.rs::body_read_failure_result` and `wasm/plugin.rs` — the
    /// returned `&'static str` is the client-visible denial reason and must
    /// stay opaque, so `ctx.record_error` is the only place the specific cause
    /// becomes observable. `NotBuffered` is excluded: the read adapter already
    /// recorded which refusal, and double-counting it here would inflate that
    /// status.
    ///
    /// Initial capture and logical-size failures are handled before this helper
    /// through `body_read_failure_result`, preserving the generic gate's
    /// 413/408/500 mapping. A later hashing failure reaches this helper and uses
    /// the plugin's opaque authentication error response.
    #[cfg(test)]
    async fn verify_body_digest(session: &mut dyn PluginSession) -> Result<(), &'static str> {
        let expected = Self::expected_body_digest(session)?;
        match Self::verify_body_digest_expected(session, &expected).await {
            Ok(()) => Ok(()),
            Err(BodyDigestError::Invalid(message)) => Err(message),
            Err(BodyDigestError::Read(error)) => {
                record_body_read_error_status(session, &error);
                Err("request body unavailable")
            }
        }
    }

    async fn verify_body_digest_expected(
        session: &mut dyn PluginSession,
        expected: &str,
    ) -> Result<(), BodyDigestError> {
        let body_hash = session.request_body_sha256().await.map_err(BodyDigestError::Read)?;
        if !Self::body_digest_matches(&body_hash, expected) {
            return Err(BodyDigestError::Invalid("Body digest mismatch"));
        }
        Ok(())
    }

    fn body_digest_matches(body_hash: &[u8; 32], expected: &str) -> bool {
        // A SHA-256 digest occupies exactly 44 bytes in padded standard Base64.
        let mut encoded = [0u8; 44];
        STANDARD
            .encode_slice(body_hash, &mut encoded)
            .is_ok_and(|len| encoded[..len] == *expected.as_bytes())
    }

    /// Pure `Digest` header check: `SHA-256=<base64(sha256(body))>`.
    #[cfg(test)]
    fn check_body_digest(body: &[u8], digest_header: &str) -> Result<(), &'static str> {
        Self::check_body_digest_hash(&Sha256::digest(body), digest_header)
    }

    #[cfg(test)]
    fn check_body_digest_hash(body_hash: &[u8], digest_header: &str) -> Result<(), &'static str> {
        let expected = Self::parse_body_digest_header(digest_header)?;
        let computed = STANDARD.encode(body_hash);
        if computed != expected {
            return Err("Body digest mismatch");
        }
        Ok(())
    }

    fn expected_body_digest(session: &dyn PluginSession) -> Result<String, &'static str> {
        let digest_header = session.header_str("digest").ok_or("Missing Digest header")?;
        Self::parse_body_digest_header(digest_header).map(str::to_owned)
    }

    fn parse_body_digest_header(digest_header: &str) -> Result<&str, &'static str> {
        // Split on the FIRST '=' so base64 padding stays with the value.
        let (algo, expected) = digest_header.split_once('=').ok_or("Malformed Digest header")?;
        if !algo.trim().eq_ignore_ascii_case("SHA-256") {
            return Err("Unsupported Digest algorithm");
        }
        Ok(expected.trim())
    }

    /// Validate Date/X-Date clock skew.
    fn validate_clock_skew(
        session: &dyn PluginSession,
        max_skew: u64,
        signed_date_header: &str,
    ) -> Result<(), &'static str> {
        let date_str = session.header_str(signed_date_header).ok_or("Missing Date header")?;

        let parsed = DateTime::parse_from_rfc2822(date_str)
            .or_else(|_| DateTime::parse_from_rfc3339(date_str))
            .map_err(|_| "Invalid Date header")?;

        let request_ts = parsed.timestamp();
        let now_ts = Utc::now().timestamp();
        let diff = now_ts.abs_diff(request_ts);
        if diff > max_skew {
            return Err("Clock skew exceeded");
        }

        Ok(())
    }

    fn set_upstream_headers(&self, session: &mut dyn PluginSession, credential: &PreparedHmacCredential) {
        for (name, value) in &credential.headers {
            let _ = session.set_request_header_value(name, value.clone());
        }
    }

    fn hide_credentials(&self, session: &mut dyn PluginSession) {
        if !self.hide_credentials {
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
        "HmacAuth"
    }

    // Body requirement is declared on `EdgionPlugin::body_requirement` (schema
    // layer) so the controller and gateway agree. HMAC owns the resolved
    // requirement at runtime: cheap authentication runs first, then body capture
    // and streaming digest verification run fail-closed.

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let username = match self.verifier.verify(session).await {
            HmacVerifyOutcome::Verified { username } => username,
            HmacVerifyOutcome::CaptureFailure(error) | HmacVerifyOutcome::LogicalBodyLimit(error) => {
                return body_read_failure_result(session, self.verifier.body_requirement(), plugin_log, &error);
            }
            HmacVerifyOutcome::DigestReadFailure(error) => {
                if error == BodyReadError::CapacityExceeded {
                    return body_read_failure_result(session, self.verifier.body_requirement(), plugin_log, &error);
                }
                record_body_read_error_status(session, &error);
                return self
                    .reject(session, plugin_log, 401, "request body unavailable", "Deny body digest")
                    .await;
            }
            HmacVerifyOutcome::NoMatch(HmacVerifyFailure::MissingAuth) => {
                if let Some(anonymous) = &self.anonymous {
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
            HmacVerifyOutcome::NoMatch(HmacVerifyFailure::NoCredentials) => {
                plugin_log.push_err("no creds configured");
                apply_auth_failure_delay(self.auth_failure_delay_ms).await;
                let _ = send_gateway_auth_error_response(
                    session,
                    500,
                    &self.auth_challenge,
                    None,
                    "Internal gateway error",
                )
                .await;
                return PluginRunningResult::ErrTerminateRequest;
            }
            HmacVerifyOutcome::NoMatch(failure) => {
                let (message, log_code) = match failure {
                    HmacVerifyFailure::BadFormat => ("Invalid HMAC authorization format", "Deny bad format"),
                    HmacVerifyFailure::AlgorithmNotAllowed => ("Algorithm not allowed", "Deny bad algorithm"),
                    HmacVerifyFailure::UnknownCredential => ("Invalid credentials", "Deny missing key"),
                    HmacVerifyFailure::MissingSignedDate => (
                        "Missing required signed header: date or x-date",
                        "Deny missing date signed",
                    ),
                    HmacVerifyFailure::BadTimestamp(message) => (message, "Deny bad timestamp"),
                    HmacVerifyFailure::MissingRequiredHeader(message) => {
                        return self
                            .reject(session, plugin_log, 401, &message, "Deny missing required header")
                            .await;
                    }
                    HmacVerifyFailure::MissingSignedHeader(message) => (message, "Deny missing header"),
                    HmacVerifyFailure::BadSignature => ("Invalid signature", "Deny bad signature"),
                    HmacVerifyFailure::DigestNotSigned => {
                        ("Missing required signed header: digest", "Deny digest not signed")
                    }
                    HmacVerifyFailure::BadDigest(message) => (message, "Deny body digest"),
                    HmacVerifyFailure::MissingAuth => ("Missing Authorization header", "Deny missing auth"),
                    HmacVerifyFailure::NoCredentials => ("No credentials configured", "no creds configured"),
                };
                return self.reject(session, plugin_log, 401, message, log_code).await;
            }
        };
        let Some(credential) = self.verifier.credential(&username) else {
            plugin_log.push_err("credential disappeared");
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        };
        self.set_upstream_headers(session, credential);
        self.hide_credentials(session);

        plugin_log.push_fmt(
            "OK group={} user {}",
            la![credential.group.as_str(), sensitive(&username)],
        );
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/hmac_auth/plugin_test.rs"]
mod tests;
