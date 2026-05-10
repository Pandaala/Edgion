//! JWT verification main flow + decode error classification.

use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use std::time::{SystemTime, UNIX_EPOCH};

use super::credentials::Credentials;
use super::{is_symmetric, to_jwt_algorithm, JwtAuth, JwtAuthResult};
use crate::core::gateway::plugins::http::common::auth_common::Claims;
use crate::core::gateway::plugins::runtime::PluginLog;

/// Verification result containing username and claims.
pub(super) struct VerifyResult {
    pub(super) username: String,
    pub(super) claims_json: Option<String>,
    /// Raw claims for claims_to_headers mapping
    pub(super) claims: serde_json::Value,
}

/// Classify jsonwebtoken decode error into a short PluginLog tag.
/// Tag is a static kind label — no token or claim contents.
fn classify_jwt_decode_error(err: &jsonwebtoken::errors::Error) -> &'static str {
    match err.kind() {
        ErrorKind::ExpiredSignature => "Deny token expired",
        ErrorKind::InvalidSignature => "Deny invalid signature",
        ErrorKind::InvalidToken | ErrorKind::Base64(_) | ErrorKind::Json(_) | ErrorKind::Utf8(_) => {
            "Deny malformed token"
        }
        ErrorKind::InvalidIssuer => "Deny invalid issuer",
        ErrorKind::InvalidAudience => "Deny invalid audience",
        ErrorKind::ImmatureSignature => "Deny immature token",
        ErrorKind::InvalidAlgorithm | ErrorKind::InvalidAlgorithmName => "Deny bad algorithm",
        _ => "Deny invalid token",
    }
}

impl JwtAuth {
    /// Verify token and return username.
    /// On failure, pushes a `Deny <kind>; ` or `err: <kind>; ` tag to `plugin_log` before returning.
    pub(super) fn verify_token(&self, token: &str, plugin_log: &mut PluginLog) -> JwtAuthResult<VerifyResult> {
        let credentials = self.load_credentials().inspect_err(|_| {
            plugin_log.push_err("credentials load fail");
        })?;
        let expected_alg = to_jwt_algorithm(self.config.algorithm);
        // §3: algorithm selection
        if is_symmetric(self.config.algorithm) {
            plugin_log.push("alg symmetric");
        } else {
            plugin_log.push("alg asymmetric");
        }

        // Always verify algorithm from JWT header matches configured algorithm.
        // This prevents algorithm confusion attacks (e.g., "alg": "none").
        let header = decode_header(token).map_err(|_| {
            plugin_log.push("Deny malformed token");
        })?;
        if header.alg != expected_alg {
            plugin_log.push("Deny bad algorithm");
            return Err(());
        }

        // Build validation
        let mut validation = Validation::new(expected_alg);
        validation.leeway = self.config.lifetime_grace_period;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.set_required_spec_claims::<&str>(&[]);

        if let Some(ref issuers) = self.config.issuers {
            if !issuers.is_empty() {
                validation.set_issuer(issuers);
            }
        }

        if let Some(ref audiences) = self.config.audiences {
            if !audiences.is_empty() {
                validation.set_audience(audiences);
            }
        } else {
            validation.validate_aud = false;
        }

        let decoding_key = match &credentials {
            Credentials::Single(cred) => self.build_decoding_key(cred).inspect_err(|_| {
                plugin_log.push_err("bad credential key");
            })?,
            Credentials::Multi(map) => {
                // For multi-key, peek at the payload to get key_claim_name value.
                let mut no_verify = Validation::new(expected_alg);
                no_verify.insecure_disable_signature_validation();
                no_verify.validate_exp = false;
                no_verify.validate_nbf = false;
                no_verify.validate_aud = false;
                no_verify.set_required_spec_claims::<&str>(&[]);

                let token_data = decode::<Claims>(token, &DecodingKey::from_secret(&[]), &no_verify).map_err(|_| {
                    plugin_log.push("Deny malformed token");
                })?;
                let claims_value = token_data.claims.to_value();

                let key_value = claims_value
                    .get(&self.config.key_claim_name)
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        plugin_log.push("Deny missing claim");
                    })?;

                let cred = map.get(key_value).ok_or_else(|| {
                    // Use a generic failure tag so external observers cannot distinguish
                    // "key not found" from "key found but signature invalid" (key enumeration).
                    plugin_log.push("Deny invalid credentials");
                })?;
                // §3: key resolved from multi-key map
                plugin_log.push("multi key resolved");

                self.build_decoding_key(cred).inspect_err(|_| {
                    plugin_log.push_err("bad credential key");
                })?
            }
            Credentials::None => {
                plugin_log.push_err("no credentials configured");
                return Err(());
            }
        };

        // Verify signature and claims
        let token_data = decode::<Claims>(token, &decoding_key, &validation).map_err(|e| {
            plugin_log.push(classify_jwt_decode_error(&e));
        })?;
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
                    plugin_log.push("Deny exp too far");
                    return Err(());
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
}
