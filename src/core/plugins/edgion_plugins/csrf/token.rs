use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::time::{SystemTime, UNIX_EPOCH};

/// CSRF Token structure
#[derive(Debug, Serialize, Deserialize)]
pub struct CsrfToken {
    pub random: f64,
    pub expires: i64,
    pub sign: String,
}

impl CsrfToken {
    /// Generate a new CSRF token
    pub fn generate(key: &str) -> Self {
        let random = rand::random::<f64>();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let sign = Self::gen_sign(random, timestamp, key);

        CsrfToken {
            random,
            expires: timestamp,
            sign,
        }
    }

    /// Generate signature for token
    fn gen_sign(random: f64, expires: i64, key: &str) -> String {
        let sign_data = format!("{{expires:{},random:{},key:{}}}", expires, random, key);
        let mut hasher = Sha256::new();
        hasher.update(sign_data.as_bytes());
        let result = hasher.finalize();
        hex::encode(result)
    }

    /// Encode token to base64 string
    pub fn encode(&self) -> Result<String, String> {
        let json = serde_json::to_string(self)
            .map_err(|e| format!("Failed to serialize token: {}", e))?;
        Ok(general_purpose::STANDARD.encode(json.as_bytes()))
    }

    /// Decode token from base64 string
    pub fn decode(token: &str) -> Result<Self, String> {
        let bytes = general_purpose::STANDARD.decode(token)
            .map_err(|e| format!("Base64 decode error: {}", e))?;

        let json = String::from_utf8(bytes)
            .map_err(|e| format!("UTF-8 decode error: {}", e))?;

        serde_json::from_str(&json)
            .map_err(|e| format!("JSON decode error: {}", e))
    }

    /// Verify token signature and expiration
    pub fn verify(&self, key: &str, max_expires: i64) -> bool {
        // Check expiration
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        if max_expires > 0 && now - self.expires > max_expires {
            tracing::debug!("CSRF: Token expired (now: {}, expires: {}, max: {})", now, self.expires, max_expires);
            return false;
        }

        // Verify signature
        let expected_sign = Self::gen_sign(self.random, self.expires, key);
        if self.sign != expected_sign {
            tracing::debug!("CSRF: Invalid signature");
            return false;
        }

        true
    }
}