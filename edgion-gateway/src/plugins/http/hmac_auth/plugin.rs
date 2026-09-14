//! HMAC Auth plugin implementation.
//!
//! Implements HMAC HTTP Signature verification with:
//! - Authorization / Proxy-Authorization parsing
//! - HMAC-SHA256/384/512 verification
//! - clock skew validation via Date / X-Date
//! - enforceHeaders validation

use crate::plugins::http::common::auth_common::set_sensitive_request_header;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
#[cfg(test)]
use sha2::Digest;
use sha2::{Sha256, Sha384, Sha512};

use crate::plugins::http::common::auth_common::{apply_auth_failure_delay, send_gateway_auth_error_response};
use crate::plugins::runtime::body_gate::{
    body_read_failure_result, over_plugin_limit_len, record_body_read_error_status,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::traits::session::BodyReadError;
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
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
struct HmacAuthParams {
    username: String,
    algorithm: HmacAlgorithm,
    headers: Vec<String>,
    signature: Vec<u8>,
}

impl std::fmt::Debug for HmacAuthParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HmacAuthParams").finish_non_exhaustive()
    }
}

/// HMAC Auth plugin.
pub struct HmacAuth {
    name: String,
    auth_failure_delay_ms: u64,
    realm: String,
    anonymous: Option<String>,
    hide_credentials: bool,
    verifier: HmacVerifier,
}

struct PreparedEnforcedHeader {
    canonical: String,
    configured: String,
}

pub(crate) struct HmacVerifier {
    algorithms: Vec<HmacAlgorithm>,
    clock_skew: u64,
    enforce_headers: Option<Vec<PreparedEnforcedHeader>>,
    validate_request_body: bool,
    credentials: Arc<HashMap<String, HmacCredential>>,
    body_requirement: BodyRequirement,
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
            credentials: Arc::new(config.resolved_credentials.clone().unwrap_or_default()),
            body_requirement,
        }
    }

    pub(crate) fn body_requirement(&self) -> &BodyRequirement {
        &self.body_requirement
    }

    fn credential(&self, username: &str) -> Option<&HmacCredential> {
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
        credentials: Option<&HashMap<String, HmacCredential>>,
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
        let Some(credential) = credentials.get(&params.username) else {
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
        if validate_request_body && !params.headers.iter().any(|header| header == "digest") {
            return HmacVerifyOutcome::NoMatch(HmacVerifyFailure::DigestNotSigned);
        }
        HmacVerifyOutcome::Verified {
            username: params.username,
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
            if let Some(error) = over_plugin_limit_len(&self.body_requirement, body_len) {
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
            name: "HmacAuth".to_string(),
            auth_failure_delay_ms: config.auth_failure_delay_ms,
            realm: config.realm.clone(),
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
        let _ = send_gateway_auth_error_response(session, status, "hmac", &self.realm, None, message).await;
        PluginRunningResult::ErrTerminateRequest
    }

    fn set_anonymous_headers(&self, session: &mut dyn PluginSession, username: &str) {
        let _ = set_sensitive_request_header(session, "X-Anonymous-Consumer", "true");
        let _ = set_sensitive_request_header(session, "X-Consumer-Username", username);
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
                let query = session.get_query();
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
                    signing.push_str(&query);
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
        let computed = STANDARD.encode(body_hash);
        if computed != expected {
            return Err(BodyDigestError::Invalid("Body digest mismatch"));
        }
        Ok(())
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

    fn set_upstream_headers(&self, session: &mut dyn PluginSession, credential: &HmacCredential) {
        for (name, value) in &credential.headers {
            if name.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0') {
                continue;
            }
            if value.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0') {
                continue;
            }
            let _ = set_sensitive_request_header(session, name, value);
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
        &self.name
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
                let _ =
                    send_gateway_auth_error_response(session, 500, "hmac", &self.realm, None, "Internal gateway error")
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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use bytes::Bytes;
    use mockall::Sequence;

    #[test]
    fn credential_debug_omits_identity_and_signature() {
        let params = HmacAuthParams {
            username: "private-identity".to_string(),
            algorithm: HmacAlgorithm::HmacSha256,
            headers: vec!["private-header".to_string()],
            signature: b"private-signature".to_vec(),
        };
        let outcome = HmacVerifyOutcome::Verified {
            username: params.username.clone(),
        };
        for value in [&params as &dyn std::fmt::Debug, &outcome] {
            assert!(!format!("{value:?}").contains("private-"));
            assert!(!format!("{value:#?}").contains("private-"));
            assert!(!format!("{:?}", Some(value)).contains("private-"));
        }
        assert!(!format!("{params:?}").contains("112, 114"));
        assert_eq!(params.signature, b"private-signature");
        assert!(matches!(outcome, HmacVerifyOutcome::Verified { username } if username == "private-identity"));
    }

    fn create_plugin() -> HmacAuth {
        let mut config = HmacAuthConfig {
            match_config: HmacMatchConfig {
                clock_skew: 600,
                enforce_headers: Some(vec!["@request-target".to_string()]),
                ..Default::default()
            },
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

    #[test]
    fn constructor_retains_only_live_policy_and_one_verifier_generation() {
        let (plugin, source_secret_ptr) = {
            let mut config = HmacAuthConfig {
                auth_failure_delay_ms: 17,
                realm: "partners".to_string(),
                anonymous: Some("guest".to_string()),
                hide_credentials: true,
                ..Default::default()
            };
            config.resolved_credentials = Some(HashMap::from([(
                "alice".to_string(),
                HmacCredential {
                    secret: b"alice-super-secret-key".to_vec(),
                    headers: HashMap::new(),
                    group: "default".to_string(),
                },
            )]));
            let source_secret_ptr = config
                .resolved_credentials
                .as_ref()
                .and_then(|credentials| credentials.get("alice"))
                .expect("test credential must exist")
                .secret
                .as_ptr();
            (HmacAuth::new(&config), source_secret_ptr)
        };

        assert_eq!(plugin.auth_failure_delay_ms, 17);
        assert_eq!(plugin.realm, "partners");
        assert_eq!(plugin.anonymous.as_deref(), Some("guest"));
        assert!(plugin.hide_credentials);
        let credential = plugin
            .verifier
            .credential("alice")
            .expect("verifier credential must exist");
        assert_eq!(credential.secret, b"alice-super-secret-key");
        assert_ne!(credential.secret.as_ptr(), source_secret_ptr);
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
    fn canonical_signing_and_hmac_vectors_are_wire_stable() {
        let mut session = MockPluginSession::new();
        session.expect_get_path().times(2).return_const("/v1/items".to_string());
        session
            .expect_get_query()
            .times(2)
            .returning(|| Some("page=1&tag=a%20b".to_string()));
        session.expect_get_method().times(2).return_const("PoSt".to_string());
        setup_header_mocks(
            &mut session,
            &[("host", "API.Example:8443"), ("x-custom", "MiXeD/Value==;")],
        );
        let headers = ["@request-target", "host", "x-custom", "(request-target)"]
            .map(str::to_string)
            .to_vec();
        let expected = concat!(
            "@request-target: post /v1/items?page=1&tag=a%20b\n",
            "host: API.Example:8443\n",
            "x-custom: MiXeD/Value==;\n",
            "(request-target): post /v1/items?page=1&tag=a%20b"
        );

        let signing = HmacAuth::build_signing_string(&session, &headers).expect("canonical signing must succeed");

        assert_eq!(signing, expected);
        assert_eq!(signing.len(), 146);
        for (algorithm, expected_signature) in [
            (
                HmacAlgorithm::HmacSha256,
                "Zfsfak+e6CMopCzcmNeHtfKGtHH+ksXcWtI/O8qn1+A=",
            ),
            (
                HmacAlgorithm::HmacSha384,
                "UyQwM5enk4xAyaGZCW/HhOkMxj8rAvb3EfDbHbpMZMapN1hYuSs5H/iEmqpQ/9O6",
            ),
            (
                HmacAlgorithm::HmacSha512,
                "sIFwUh6tTKGSXsCoE9LqRQKlY/3Zj+Hwjat+YXYwH8xw5j/j+hhqDzqdgVUwNXcJ1IzSitgN7wvbtKYr7tB5Kw==",
            ),
        ] {
            let signature = HmacAuth::compute_hmac(algorithm, b"correct horse battery staple", &signing)
                .expect("test HMAC must succeed");
            assert_eq!(STANDARD.encode(&signature), expected_signature);
            assert!(HmacAuth::verify_signature(
                algorithm,
                b"correct horse battery staple",
                &signing,
                &signature,
            ));
        }
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
    fn test_check_empty_body_digest_matches() {
        let digest = format!("SHA-256={}", STANDARD.encode(Sha256::digest([])));
        assert!(HmacAuth::check_body_digest(b"", &digest).is_ok());
    }

    #[test]
    fn test_check_empty_body_digest_mismatch() {
        let digest = format!("SHA-256={}", STANDARD.encode(Sha256::digest(b"not empty")));
        assert_eq!(HmacAuth::check_body_digest(b"", &digest), Err("Body digest mismatch"));
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
    /// `EdgionErrorCode`, matching `body_gate.rs` / `wasm/plugin.rs`. The denial
    /// reason returned to the caller stays opaque, so `record_error` is the only
    /// place the cause is observable — collapsing the variants here would make
    /// a timeout indistinguishable from an oversize body in production.
    #[tokio::test]
    async fn verify_body_digest_records_status_per_read_error() {
        for (err, expected) in [
            (BodyReadError::TooLarge { limit: 8 }, EdgionErrorCode::BodyReadTooLarge),
            (BodyReadError::Timeout, EdgionErrorCode::BodyReadTimeout),
            (BodyReadError::Incomplete, EdgionErrorCode::BodyReadAborted),
            (BodyReadError::Unimplemented, EdgionErrorCode::BodyReadUnimplemented),
        ] {
            let mut session = MockPluginSession::new();
            setup_header_mocks(&mut session, &[("digest", "SHA-256=unused")]);
            session.expect_request_body_sha256().returning(move || Err(err.clone()));
            session
                .expect_record_error()
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
        setup_header_mocks(&mut session, &[("digest", "SHA-256=unused")]);
        session
            .expect_request_body_sha256()
            .returning(|| Err(BodyReadError::NotBuffered));
        session.expect_record_error().times(0);

        assert_eq!(
            HmacAuth::verify_body_digest(&mut session).await,
            Err("request body unavailable")
        );
    }

    #[test]
    fn test_body_requirement_declared_only_when_validate_body() {
        use edgion_resources::resources::edgion_plugins::EdgionPlugin;
        let base = HmacAuthConfig {
            match_config: HmacMatchConfig {
                resolved_credentials: Some(HashMap::new()),
                ..Default::default()
            },
            ..Default::default()
        };
        use edgion_resources::resources::edgion_plugins::BodyStage;
        assert!(EdgionPlugin::HmacAuth(base.clone())
            .body_requirement(BodyStage::Request)
            .is_none());
        assert!(!EdgionPlugin::HmacAuth(base.clone()).accepts_body(BodyStage::Request));

        let with_body = HmacAuthConfig {
            match_config: HmacMatchConfig {
                validate_request_body: true,
                ..base.match_config.clone()
            },
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

        setup_header_mocks(
            &mut session,
            &[("authorization", auth_header.as_str()), ("date", date.as_str())],
        );
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny missing date signed"));
    }

    #[tokio::test]
    async fn no_credentials_uses_generic_exact_internal_response() {
        let config = HmacAuthConfig {
            realm: "partners".to_string(),
            auth_failure_delay_ms: 0,
            ..Default::default()
        };
        let plugin = HmacAuth::new(&config);
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("authorization", "hmac unused")]);

        let mut sequence = Sequence::new();
        session
            .expect_write_response_header()
            .withf(|response, end_of_stream| {
                !end_of_stream
                    && response.status.as_u16() == 500
                    && response
                        .headers
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        == Some("text/plain")
                    && response
                        .headers
                        .get("www-authenticate")
                        .and_then(|value| value.to_str().ok())
                        == Some("hmac realm=\"partners\"")
            })
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_write_response_body()
            .withf(|body, end_of_stream| {
                *end_of_stream && body.as_deref() == Some(Bytes::from_static(b"500 Internal gateway error").as_ref())
            })
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_shutdown()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|| {});

        let mut log = PluginLog::new("HmacAuth");
        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert_eq!(log.render_for_test(), "err: no creds configured");
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
        let mut header_pairs: Vec<(&str, &str)> = vec![("authorization", auth.as_str()), ("date", date.as_str())];
        if let Some(digest_val) = digest_val.as_deref() {
            header_pairs.push(("digest", digest_val));
        }
        setup_header_mocks(&mut session, &header_pairs);
        session.expect_get_path().return_const(path.to_string());
        session.expect_get_query().returning(|| None);
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_request_body_sha256()
            .returning(move || Ok(Sha256::digest(body).into()));
        session.expect_set_request_header_value().returning(|_, value| {
            assert!(value.is_sensitive());
            Ok(())
        });
        session.expect_remove_request_header().returning(|_| Ok(()));
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});
        session
    }

    #[test]
    fn enforce_headers_are_prepared_once_in_order_with_configured_error_spelling() {
        let mut config = HmacMatchConfig {
            clock_skew: 600,
            enforce_headers: Some(vec!["X-Custom".to_string(), "X-Custom".to_string(), "Date".to_string()]),
            ..Default::default()
        };
        config.resolved_credentials = Some(HashMap::from([(
            "alice".to_string(),
            HmacCredential {
                secret: b"alice-super-secret-key".to_vec(),
                headers: HashMap::new(),
                group: "default".to_string(),
            },
        )]));
        let verifier = HmacVerifier::new(&config, None);
        let prepared = verifier
            .enforce_headers
            .as_deref()
            .expect("enforced headers must be prepared");
        assert_eq!(prepared.len(), 3);
        assert_eq!(prepared[0].canonical, "x-custom");
        assert_eq!(prepared[0].configured, "X-Custom");
        assert_eq!(prepared[1].canonical, "x-custom");
        assert_eq!(prepared[1].configured, "X-Custom");
        assert_eq!(prepared[2].canonical, "date");
        assert_eq!(prepared[2].configured, "Date");

        let session = signed_session(&["@request-target", "date"], None, b"");
        match verifier.verify_headers(&session) {
            HmacVerifyOutcome::NoMatch(HmacVerifyFailure::MissingRequiredHeader(message)) => {
                assert_eq!(message, "Missing required signed header: X-Custom");
            }
            outcome => panic!("expected first configured header to be missing, got {outcome:?}"),
        }
    }

    fn plugin_validate_body() -> HmacAuth {
        let mut config = HmacAuthConfig {
            match_config: HmacMatchConfig {
                clock_skew: 600,
                validate_request_body: true,
                ..Default::default()
            },
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
    async fn verify_headers_agrees_with_verify_on_header_only_request() {
        let plugin = create_plugin();
        let mut session = signed_session(&["@request-target", "date"], None, b"");
        let header_outcome = plugin.verifier.verify_headers(&session);
        let verify_outcome = plugin.verifier.verify(&mut session).await;
        match (header_outcome, verify_outcome) {
            (HmacVerifyOutcome::Verified { username: headers }, HmacVerifyOutcome::Verified { username: verify }) => {
                assert_eq!(headers, verify);
                assert_eq!(headers, "alice");
            }
            (headers, verify) => panic!("expected both Verified, got headers={headers:?} verify={verify:?}"),
        }
    }

    #[tokio::test]
    async fn validate_body_rejects_when_digest_not_signed() {
        // F1: digest is NOT in the signed-headers list → must reject even though
        // the signature itself is valid and a (spoofable) Digest header is present.
        let body = b"{\"a\":1}";
        let plugin = plugin_validate_body();
        let mut session = signed_session(&["@request-target", "date"], Some(&body_digest(body)), body);
        session.expect_ensure_request_body().times(0);
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
        session
            .expect_ensure_request_body()
            .returning(move |_| Ok(body.len() as u64));
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
        session
            .expect_ensure_request_body()
            .returning(move |_| Ok(real_body.len() as u64));
        let mut log = PluginLog::new("HmacAuth");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny body digest"));
    }

    #[tokio::test]
    async fn validate_body_enforces_plugin_limit_after_capture() {
        let body = b"12345";
        let mut plugin = plugin_validate_body();
        plugin.verifier.body_requirement.max_body_size = Some("4".to_string());
        let mut session = signed_session(&["@request-target", "date", "digest"], Some(&body_digest(body)), body);
        session
            .expect_ensure_request_body()
            .returning(move |_| Ok(body.len() as u64));
        session
            .expect_record_error()
            .withf(|status| *status == EdgionErrorCode::BodyReadTooLarge)
            .times(1)
            .returning(|_| ());

        let mut log = PluginLog::new("HmacAuth");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 413,
                body: None
            }
        );
    }

    #[tokio::test]
    async fn validate_body_preserves_physical_cap_failure() {
        let body = b"12345";
        let plugin = plugin_validate_body();
        let mut session = signed_session(&["@request-target", "date", "digest"], Some(&body_digest(body)), body);
        session
            .expect_ensure_request_body()
            .returning(|_| Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
        session
            .expect_record_error()
            .withf(|status| *status == EdgionErrorCode::BodyReadTooLarge)
            .times(1)
            .returning(|_| ());

        let mut log = PluginLog::new("HmacAuth");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 413,
                body: None
            }
        );
    }

    #[tokio::test]
    async fn validate_body_fails_closed_without_buffering_capability() {
        let body = b"12345";
        let plugin = plugin_validate_body();
        let mut session = signed_session(&["@request-target", "date", "digest"], Some(&body_digest(body)), body);
        session
            .expect_ensure_request_body()
            .returning(|_| Err(BodyReadError::Unimplemented));
        session
            .expect_record_error()
            .withf(|status| *status == EdgionErrorCode::BodyReadUnimplemented)
            .times(1)
            .returning(|_| ());

        let mut log = PluginLog::new("HmacAuth");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            }
        );
    }
}
