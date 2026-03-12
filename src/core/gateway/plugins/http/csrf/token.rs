use base64::{engine::general_purpose, Engine as _};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

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

        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;

        let sign = Self::gen_sign(&random, timestamp, key);

        CsrfToken {
            random,
            expires: timestamp,
            sign,
        }
    }

    /// Generate signature for token
    fn gen_sign(random: &str, expires: i64, key: &str) -> String {
        let sign_data = format!("{{expires:{},random:{},key:{}}}", expires, random, key);
        let mut hasher = Sha256::new();
        hasher.update(sign_data.as_bytes());
        let result = hasher.finalize();
        hex::encode(result)
    }

    /// Encode token to base64 string
    pub fn encode(&self) -> Result<String, String> {
        let json = serde_json::to_string(self).map_err(|e| format!("Failed to serialize token: {}", e))?;
        Ok(general_purpose::STANDARD.encode(json.as_bytes()))
    }

    /// Decode token from base64 string
    pub fn decode(token: &str) -> Result<Self, String> {
        let bytes = general_purpose::STANDARD
            .decode(token)
            .map_err(|e| format!("Base64 decode error: {}", e))?;

        let json = String::from_utf8(bytes).map_err(|e| format!("UTF-8 decode error: {}", e))?;

        serde_json::from_str(&json).map_err(|e| format!("JSON decode error: {}", e))
    }

    /// Verify token signature and expiration
    pub fn verify(&self, key: &str, max_expires: i64) -> bool {
        // Check expiration
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;

        if max_expires > 0 && now - self.expires > max_expires {
            return false;
        }

        // Verify signature
        let expected_sign = Self::gen_sign(&self.random, self.expires, key);
        if self.sign != expected_sign {
            return false;
        }

        true
    }
}
