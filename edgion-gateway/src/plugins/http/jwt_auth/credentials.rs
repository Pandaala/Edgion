//! JwtAuth credential store: iss -> group, immutable post-construction.
//!
//! Built once from the controller-resolved `resolved_groups_by_iss` map at
//! plugin construction. No `RwLock`, no runtime Secret fetch -- all keys are
//! present in memory before the first request. `Arc<ResolvedJwtIssuerGroup>`
//! lets multiple iss entries pointing to the same group share one allocation.

use std::collections::HashMap;
use std::sync::Arc;

use jsonwebtoken::DecodingKey;

use edgion_resources::resources::edgion_plugins::plugin_configs::{
    JwtAlgorithm, JwtAuthConfig, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
};

/// O(1) iss -> group lookup. Built once at plugin construction.
pub(super) struct JwtIssuerGroupStore {
    by_iss: HashMap<String, Arc<ResolvedJwtIssuerGroup>>,
}

impl JwtIssuerGroupStore {
    pub(super) fn new(by_iss_map: HashMap<String, ResolvedJwtIssuerGroup>) -> Self {
        let arc_map = by_iss_map
            .into_iter()
            .map(|(iss, group)| (iss, Arc::new(group)))
            .collect();
        Self { by_iss: arc_map }
    }

    pub(super) fn lookup_by_iss(&self, iss: &str) -> Option<&Arc<ResolvedJwtIssuerGroup>> {
        self.by_iss.get(iss)
    }
}

/// Build a `DecodingKey` from the credential + algorithm. Used per request
/// (no caching -- parity with pre-grouping behavior; PEM-caching is a
/// future perf optimization outside this pilot's scope, see spec §11).
///
/// Returns `Err(&'static str)` with a short reason if the key material is
/// missing or malformed. The caller maps this to `Deny invalid credentials`.
///
/// Secret storage note: `ResolvedJwtCredential.secret` is stored as the raw
/// UTF-8 string from the K8s Secret data (NOT additionally base64-encoded by
/// the controller). If `config.base64_secret` is true, the value is itself
/// a base64-encoded string that wraps the actual symmetric key bytes.
pub(super) fn build_decoding_key(
    algorithm: JwtAlgorithm,
    cred: &ResolvedJwtCredential,
    config: &JwtAuthConfig,
) -> Result<DecodingKey, &'static str> {
    use JwtAlgorithm::*;
    match algorithm {
        HS256 | HS384 | HS512 => {
            let raw = cred.secret.as_deref().ok_or("missing symmetric secret")?;
            if config.base64_secret {
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(raw.trim())
                    .map_err(|_| "base64 decode failed")?;
                Ok(DecodingKey::from_secret(&decoded))
            } else {
                Ok(DecodingKey::from_secret(raw.as_bytes()))
            }
        }
        RS256 | RS384 | RS512 => {
            let pem = cred.public_key.as_deref().ok_or("missing RSA public_key")?;
            DecodingKey::from_rsa_pem(pem.as_bytes()).map_err(|_| "invalid RSA PEM")
        }
        ES256 | ES384 => {
            let pem = cred.public_key.as_deref().ok_or("missing EC public_key")?;
            DecodingKey::from_ec_pem(pem.as_bytes()).map_err(|_| "invalid EC PEM")
        }
    }
}
