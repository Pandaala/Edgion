//! JWT Authentication plugin implementation
//!
//! Verifies JWT from header (default Authorization), query, or cookie.
//! Supports HS256/HS384/HS512 (symmetric) and RS256/RS384/RS512, ES256/ES384/ES512 (asymmetric).
//! All credentials come from K8s Secret (secret_ref or secret_refs).

use base64::Engine;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::core::conf_mgr::sync_runtime::resource_processor::get_secret;
use crate::core::plugins::edgion_plugins::common::auth_common::{
    send_auth_error_response, set_claims_headers as set_common_claims_headers, Claims,
};
use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{JwtAlgorithm, JwtAuthConfig};

use async_trait::async_trait;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};

type JwtAuthError = Box<dyn std::error::Error + Send + Sync>;
type JwtAuthResult<T> = Result<T, JwtAuthError>;

/// Token source for hide_credentials
#[derive(Debug, Clone, Copy)]
enum TokenSource {
    Header,
    Query,
    Cookie,
}

/// Credential loaded from Secret
#[derive(Debug, Clone)]
enum Credential {
    /// Symmetric key (HS256/384/512)
    Symmetric(Vec<u8>),
    /// Asymmetric public key PEM (RS256/384/512, ES256/384/512)
    Asymmetric(String),
}

/// Credentials storage
#[derive(Debug, Clone)]
enum Credentials {
    /// Single credential (from secret_ref)
    Single(Credential),
    /// Multiple credentials keyed by claim value (from secret_refs)
    Multi(HashMap<String, Credential>),
    /// No credentials configured
    None,
}

/// Verification result containing username and claims
struct VerifyResult {
    username: String,
    claims_json: Option<String>,
    /// Raw claims for claims_to_headers mapping
    claims: serde_json::Value,
}

/// JWT Authentication plugin
pub struct JwtAuth {
    name: String,
    config: JwtAuthConfig,
    /// EdgionPlugins namespace (fallback for secret_ref without namespace)
    plugin_namespace: String,
    /// Loaded credentials (lazily populated on first request)
    credentials: Arc<std::sync::RwLock<Option<Credentials>>>,
}

impl JwtAuth {
    /// Create from JwtAuthConfig with plugin namespace
    pub fn new(config: &JwtAuthConfig, plugin_namespace: String) -> Self {
        JwtAuth {
            name: "JwtAuth".to_string(),
            config: config.clone(),
            plugin_namespace,
            credentials: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    /// Convert JwtAlgorithm to jsonwebtoken::Algorithm
    fn to_jwt_algorithm(alg: JwtAlgorithm) -> Algorithm {
        match alg {
            JwtAlgorithm::HS256 => Algorithm::HS256,
            JwtAlgorithm::HS384 => Algorithm::HS384,
            JwtAlgorithm::HS512 => Algorithm::HS512,
            JwtAlgorithm::RS256 => Algorithm::RS256,
            JwtAlgorithm::RS384 => Algorithm::RS384,
            JwtAlgorithm::RS512 => Algorithm::RS512,
            JwtAlgorithm::ES256 => Algorithm::ES256,
            JwtAlgorithm::ES384 => Algorithm::ES384,
        }
    }

    /// Check if algorithm is symmetric (HS*)
    fn is_symmetric(alg: JwtAlgorithm) -> bool {
        matches!(alg, JwtAlgorithm::HS256 | JwtAlgorithm::HS384 | JwtAlgorithm::HS512)
    }

    /// Extract token from request (priority: Header > Query > Cookie)
    fn extract_token(&self, session: &dyn PluginSession) -> Option<(String, TokenSource)> {
        // 1. Header
        if let Some(v) = session.header_value(&self.config.header) {
            let token = if v.to_lowercase().starts_with("bearer ") {
                v[7..].trim().to_string()
            } else {
                v.trim().to_string()
            };
            if !token.is_empty() {
                return Some((token, TokenSource::Header));
            }
        }
        // 2. Query
        if let Some(t) = session.get_query_param(&self.config.query) {
            if !t.is_empty() {
                return Some((t, TokenSource::Query));
            }
        }
        // 3. Cookie
        if let Some(t) = session.get_cookie(&self.config.cookie) {
            if !t.is_empty() {
                return Some((t, TokenSource::Cookie));
            }
        }
        None
    }

    /// Load credentials from K8s Secret (lazily, on first request)
    fn load_credentials(&self) -> JwtAuthResult<Credentials> {
        // Check if already loaded
        {
            let guard = self
                .credentials
                .read()
                .map_err(|_| "JWT credential cache lock poisoned")?;
            if let Some(ref creds) = *guard {
                return Ok(creds.clone());
            }
        }

        // Load from Secret
        let creds = self.load_credentials_from_secret()?;

        // Cache
        {
            let mut guard = self
                .credentials
                .write()
                .map_err(|_| "JWT credential cache lock poisoned")?;
            *guard = Some(creds.clone());
        }

        Ok(creds)
    }

    /// Decode secret bytes if base64_secret is enabled
    fn decode_secret_if_needed(&self, secret_bytes: Vec<u8>) -> JwtAuthResult<Vec<u8>> {
        if self.config.base64_secret {
            // Secret is base64 encoded, decode it
            let secret_str =
                String::from_utf8(secret_bytes).map_err(|e| format!("Invalid UTF-8 in base64 secret: {}", e))?;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(secret_str.trim())
                .map_err(|e| format!("Failed to decode base64 secret: {}", e))?;
            Ok(decoded)
        } else {
            Ok(secret_bytes)
        }
    }

    /// Load credentials from pre-resolved data (populated by controller)
    /// Falls back to get_secret for backward compatibility
    fn load_credentials_from_secret(&self) -> JwtAuthResult<Credentials> {
        let is_symmetric = Self::is_symmetric(self.config.algorithm);

        // Try pre-resolved single credential first
        if let Some(ref resolved) = self.config.resolved_credential {
            let cred = if is_symmetric {
                let secret_b64 = resolved.secret.as_ref().ok_or("Resolved credential missing 'secret'")?;
                let secret_bytes = base64::engine::general_purpose::STANDARD
                    .decode(secret_b64)
                    .map_err(|e| format!("Invalid base64 secret: {}", e))?;
                // Apply additional base64 decoding if configured
                let final_secret = self.decode_secret_if_needed(secret_bytes)?;
                Credential::Symmetric(final_secret)
            } else {
                let pem = resolved
                    .public_key
                    .as_ref()
                    .ok_or("Resolved credential missing 'publicKey'")?;
                Credential::Asymmetric(pem.clone())
            };
            return Ok(Credentials::Single(cred));
        }

        // Try pre-resolved multiple credentials
        if let Some(ref resolved_map) = self.config.resolved_credentials {
            let mut map = HashMap::new();
            for (key, resolved) in resolved_map {
                let cred = if is_symmetric {
                    let secret_b64 = resolved
                        .secret
                        .as_ref()
                        .ok_or_else(|| format!("Resolved credential '{}' missing 'secret'", key))?;
                    let secret_bytes = base64::engine::general_purpose::STANDARD
                        .decode(secret_b64)
                        .map_err(|e| format!("Invalid base64 secret for '{}': {}", key, e))?;
                    // Apply additional base64 decoding if configured
                    let final_secret = self.decode_secret_if_needed(secret_bytes)?;
                    Credential::Symmetric(final_secret)
                } else {
                    let pem = resolved
                        .public_key
                        .as_ref()
                        .ok_or_else(|| format!("Resolved credential '{}' missing 'publicKey'", key))?;
                    Credential::Asymmetric(pem.clone())
                };
                map.insert(key.clone(), cred);
            }
            if !map.is_empty() {
                return Ok(Credentials::Multi(map));
            }
        }

        // Fallback to get_secret (for controller-side or backward compatibility)
        // Single secret_ref
        if let Some(ref secret_ref) = self.config.secret_ref {
            let ns = secret_ref.namespace.as_deref().unwrap_or(&self.plugin_namespace);
            let secret = get_secret(Some(ns), &secret_ref.name)
                .ok_or_else(|| format!("Secret {}/{} not found", ns, secret_ref.name))?;

            let data = secret
                .data
                .as_ref()
                .ok_or_else(|| format!("Secret {}/{} has no data", ns, secret_ref.name))?;

            let cred = if is_symmetric {
                let secret_bytes = data
                    .get("secret")
                    .ok_or_else(|| format!("Secret {}/{} missing 'secret' key", ns, secret_ref.name))?;
                // Apply additional base64 decoding if configured
                let final_secret = self.decode_secret_if_needed(secret_bytes.0.clone())?;
                Credential::Symmetric(final_secret)
            } else {
                let public_key_bytes = data
                    .get("publicKey")
                    .ok_or_else(|| format!("Secret {}/{} missing 'publicKey' key", ns, secret_ref.name))?;
                let pem = String::from_utf8(public_key_bytes.0.clone())
                    .map_err(|e| format!("Invalid publicKey encoding: {}", e))?;
                Credential::Asymmetric(pem)
            };

            return Ok(Credentials::Single(cred));
        }

        // Multiple secret_refs
        if let Some(ref secret_refs) = self.config.secret_refs {
            let mut map = HashMap::new();

            for secret_ref in secret_refs {
                let ns = secret_ref.namespace.as_deref().unwrap_or(&self.plugin_namespace);
                let secret = get_secret(Some(ns), &secret_ref.name)
                    .ok_or_else(|| format!("Secret {}/{} not found", ns, secret_ref.name))?;

                let data = secret
                    .data
                    .as_ref()
                    .ok_or_else(|| format!("Secret {}/{} has no data", ns, secret_ref.name))?;

                let key_bytes = data
                    .get("key")
                    .ok_or_else(|| format!("Secret {}/{} missing 'key' field", ns, secret_ref.name))?;
                let key_str =
                    String::from_utf8(key_bytes.0.clone()).map_err(|e| format!("Invalid key encoding: {}", e))?;

                let cred = if is_symmetric {
                    let secret_bytes = data
                        .get("secret")
                        .ok_or_else(|| format!("Secret {}/{} missing 'secret' key", ns, secret_ref.name))?;
                    // Apply additional base64 decoding if configured
                    let final_secret = self.decode_secret_if_needed(secret_bytes.0.clone())?;
                    Credential::Symmetric(final_secret)
                } else {
                    let public_key_bytes = data
                        .get("publicKey")
                        .ok_or_else(|| format!("Secret {}/{} missing 'publicKey' key", ns, secret_ref.name))?;
                    let pem = String::from_utf8(public_key_bytes.0.clone())
                        .map_err(|e| format!("Invalid publicKey encoding: {}", e))?;
                    Credential::Asymmetric(pem)
                };

                map.insert(key_str, cred);
            }

            return Ok(Credentials::Multi(map));
        }

        // No credentials configured
        Ok(Credentials::None)
    }

    /// Build DecodingKey from credential
    fn build_decoding_key(&self, cred: &Credential) -> JwtAuthResult<DecodingKey> {
        match cred {
            Credential::Symmetric(secret) => Ok(DecodingKey::from_secret(secret)),
            Credential::Asymmetric(pem) => {
                // Try RSA first, then EC
                if let Ok(key) = DecodingKey::from_rsa_pem(pem.as_bytes()) {
                    return Ok(key);
                }
                if let Ok(key) = DecodingKey::from_ec_pem(pem.as_bytes()) {
                    return Ok(key);
                }
                Err("Invalid public key PEM (not RSA or EC)".into())
            }
        }
    }

    /// Verify token and return username
    fn verify_token(&self, token: &str) -> JwtAuthResult<VerifyResult> {
        let credentials = self.load_credentials()?;
        let expected_alg = Self::to_jwt_algorithm(self.config.algorithm);

        // Always verify algorithm from JWT header matches configured algorithm
        // This prevents algorithm confusion attacks (e.g., "alg": "none")
        let header = decode_header(token).map_err(|e| format!("Invalid JWT header: {}", e))?;
        if header.alg != expected_alg {
            return Err(format!(
                "Algorithm mismatch: expected {:?}, got {:?}",
                self.config.algorithm, header.alg
            )
            .into());
        }

        // Build validation
        let mut validation = Validation::new(expected_alg);
        validation.leeway = self.config.lifetime_grace_period;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.set_required_spec_claims::<&str>(&[]);

        // Configure issuer validation if specified
        if let Some(ref issuers) = self.config.issuers {
            if !issuers.is_empty() {
                validation.set_issuer(issuers);
            }
        }

        // Configure audience validation if specified
        if let Some(ref audiences) = self.config.audiences {
            if !audiences.is_empty() {
                validation.set_audience(audiences);
            }
        } else {
            validation.validate_aud = false;
        }

        let decoding_key = match &credentials {
            Credentials::Single(cred) => self.build_decoding_key(cred)?,
            Credentials::Multi(map) => {
                // For multi-key, we need to peek at the payload to get key_claim_name value
                // Decode without verification to get payload
                let mut no_verify = Validation::new(expected_alg);
                no_verify.insecure_disable_signature_validation();
                no_verify.validate_exp = false;
                no_verify.validate_nbf = false;
                no_verify.validate_aud = false;
                no_verify.set_required_spec_claims::<&str>(&[]);

                let token_data = decode::<Claims>(token, &DecodingKey::from_secret(&[]), &no_verify)
                    .map_err(|e| format!("Failed to decode JWT payload: {}", e))?;
                let claims_value = token_data.claims.to_value();

                let key_value = claims_value
                    .get(&self.config.key_claim_name)
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        format!(
                            "JWT payload missing '{}' claim for key lookup",
                            self.config.key_claim_name
                        )
                    })?;

                let cred = map
                    .get(key_value)
                    .ok_or_else(|| format!("Unknown key '{}' in JWT", key_value))?;

                self.build_decoding_key(cred)?
            }
            Credentials::None => {
                return Err("No credentials configured (secret_ref or secret_refs required)".into());
            }
        };

        // Verify signature and claims
        let token_data = decode::<Claims>(token, &decoding_key, &validation)
            .map_err(|e| format!("JWT verification failed: {}", e))?;
        let claims_value = token_data.claims.to_value();

        // Validate maximum_expiration if configured
        if self.config.maximum_expiration > 0 {
            if let Some(exp) = claims_value.get("exp").and_then(|v| v.as_u64()) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let max_allowed_exp = now + self.config.maximum_expiration;
                if exp > max_allowed_exp {
                    return Err(format!(
                        "Token expiration too far in future: {} > {} (max {}s)",
                        exp, max_allowed_exp, self.config.maximum_expiration
                    )
                    .into());
                }
            }
        }

        // Extract username: try key_claim_name first, then "sub" claim, then empty string
        let username = claims_value
            .get(&self.config.key_claim_name)
            .and_then(|v| v.as_str())
            .or_else(|| claims_value.get("sub").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();

        // Serialize claims to JSON if store_claims_in_ctx is enabled
        let claims_json = if self.config.store_claims_in_ctx {
            serde_json::to_string(&claims_value).ok()
        } else {
            None
        };

        Ok(VerifyResult {
            username,
            claims_json,
            claims: claims_value,
        })
    }

    /// Set headers from JWT claims based on claims_to_headers configuration
    fn set_claims_headers(&self, session: &mut dyn PluginSession, claims: &serde_json::Value) {
        if let Some(ref mapping) = self.config.claims_to_headers {
            // Keep existing jwt_auth behavior by not enforcing header size limits here.
            set_common_claims_headers(session, claims, mapping, usize::MAX, usize::MAX);
        }
    }

    /// Handle anonymous access
    /// Sets X-Anonymous-Consumer and X-Consumer-Username headers.
    fn handle_anonymous_access(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> bool {
        if let Some(ref anonymous) = self.config.anonymous {
            plugin_log.push(&format!("Anon={}; ", anonymous));
            let _ = session.set_request_header("X-Anonymous-Consumer", "true");
            let _ = session.set_request_header("X-Consumer-Username", anonymous);
            return true;
        }
        false
    }

    /// Return 401 Unauthorized response with WWW-Authenticate header
    async fn auth_failed_return(&self, session: &mut dyn PluginSession) -> JwtAuthResult<()> {
        send_auth_error_response(
            session,
            401,
            "Bearer",
            &self.config.realm,
            "Unauthorized - Invalid or missing JWT",
        )
        .await
    }

    /// Delay before returning auth failure (timing attack protection)
    async fn apply_auth_failure_delay(&self) {
        if self.config.auth_failure_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.config.auth_failure_delay_ms)).await;
        }
    }

    /// Hide credentials based on token source
    ///
    /// Note: Only header-based tokens can be fully hidden.
    /// Query parameter and cookie tokens cannot be removed from the upstream request
    /// due to HTTP protocol limitations (would require URI/Cookie header rewriting).
    fn hide_credentials_if_needed(&self, session: &mut dyn PluginSession, source: TokenSource) {
        if !self.config.hide_credentials {
            return;
        }
        match source {
            TokenSource::Header => {
                let _ = session.remove_request_header(&self.config.header);
            }
            TokenSource::Query => {
                // Query parameter removal requires URI rewriting which is complex
                // Log a warning so users know this limitation
                tracing::warn!(
                    "hide_credentials: JWT in query parameter '{}' cannot be hidden (use header instead)",
                    self.config.query
                );
            }
            TokenSource::Cookie => {
                // Cookie removal requires Cookie header rewriting which is complex
                // Log a warning so users know this limitation
                tracing::warn!(
                    "hide_credentials: JWT in cookie '{}' cannot be hidden (use header instead)",
                    self.config.cookie
                );
            }
        }
    }
}

#[async_trait]
impl RequestFilter for JwtAuth {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Extract token
        let (token, source) = match self.extract_token(session) {
            Some(t) => t,
            None => {
                plugin_log.push("No token; ");
                // Check anonymous
                if self.handle_anonymous_access(session, plugin_log) {
                    return PluginRunningResult::GoodNext;
                }
                // Apply failure delay before returning 401
                self.apply_auth_failure_delay().await;
                let _ = self.auth_failed_return(session).await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        // Verify token
        match self.verify_token(&token) {
            Ok(result) => {
                plugin_log.push(&format!("OK u={}; ", result.username));
                // Set claims to headers if configured
                self.set_claims_headers(session, &result.claims);
                // Store claims in context if configured
                if let Some(ref claims_json) = result.claims_json {
                    let _ = session.set_ctx_var("jwt_claims", claims_json);
                }
                self.hide_credentials_if_needed(session, source);
                PluginRunningResult::GoodNext
            }
            Err(e) => {
                // Log detailed error to system log, not access log (avoid leaking sensitive info)
                tracing::debug!("JWT auth failed: {}", e);
                plugin_log.push("FAIL; ");
                // Check anonymous
                if self.handle_anonymous_access(session, plugin_log) {
                    self.hide_credentials_if_needed(session, source);
                    return PluginRunningResult::GoodNext;
                }
                // Apply failure delay before returning 401
                self.apply_auth_failure_delay().await;
                let _ = self.auth_failed_return(session).await;
                PluginRunningResult::ErrTerminateRequest
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;
    use crate::types::plugin_configs::ResolvedJwtCredential;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;
    use std::collections::HashMap as StdHashMap;
    use std::sync::{Arc, Mutex};

    const TEST_SECRET: &str = "test-secret-key-for-jwt-auth-testing-min-32-bytes";

    /// Helper: create JwtAuth with default config
    fn jwt_auth_default(anonymous: Option<String>) -> JwtAuth {
        let config = JwtAuthConfig {
            anonymous,
            ..Default::default()
        };
        JwtAuth::new(&config, "default".to_string())
    }

    /// Helper: create JwtAuth with resolved credential
    /// Note: secret is automatically base64-encoded as expected by the runtime
    fn jwt_auth_with_credential(
        secret: &str,
        algorithm: JwtAlgorithm,
        config_fn: Option<fn(&mut JwtAuthConfig)>,
    ) -> JwtAuth {
        use base64::Engine;
        let mut config = JwtAuthConfig {
            algorithm,
            ..Default::default()
        };
        // Controller base64-encodes the secret, so we do the same in tests
        let secret_b64 = base64::engine::general_purpose::STANDARD.encode(secret.as_bytes());
        config.resolved_credential = Some(ResolvedJwtCredential {
            key: None,
            secret: Some(secret_b64),
            public_key: None,
        });
        if let Some(f) = config_fn {
            f(&mut config);
        }
        JwtAuth::new(&config, "default".to_string())
    }

    /// Helper: generate HS256 JWT token
    fn generate_hs256_token(claims: serde_json::Value, secret: &str) -> String {
        let header = Header::new(Algorithm::HS256);
        encode(&header, &claims, &EncodingKey::from_secret(secret.as_bytes())).unwrap()
    }

    /// Helper: generate JWT with specific algorithm
    fn generate_token_with_alg(claims: serde_json::Value, secret: &str, alg: Algorithm) -> String {
        let header = Header::new(alg);
        encode(&header, &claims, &EncodingKey::from_secret(secret.as_bytes())).unwrap()
    }

    /// Helper: get current timestamp
    fn now_ts() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
    }

    // ========== Basic Tests ==========

    #[tokio::test]
    async fn test_no_token_returns_401() {
        let auth = jwt_auth_default(None);
        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        mock.expect_header_value().returning(|_| None);
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_write_response_header().returning(|_, _| Ok(()));
        mock.expect_write_response_body().returning(|_, _| Ok(()));
        mock.expect_shutdown().returning(|| {});

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("No token"));
    }

    #[tokio::test]
    async fn test_no_token_with_anonymous() {
        let auth = jwt_auth_default(Some("anon-user".to_string()));
        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        mock.expect_header_value().returning(|_| None);
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_set_request_header().returning(|_, _| Ok(()));

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("Anon=anon-user"));
    }

    #[tokio::test]
    async fn test_invalid_token_returns_401() {
        let auth = jwt_auth_default(None);
        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        mock.expect_header_value()
            .returning(|_| Some("Bearer invalid.token.here".to_string()));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_write_response_header().returning(|_, _| Ok(()));
        mock.expect_write_response_body().returning(|_, _| Ok(()));
        mock.expect_shutdown().returning(|| {});

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("FAIL"));
    }

    // ========== P0: Valid Token Verification ==========

    #[tokio::test]
    async fn test_valid_hs256_token_succeeds() {
        let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);
        let claims = json!({
            "sub": "user123",
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_set_request_header().returning(|_, _| Ok(()));

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("OK u=user123"));
    }

    // ========== P0: Algorithm Mismatch Rejection ==========

    #[tokio::test]
    async fn test_algorithm_mismatch_rejected() {
        // Config expects HS256, but token is HS384
        let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);
        let claims = json!({
            "sub": "user123",
            "exp": now_ts() + 3600
        });
        // Generate token with HS384
        let token = generate_token_with_alg(claims, TEST_SECRET, Algorithm::HS384);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_write_response_header().returning(|_, _| Ok(()));
        mock.expect_write_response_body().returning(|_, _| Ok(()));
        mock.expect_shutdown().returning(|| {});

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("FAIL"));
    }

    // ========== P0: WWW-Authenticate Header ==========

    #[tokio::test]
    async fn test_401_includes_www_authenticate_header() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.realm = "my-api".to_string();
            }),
        );

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        mock.expect_header_value().returning(|_| None);
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);

        // Capture response headers
        let headers_captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let headers_clone = headers_captured.clone();
        mock.expect_write_response_header().returning(move |resp, _| {
            // Check for WWW-Authenticate header in response
            if let Some(val) = resp.headers.get("WWW-Authenticate") {
                headers_clone
                    .lock()
                    .unwrap()
                    .push(("WWW-Authenticate".to_string(), val.to_str().unwrap_or("").to_string()));
            }
            Ok(())
        });
        mock.expect_write_response_body().returning(|_, _| Ok(()));
        mock.expect_shutdown().returning(|| {});

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);

        let headers = headers_captured.lock().unwrap();
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "WWW-Authenticate" && v.contains("Bearer") && v.contains("my-api")),
            "Expected WWW-Authenticate header with realm, got: {:?}",
            *headers
        );
    }

    // ========== P0: Base64 Secret ==========

    #[tokio::test]
    async fn test_base64_secret_decoding() {
        // Create base64-encoded secret
        let raw_secret = "my-raw-secret-key-for-testing!!";
        let base64_secret = base64::engine::general_purpose::STANDARD.encode(raw_secret);

        let auth = jwt_auth_with_credential(
            &base64_secret,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.base64_secret = true;
            }),
        );

        // Generate token with raw secret
        let claims = json!({
            "sub": "user123",
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, raw_secret);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_set_request_header().returning(|_, _| Ok(()));

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("OK u=user123"));
    }

    // ========== P1: Store Claims in Context ==========

    #[tokio::test]
    async fn test_store_claims_in_ctx() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.store_claims_in_ctx = true;
            }),
        );

        let claims = json!({
            "sub": "user123",
            "email": "user@example.com",
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_set_request_header().returning(|_, _| Ok(()));

        // Capture ctx var
        let ctx_var_captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let ctx_clone = ctx_var_captured.clone();
        mock.expect_set_ctx_var().returning(move |key, value| {
            if key == "jwt_claims" {
                *ctx_clone.lock().unwrap() = Some(value.to_string());
            }
            Ok(())
        });

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);

        let ctx_val = ctx_var_captured.lock().unwrap();
        assert!(ctx_val.is_some(), "jwt_claims should be set in context");
        let claims_json = ctx_val.as_ref().unwrap();
        assert!(claims_json.contains("user123"));
        assert!(claims_json.contains("user@example.com"));
    }

    // ========== P2: Username Extraction Fallback ==========

    #[tokio::test]
    async fn test_username_from_key_claim_name() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.key_claim_name = "preferred_username".to_string();
            }),
        );

        let claims = json!({
            "sub": "sub-value",
            "preferred_username": "custom-user",
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_set_request_header().returning(|_, _| Ok(()));

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        // Should use preferred_username, not sub
        assert!(log.contains("OK u=custom-user"));
    }

    #[tokio::test]
    async fn test_username_fallback_to_sub() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.key_claim_name = "nonexistent_claim".to_string();
            }),
        );

        let claims = json!({
            "sub": "fallback-user",
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_set_request_header().returning(|_, _| Ok(()));

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        // Should fallback to sub
        assert!(log.contains("OK u=fallback-user"));
    }

    // ========== P2: Issuer Validation ==========

    #[tokio::test]
    async fn test_issuer_validation_pass() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.issuers = Some(vec!["https://auth.example.com".to_string()]);
            }),
        );

        let claims = json!({
            "sub": "user123",
            "iss": "https://auth.example.com",
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_set_request_header().returning(|_, _| Ok(()));

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_issuer_validation_fail() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.issuers = Some(vec!["https://auth.example.com".to_string()]);
            }),
        );

        let claims = json!({
            "sub": "user123",
            "iss": "https://wrong-issuer.com",
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_write_response_header().returning(|_, _| Ok(()));
        mock.expect_write_response_body().returning(|_, _| Ok(()));
        mock.expect_shutdown().returning(|| {});

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("FAIL"));
    }

    // ========== P2: Audience Validation ==========

    #[tokio::test]
    async fn test_audience_validation_pass() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.audiences = Some(vec!["my-api".to_string()]);
            }),
        );

        let claims = json!({
            "sub": "user123",
            "aud": "my-api",
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_set_request_header().returning(|_, _| Ok(()));

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_audience_validation_fail() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.audiences = Some(vec!["my-api".to_string()]);
            }),
        );

        let claims = json!({
            "sub": "user123",
            "aud": "wrong-api",
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_write_response_header().returning(|_, _| Ok(()));
        mock.expect_write_response_body().returning(|_, _| Ok(()));
        mock.expect_shutdown().returning(|| {});

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("FAIL"));
    }

    // ========== P2: Maximum Expiration ==========

    #[tokio::test]
    async fn test_maximum_expiration_pass() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.maximum_expiration = 7200; // 2 hours max
            }),
        );

        let claims = json!({
            "sub": "user123",
            "exp": now_ts() + 3600  // 1 hour, within limit
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_set_request_header().returning(|_, _| Ok(()));

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_maximum_expiration_exceeded() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.maximum_expiration = 1800; // 30 minutes max
            }),
        );

        let claims = json!({
            "sub": "user123",
            "exp": now_ts() + 7200  // 2 hours, exceeds limit
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_write_response_header().returning(|_, _| Ok(()));
        mock.expect_write_response_body().returning(|_, _| Ok(()));
        mock.expect_shutdown().returning(|| {});

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("FAIL"));
    }

    // ========== P2: Auth Failure Delay ==========

    #[tokio::test]
    async fn test_auth_failure_delay() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.auth_failure_delay_ms = 100; // 100ms delay
            }),
        );

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        mock.expect_header_value().returning(|_| None);
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_write_response_header().returning(|_, _| Ok(()));
        mock.expect_write_response_body().returning(|_, _| Ok(()));
        mock.expect_shutdown().returning(|| {});

        let start = std::time::Instant::now();
        let r = auth.run_request(&mut mock, &mut log).await;
        let elapsed = start.elapsed();

        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        // Should have at least ~100ms delay
        assert!(elapsed.as_millis() >= 90, "Expected delay of ~100ms, got {:?}", elapsed);
    }

    // ========== New: Claims to Headers ==========

    #[tokio::test]
    async fn test_claims_to_headers() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                let mut mapping = StdHashMap::new();
                mapping.insert("sub".to_string(), "X-User-ID".to_string());
                mapping.insert("email".to_string(), "X-User-Email".to_string());
                mapping.insert("roles".to_string(), "X-User-Roles".to_string());
                c.claims_to_headers = Some(mapping);
            }),
        );

        let claims = json!({
            "sub": "user123",
            "email": "user@example.com",
            "roles": ["admin", "user"],
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        let token_clone = token.clone();
        mock.expect_header_value()
            .returning(move |_| Some(format!("Bearer {}", token_clone)));
        mock.expect_get_query_param().returning(|_| None);
        mock.expect_get_cookie().returning(|_| None);

        // Capture set headers
        let headers_set: Arc<Mutex<StdHashMap<String, String>>> = Arc::new(Mutex::new(StdHashMap::new()));
        let headers_clone = headers_set.clone();
        mock.expect_set_request_header().returning(move |name, value| {
            headers_clone
                .lock()
                .unwrap()
                .insert(name.to_string(), value.to_string());
            Ok(())
        });

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);

        let headers = headers_set.lock().unwrap();
        assert_eq!(headers.get("X-User-ID"), Some(&"user123".to_string()));
        assert_eq!(headers.get("X-User-Email"), Some(&"user@example.com".to_string()));
        assert_eq!(headers.get("X-User-Roles"), Some(&"admin,user".to_string()));
    }

    // ========== Token Extraction Tests ==========

    #[tokio::test]
    async fn test_token_from_query_param() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.query = "token".to_string();
            }),
        );

        let claims = json!({
            "sub": "query-user",
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        mock.expect_header_value().returning(|_| None);
        let token_clone = token.clone();
        mock.expect_get_query_param().returning(move |name| {
            if name == "token" {
                Some(token_clone.clone())
            } else {
                None
            }
        });
        mock.expect_get_cookie().returning(|_| None);
        mock.expect_set_request_header().returning(|_, _| Ok(()));

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("OK u=query-user"));
    }

    #[tokio::test]
    async fn test_token_from_cookie() {
        let auth = jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|c| {
                c.cookie = "jwt_cookie".to_string();
            }),
        );

        let claims = json!({
            "sub": "cookie-user",
            "exp": now_ts() + 3600
        });
        let token = generate_hs256_token(claims, TEST_SECRET);

        let mut mock = MockPluginSession::new();
        let mut log = PluginLog::new("JwtAuth");

        mock.expect_method().returning(|| "GET".to_string());
        mock.expect_header_value().returning(|_| None);
        mock.expect_get_query_param().returning(|_| None);
        let token_clone = token.clone();
        mock.expect_get_cookie().returning(move |name| {
            if name == "jwt_cookie" {
                Some(token_clone.clone())
            } else {
                None
            }
        });
        mock.expect_set_request_header().returning(|_, _| Ok(()));

        let r = auth.run_request(&mut mock, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("OK u=cookie-user"));
    }
}
