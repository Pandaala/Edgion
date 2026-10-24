//! JWT verification: iss-routed, algorithm-confusion-defended.

use jsonwebtoken::{decode, decode_header};
use serde_json::Value;
use std::fmt;

use super::credentials::{CachedJwtIssuerGroup, JwtIssuerGroupStore};
use super::to_jwt_algorithm;
use crate::plugins::runtime::PluginLog;
use edgion_resources::resources::edgion_plugins::plugin_configs::JwtAuthConfig;

pub(super) struct VerifyOk<'a> {
    pub group: &'a CachedJwtIssuerGroup,
    claims: Value,
}

impl VerifyOk<'_> {
    pub(super) fn issuer(&self) -> &str {
        self.claims
            .get("iss")
            .and_then(Value::as_str)
            .expect("verify_jwt validates issuer before constructing immutable claims")
    }

    pub(super) fn claims(&self) -> &Value {
        &self.claims
    }
}

impl fmt::Debug for VerifyOk<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifyOk").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(super) enum VerifyDeny {
    Malformed,
    MissingIss,
    UnknownIss,
    BadAlgorithm,
    MissingKey,
    InvalidCredentials,
    SignatureFail,
    ExpTooFar,
    InvalidNumericDate,
}

/// Sync verify: peek payload (insecure decode) -> route by iss -> enforce
/// header.alg == group.algorithm -> look up kid scoped to group -> real decode.
pub(super) fn verify_jwt<'a>(
    token: &str,
    store: &'a JwtIssuerGroupStore,
    config: &JwtAuthConfig,
    plugin_log: &mut PluginLog,
) -> Result<VerifyOk<'a>, VerifyDeny> {
    // 1. Decode header -- gives us the token's claimed alg.
    let Ok(header) = decode_header(token) else {
        plugin_log.push("Deny malformed token");
        return Err(VerifyDeny::Malformed);
    };

    // 2. Peek at payload (insecure decode) to read iss.
    let Ok(peek) = decode::<Value>(token, store.peek_decoding_key(), store.peek_validation()) else {
        plugin_log.push("Deny malformed token");
        return Err(VerifyDeny::Malformed);
    };
    drop(peek.header);
    let peek = peek.claims;

    // 3. Extract iss (REQUIRED, must be non-empty string).
    let iss = match peek.get("iss").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s,
        _ => {
            plugin_log.push("Deny missing iss");
            return Err(VerifyDeny::MissingIss);
        }
    };

    // 4. Route to group (exact iss match; no normalization).
    let Some(cached_group) = store.lookup_by_iss(iss) else {
        plugin_log.push("Deny unknown iss"); // NO iss value in log
        return Err(VerifyDeny::UnknownIss);
    };

    // 5. Algorithm-confusion defense -- header.alg MUST match group.algorithm.
    //    Without this, an attacker could submit alg=HS256 token claiming an
    //    RS-issuer's iss and force HMAC verification with the public key
    //    used as the symmetric secret (classic JWT alg-confusion attack).
    if header.alg != to_jwt_algorithm(cached_group.algorithm) {
        plugin_log.push("Deny bad algorithm"); // NO group/iss/alg value
        return Err(VerifyDeny::BadAlgorithm);
    }

    // 6. Resolve kid within THIS group only (key_claim_name is plugin-level).
    let kid = peek.get(&config.key_claim_name).and_then(|v| v.as_str());
    let decoding_key = match kid.and_then(|k| cached_group.lookup_decoding_key(k)) {
        Some(Ok(key)) => key,
        Some(Err(_reason)) => {
            plugin_log.push("Deny invalid credentials");
            return Err(VerifyDeny::InvalidCredentials);
        }
        None => {
            plugin_log.push("Deny missing key"); // NO group/iss/kid value
            return Err(VerifyDeny::MissingKey);
        }
    };

    drop(peek);
    drop(header);

    // 7. Real verification -- group.algorithm + this cred's secret/public_key.
    let decoded = match decode::<Value>(token, decoding_key, cached_group.validation()) {
        Ok(d) => d,
        Err(e) => {
            plugin_log.push(classify_jwt_decode_error(&e));
            return Err(VerifyDeny::SignatureFail);
        }
    };
    match decoded.claims.get("iss").and_then(Value::as_str) {
        Some(iss) if !iss.is_empty() => {}
        _ => {
            plugin_log.push("Deny missing iss");
            return Err(VerifyDeny::MissingIss);
        }
    };

    // 8. Reject tokens whose exp exceeds now + maximum_expiration.
    if config.maximum_expiration > 0 {
        let Some(exp) = decoded.claims.get("exp").and_then(numeric_date) else {
            plugin_log.push("Deny invalid token");
            return Err(VerifyDeny::InvalidNumericDate);
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if exp > now.saturating_add(config.maximum_expiration) {
            plugin_log.push("Deny exp too far");
            return Err(VerifyDeny::ExpTooFar);
        }
    }

    Ok(VerifyOk {
        group: cached_group,
        claims: decoded.claims,
    })
}

fn numeric_date(value: &Value) -> Option<u64> {
    let number = value.as_number()?;
    if let Some(value) = number.as_u64() {
        return Some(value);
    }
    let value = number.as_f64()?;
    (value.is_finite() && value >= 0.0 && value < u64::MAX as f64).then(|| value.round() as u64)
}

/// Map jsonwebtoken decode errors to PluginLog literal strings.
fn classify_jwt_decode_error(e: &jsonwebtoken::errors::Error) -> &'static str {
    use jsonwebtoken::errors::ErrorKind;
    match e.kind() {
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

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/jwt_auth/verify_test.rs"]
mod tests;
