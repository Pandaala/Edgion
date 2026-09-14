use base64::{engine::general_purpose, Engine as _};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug)]
pub(super) struct TokenDecodeError;

/// CSRF Token structure
#[derive(Debug, Serialize, Deserialize)]
pub struct CsrfToken {
    /// Nonce for entropy (hex encoded 32 bytes)
    pub random: String,
    pub expires: i64,
    pub sign: String,
}

impl CsrfToken {
    /// Generate a new CSRF token with crypto-secure RNG
    pub fn generate(key: &str) -> Self {
        let mut random_bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut random_bytes);
        let random = hex::encode(random_bytes);

        // Generation retains the existing epoch-zero fallback for a pre-epoch
        // clock. Verification handles its own clock failure separately and
        // fails closed, so such a token is not accepted while the clock is bad.
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let sign = Self::gen_sign(&random, timestamp, key);

        CsrfToken {
            random,
            expires: timestamp,
            sign,
        }
    }

    fn gen_sign(random: &str, expires: i64, key: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(key.as_bytes())
            .expect("HMAC accepts any non-empty key per RFC 2104; key validated non-empty by config loader");
        let data = format!("expires:{},random:{}", expires, random);
        mac.update(data.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// Encode token to base64 string
    pub fn encode(&self) -> Result<String, String> {
        let json = serde_json::to_string(self).map_err(|e| format!("Failed to serialize token: {}", e))?;
        Ok(general_purpose::STANDARD.encode(json.as_bytes()))
    }

    /// Decode token from base64 string
    pub fn decode(token: &str) -> Result<Self, TokenDecodeError> {
        let bytes = general_purpose::STANDARD.decode(token).map_err(|_| TokenDecodeError)?;

        serde_json::from_slice(&bytes).map_err(|_| TokenDecodeError)
    }

    /// Verify token signature and expiration
    pub fn verify(&self, key: &str, max_age_seconds: i64) -> bool {
        let Ok(elapsed) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            return false;
        };
        let Ok(now) = i64::try_from(elapsed.as_secs()) else {
            return false;
        };

        self.verify_at(key, max_age_seconds, now)
    }

    fn verify_at(&self, key: &str, max_age_seconds: i64, now_epoch_seconds: i64) -> bool {
        if max_age_seconds <= 0 {
            return false;
        }
        let Some(age) = now_epoch_seconds.checked_sub(self.expires) else {
            return false;
        };
        if age < 0 || age > max_age_seconds {
            return false;
        }

        // Verify signature (constant-time comparison to prevent timing attacks)
        let expected_sign = Self::gen_sign(&self.random, self.expires, key);
        let a = self.sign.as_bytes();
        let b = expected_sign.as_bytes();
        if a.len() != b.len() || a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) != 0 {
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "test-signing-key";

    fn signed_token(expires: i64) -> CsrfToken {
        let random = "00".repeat(32);
        let sign = CsrfToken::gen_sign(&random, expires, KEY);
        CsrfToken { random, expires, sign }
    }

    #[test]
    fn verify_at_preserves_the_inclusive_lifetime_boundary() {
        let token = signed_token(100);
        assert!(token.verify_at(KEY, 10, 109));
        assert!(token.verify_at(KEY, 10, 110));
        assert!(!token.verify_at(KEY, 10, 111));
    }

    #[test]
    fn verify_at_rejects_nonpositive_lifetimes_future_tokens_and_extreme_timestamps() {
        let token = signed_token(100);
        assert!(!token.verify_at(KEY, 0, 100));
        assert!(!token.verify_at(KEY, -1, 100));
        assert!(!token.verify_at(KEY, 10, 99));

        let earliest = signed_token(i64::MIN);
        assert!(!earliest.verify_at(KEY, i64::MAX, i64::MAX));
        let latest = signed_token(i64::MAX);
        assert!(!latest.verify_at(KEY, i64::MAX, i64::MIN));
    }

    #[test]
    fn verify_at_rejects_an_invalid_signature() {
        let mut token = signed_token(100);
        token.sign.replace_range(..1, "f");
        assert!(!token.verify_at(KEY, 10, 100));
    }

    #[test]
    fn token_wire_shape_and_signature_input_remain_stable() {
        let token = signed_token(100);
        assert_eq!(
            token.sign,
            "9810c1eed8795dab07eda4939817f0cff36454b7a0fc7a735f41d356efbfdb95"
        );

        let encoded = token.encode().unwrap();
        let decoded_bytes = general_purpose::STANDARD.decode(&encoded).unwrap();
        let wire: serde_json::Value = serde_json::from_slice(&decoded_bytes).unwrap();
        assert_eq!(wire["random"], "00".repeat(32));
        assert_eq!(wire["expires"], 100);
        assert_eq!(wire["sign"], token.sign);

        let decoded = CsrfToken::decode(&encoded).unwrap();
        assert_eq!(decoded.random, token.random);
        assert_eq!(decoded.expires, token.expires);
        assert_eq!(decoded.sign, token.sign);
    }

    #[test]
    fn generated_nonce_is_exactly_32_bytes() {
        let token = CsrfToken::generate(KEY);
        assert_eq!(hex::decode(token.random).unwrap().len(), 32);
    }

    #[test]
    fn decode_rejects_invalid_base64_utf8_and_json_without_dynamic_errors() {
        assert!(CsrfToken::decode("not-base64!").is_err());
        assert!(CsrfToken::decode("/w==").is_err());
        assert!(CsrfToken::decode("bm90LWpzb24=").is_err());
    }
}
