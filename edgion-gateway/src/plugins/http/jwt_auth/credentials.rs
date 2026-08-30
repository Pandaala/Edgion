//! JwtAuth credential store: iss -> group, immutable post-construction.
//!
//! Built once from the controller-resolved `resolved_groups_by_iss` map at
//! plugin construction. No `RwLock`, no runtime Secret fetch -- all keys are
//! parsed and present in memory before the first request.

use std::collections::HashMap;
use std::sync::Arc;

use jsonwebtoken::DecodingKey;

use edgion_resources::resources::edgion_plugins::plugin_configs::{
    JwtAlgorithm, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
};

/// O(1) iss -> group lookup. Built once at plugin construction.
pub(super) struct JwtIssuerGroupStore {
    by_iss: HashMap<String, Arc<CachedJwtIssuerGroup>>,
}

pub(super) struct CachedJwtIssuerGroup {
    pub(super) resolved: Arc<ResolvedJwtIssuerGroup>,
    decoding_keys: HashMap<String, Result<DecodingKey, &'static str>>,
}

impl JwtIssuerGroupStore {
    pub(super) fn new(by_iss_map: HashMap<String, ResolvedJwtIssuerGroup>, base64_secret: bool) -> Self {
        let arc_map = by_iss_map
            .into_iter()
            .map(|(iss, group)| {
                let decoding_keys = group
                    .credentials
                    .iter()
                    .map(|(kid, credential)| {
                        (
                            kid.clone(),
                            build_decoding_key(group.algorithm, credential, base64_secret),
                        )
                    })
                    .collect();
                let cached_group = CachedJwtIssuerGroup {
                    resolved: Arc::new(group),
                    decoding_keys,
                };
                (iss, Arc::new(cached_group))
            })
            .collect();
        Self { by_iss: arc_map }
    }

    pub(super) fn lookup_by_iss(&self, iss: &str) -> Option<&Arc<CachedJwtIssuerGroup>> {
        self.by_iss.get(iss)
    }
}

impl CachedJwtIssuerGroup {
    pub(super) fn lookup_decoding_key(&self, kid: &str) -> Option<Result<&DecodingKey, &'static str>> {
        self.decoding_keys
            .get(kid)
            .map(|result| result.as_ref().map_err(|reason| *reason))
    }
}

/// Build a `DecodingKey` from the credential + algorithm at store construction.
///
/// Returns `Err(&'static str)` with a short reason if the key material is
/// missing or malformed. The caller maps this to `Deny invalid credentials`.
///
/// Secret storage note: `ResolvedJwtCredential.secret` is stored as the raw
/// UTF-8 string from the K8s Secret data (NOT additionally base64-encoded by
/// the controller). If `base64_secret` is true, the value is itself
/// a base64-encoded string that wraps the actual symmetric key bytes.
fn build_decoding_key(
    algorithm: JwtAlgorithm,
    cred: &ResolvedJwtCredential,
    base64_secret: bool,
) -> Result<DecodingKey, &'static str> {
    use JwtAlgorithm::*;
    match algorithm {
        HS256 | HS384 | HS512 => {
            let raw = cred.secret.as_deref().ok_or("missing symmetric secret")?;
            if base64_secret {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn group(algorithm: JwtAlgorithm, credential: ResolvedJwtCredential) -> ResolvedJwtIssuerGroup {
        ResolvedJwtIssuerGroup {
            group: "test-group".to_string(),
            algorithm,
            audiences: None,
            credentials: HashMap::from([("test-kid".to_string(), credential)]),
        }
    }

    #[test]
    fn store_prebuilds_invalid_rsa_key_as_cached_error() {
        let credential = ResolvedJwtCredential {
            key: Some("test-kid".to_string()),
            secret: None,
            public_key: Some("not a PEM key".to_string()),
        };
        let store = JwtIssuerGroupStore::new(
            HashMap::from([(
                "https://test.example".to_string(),
                group(JwtAlgorithm::RS256, credential),
            )]),
            false,
        );

        let cached_group = store
            .lookup_by_iss("https://test.example")
            .expect("issuer must be cached");
        assert!(matches!(
            cached_group.lookup_decoding_key("test-kid"),
            Some(Err("invalid RSA PEM"))
        ));
    }
}
