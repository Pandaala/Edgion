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
    pub iss: String,
    pub claims: Value,
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
    let header = match decode_header(token) {
        Ok(h) => h,
        Err(_) => {
            plugin_log.push("Deny malformed token");
            return Err(VerifyDeny::Malformed);
        }
    };

    // 2. Peek at payload (insecure decode) to read iss.
    let peek = match decode::<Value>(token, store.peek_decoding_key(), store.peek_validation()) {
        Ok(d) => d.claims,
        Err(_) => {
            plugin_log.push("Deny malformed token");
            return Err(VerifyDeny::Malformed);
        }
    };

    // 3. Extract iss (REQUIRED, must be non-empty string).
    let iss = match peek.get("iss").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s,
        _ => {
            plugin_log.push("Deny missing iss");
            return Err(VerifyDeny::MissingIss);
        }
    };

    // 4. Route to group (exact iss match; no normalization).
    let cached_group = match store.lookup_by_iss(iss) {
        Some(g) => g,
        None => {
            plugin_log.push("Deny unknown iss"); // NO iss value in log
            return Err(VerifyDeny::UnknownIss);
        }
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

    // 7. Real verification -- group.algorithm + this cred's secret/public_key.
    let decoded = match decode::<Value>(token, decoding_key, cached_group.validation()) {
        Ok(d) => d,
        Err(e) => {
            plugin_log.push(classify_jwt_decode_error(&e));
            return Err(VerifyDeny::SignatureFail);
        }
    };
    let verified_iss = match decoded.claims.get("iss").and_then(Value::as_str) {
        Some(iss) if !iss.is_empty() => iss.to_string(),
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
        iss: verified_iss,
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
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_plugins::plugin_configs::{
        JwtAlgorithm, JwtAuthConfig, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
    };
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use serde_json::json;
    use std::collections::HashMap;

    fn make_hs_group(name: &str, kid: &str, secret_bytes: &[u8]) -> ResolvedJwtIssuerGroup {
        let mut creds = HashMap::new();
        creds.insert(
            kid.to_string(),
            ResolvedJwtCredential {
                key: Some(kid.to_string()),
                secret: Some(std::str::from_utf8(secret_bytes).unwrap().to_string()),
                public_key: None,
            },
        );
        ResolvedJwtIssuerGroup {
            group: name.to_string(),
            algorithm: JwtAlgorithm::HS256,
            audiences: None,
            credentials: creds,
        }
    }

    fn config_default_keyclaim() -> JwtAuthConfig {
        JwtAuthConfig::default()
    }

    fn make_token(alg: Algorithm, kid: &str, iss: &str, secret_bytes: &[u8]) -> String {
        let claims = json!({
            "iss": iss,
            "key": kid,
            "sub": "alice",
            "exp": (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3600) as u64,
        });
        let header = Header::new(alg);
        encode(&header, &claims, &EncodingKey::from_secret(secret_bytes)).unwrap()
    }

    #[test]
    fn verify_hits_correct_group_by_iss() {
        let secret = b"hs256-secret-at-least-32-bytes-long-padding";
        let group_a = make_hs_group("partner-a", "k1", secret);
        let mut by_iss = HashMap::new();
        by_iss.insert("https://a.example".to_string(), group_a);
        let store = JwtIssuerGroupStore::new(by_iss, false, 0);
        let config = config_default_keyclaim();
        let mut log = PluginLog::new("JwtAuth");

        let token = make_token(Algorithm::HS256, "k1", "https://a.example", secret);
        let ok = verify_jwt(&token, &store, &config, &mut log).unwrap();
        assert_eq!(ok.iss, "https://a.example");
        assert_eq!(ok.group.group, "partner-a");
    }

    #[test]
    fn verify_rejects_unknown_iss() {
        let secret = b"hs256-secret-at-least-32-bytes-long-padding";
        let group_a = make_hs_group("partner-a", "k1", secret);
        let mut by_iss = HashMap::new();
        by_iss.insert("https://a.example".to_string(), group_a);
        let store = JwtIssuerGroupStore::new(by_iss, false, 0);
        let config = config_default_keyclaim();
        let mut log = PluginLog::new("JwtAuth");

        let token = make_token(Algorithm::HS256, "k1", "https://unknown.example", secret);
        let err = verify_jwt(&token, &store, &config, &mut log).unwrap_err();
        assert!(matches!(err, VerifyDeny::UnknownIss));
    }

    #[test]
    fn verify_rejects_iss_trailing_slash_mismatch() {
        // Token iss has trailing slash; group iss does not. iss matching is exact, no normalization.
        let secret = b"hs256-secret-at-least-32-bytes-long-padding";
        let group_a = make_hs_group("partner-a", "k1", secret);
        let mut by_iss = HashMap::new();
        by_iss.insert("https://a.example".to_string(), group_a); // no slash
        let store = JwtIssuerGroupStore::new(by_iss, false, 0);
        let config = config_default_keyclaim();
        let mut log = PluginLog::new("JwtAuth");

        let token = make_token(Algorithm::HS256, "k1", "https://a.example/", secret); // with slash
        let err = verify_jwt(&token, &store, &config, &mut log).unwrap_err();
        assert!(
            matches!(err, VerifyDeny::UnknownIss),
            "got: {:?}",
            std::mem::discriminant(&err)
        );
    }

    #[test]
    fn verify_rejects_algorithm_confusion_attack() {
        // partner-a is HS256. Token claims iss=partner-a but signed with HS384.
        // header.alg=HS384 vs group.algorithm=HS256 -> Deny bad algorithm.
        let secret = b"hs256-secret-at-least-32-bytes-long-padding";
        let group_a = make_hs_group("partner-a", "k1", secret);
        let mut by_iss = HashMap::new();
        by_iss.insert("https://a.example".to_string(), group_a);
        let store = JwtIssuerGroupStore::new(by_iss, false, 0);
        let config = config_default_keyclaim();
        let mut log = PluginLog::new("JwtAuth");

        let token = make_token(Algorithm::HS384, "k1", "https://a.example", secret);
        let err = verify_jwt(&token, &store, &config, &mut log).unwrap_err();
        assert!(matches!(err, VerifyDeny::BadAlgorithm));
    }

    #[test]
    fn verify_rejects_kid_from_other_group() {
        // partner-a has kid=ka; partner-b has kid=kb.
        // Token claims iss=partner-b but kid=ka (from partner-a).
        // Routing -> partner-b group -> ka not in partner-b.credentials -> Deny missing key.
        let secret_a = b"hs256-secret-a-padding-padding-padding-padding";
        let secret_b = b"hs256-secret-b-padding-padding-padding-padding";
        let group_a = make_hs_group("partner-a", "ka", secret_a);
        let group_b = make_hs_group("partner-b", "kb", secret_b);
        let mut by_iss = HashMap::new();
        by_iss.insert("https://a.example".to_string(), group_a);
        by_iss.insert("https://b.example".to_string(), group_b);
        let store = JwtIssuerGroupStore::new(by_iss, false, 0);
        let config = config_default_keyclaim();
        let mut log = PluginLog::new("JwtAuth");

        // Sign with secret_a (would verify only if routed to group_a), but
        // claim iss=https://b.example. Routing -> group_b -> kid `ka` not in
        // group_b's credentials -> Deny missing key.
        let token = make_token(Algorithm::HS256, "ka", "https://b.example", secret_a);
        let err = verify_jwt(&token, &store, &config, &mut log).unwrap_err();
        assert!(matches!(err, VerifyDeny::MissingKey));
    }

    #[test]
    fn verify_reuses_cached_malformed_key_error_without_logging_key_context() {
        let group = ResolvedJwtIssuerGroup {
            group: "sensitive-group".to_string(),
            algorithm: JwtAlgorithm::HS256,
            audiences: None,
            credentials: HashMap::from([(
                "sensitive-kid".to_string(),
                ResolvedJwtCredential {
                    key: Some("sensitive-kid".to_string()),
                    secret: Some("not-valid-base64!".to_string()),
                    public_key: None,
                },
            )]),
        };
        let store = JwtIssuerGroupStore::new(
            HashMap::from([("https://sensitive.example".to_string(), group)]),
            true,
            0,
        );
        let config = config_default_keyclaim();
        let token = make_token(
            Algorithm::HS256,
            "sensitive-kid",
            "https://sensitive.example",
            b"unused-signing-secret-at-least-32-bytes",
        );

        for _ in 0..2 {
            let mut log = PluginLog::new("JwtAuth");
            let error = verify_jwt(&token, &store, &config, &mut log).unwrap_err();
            assert!(matches!(error, VerifyDeny::InvalidCredentials));
            assert!(log.contains("Deny invalid credentials"));
            let serialized = serde_json::to_string(&log).unwrap();
            for forbidden in ["sensitive-group", "sensitive-kid", "https://sensitive.example"] {
                assert!(!serialized.contains(forbidden), "failure log leaked `{forbidden}`");
            }
        }
    }

    #[test]
    fn numeric_date_matches_jsonwebtoken_rounding_boundary() {
        assert_eq!(numeric_date(&json!(42_u64)), Some(42));
        assert_eq!(numeric_date(&json!(42.49)), Some(42));
        assert_eq!(numeric_date(&json!(42.5)), Some(43));
        assert_eq!(numeric_date(&json!(42.51)), Some(43));
        assert_eq!(numeric_date(&json!(-0.1)), None);

        let max_as_float = serde_json::Number::from_f64(u64::MAX as f64).unwrap();
        assert_eq!(numeric_date(&Value::Number(max_as_float)), None);
    }

    #[test]
    fn verify_ok_debug_is_fully_opaque() {
        let secret = b"sensitive-key-material-at-least-32-bytes";
        let store = JwtIssuerGroupStore::new(
            HashMap::from([(
                "sensitive-issuer".to_string(),
                make_hs_group("sensitive-group", "sensitive-kid", secret),
            )]),
            false,
            0,
        );
        let ok = VerifyOk {
            group: store.lookup_by_iss("sensitive-issuer").unwrap(),
            iss: "sensitive-issuer".to_string(),
            claims: json!({
                "sub": "sensitive-claim",
                "token": "sensitive-token"
            }),
        };

        let debug = format!("{ok:?}");
        assert_eq!(debug, "VerifyOk { .. }");
        for sentinel in [
            "sensitive-issuer",
            "sensitive-group",
            "sensitive-kid",
            "sensitive-key-material",
            "sensitive-claim",
            "sensitive-token",
        ] {
            assert!(!debug.contains(sentinel), "Debug leaked `{sentinel}`: {debug}");
        }
    }
}
