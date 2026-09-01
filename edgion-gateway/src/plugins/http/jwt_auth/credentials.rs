//! JwtAuth credential store: iss -> group, immutable post-construction.
//!
//! Built once from the controller-resolved `resolved_groups_by_iss` map at
//! plugin construction. No `RwLock`, no runtime Secret fetch -- all keys are
//! parsed and present in memory before the first request.

use std::collections::HashMap;

use jsonwebtoken::{Algorithm, DecodingKey, Validation};

use edgion_resources::resources::edgion_plugins::plugin_configs::{
    JwtAlgorithm, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
};

use super::to_jwt_algorithm;

/// O(1) iss -> group lookup. Built once at plugin construction.
pub(super) struct JwtIssuerGroupStore {
    by_iss: HashMap<String, CachedJwtIssuerGroup>,
    peek_decoding_key: DecodingKey,
    peek_validation: Validation,
}

pub(super) struct CachedJwtIssuerGroup {
    pub(super) group: String,
    pub(super) algorithm: JwtAlgorithm,
    validation: Validation,
    decoding_keys: HashMap<String, Result<DecodingKey, &'static str>>,
}

impl JwtIssuerGroupStore {
    pub(super) fn new(
        by_iss_map: HashMap<String, ResolvedJwtIssuerGroup>,
        base64_secret: bool,
        lifetime_grace_period: u64,
    ) -> Self {
        let arc_map = by_iss_map
            .into_iter()
            .map(|(iss, group)| {
                let ResolvedJwtIssuerGroup {
                    group,
                    algorithm,
                    audiences,
                    credentials,
                } = group;
                let decoding_keys = credentials
                    .into_iter()
                    .map(|(kid, credential)| (kid, build_decoding_key(algorithm, credential, base64_secret)))
                    .collect();
                let validation = build_validation(algorithm, audiences, lifetime_grace_period);
                let cached_group = CachedJwtIssuerGroup {
                    group,
                    algorithm,
                    validation,
                    decoding_keys,
                };
                (iss, cached_group)
            })
            .collect();

        let mut peek_validation = Validation::new(Algorithm::HS256);
        peek_validation.insecure_disable_signature_validation();
        peek_validation.validate_exp = false;
        peek_validation.validate_nbf = false;
        peek_validation.validate_aud = false;
        peek_validation.required_spec_claims.clear();

        Self {
            by_iss: arc_map,
            peek_decoding_key: DecodingKey::from_secret(&[]),
            peek_validation,
        }
    }

    pub(super) fn lookup_by_iss(&self, iss: &str) -> Option<&CachedJwtIssuerGroup> {
        self.by_iss.get(iss)
    }

    pub(super) fn peek_decoding_key(&self) -> &DecodingKey {
        &self.peek_decoding_key
    }

    pub(super) fn peek_validation(&self) -> &Validation {
        &self.peek_validation
    }
}

impl CachedJwtIssuerGroup {
    pub(super) fn lookup_decoding_key(&self, kid: &str) -> Option<Result<&DecodingKey, &'static str>> {
        self.decoding_keys
            .get(kid)
            .map(|result| result.as_ref().map_err(|reason| *reason))
    }

    pub(super) fn validation(&self) -> &Validation {
        &self.validation
    }
}

fn build_validation(algorithm: JwtAlgorithm, audiences: Option<Vec<String>>, lifetime_grace_period: u64) -> Validation {
    let mut validation = Validation::new(to_jwt_algorithm(algorithm));
    validation.leeway = lifetime_grace_period;
    validation.validate_nbf = true;
    match audiences {
        Some(audiences) if !audiences.is_empty() => {
            validation.aud = Some(audiences.into_iter().collect());
            validation.required_spec_claims.insert("aud".to_string());
        }
        _ => validation.validate_aud = false,
    }
    validation
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
    cred: ResolvedJwtCredential,
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
            0,
        );

        let cached_group = store
            .lookup_by_iss("https://test.example")
            .expect("issuer must be cached");
        assert!(matches!(
            cached_group.lookup_decoding_key("test-kid"),
            Some(Err("invalid RSA PEM"))
        ));
        assert!(matches!(
            cached_group.lookup_decoding_key("test-kid"),
            Some(Err("invalid RSA PEM"))
        ));
    }

    #[test]
    fn prepared_validation_preserves_exp_and_configures_optional_claims() {
        let configured = build_validation(
            JwtAlgorithm::HS256,
            Some(vec!["api-a".to_string(), "api-b".to_string()]),
            17,
        );
        assert!(configured.required_spec_claims.contains("exp"));
        assert!(configured.required_spec_claims.contains("aud"));
        assert!(configured.validate_aud);
        assert!(configured.validate_nbf);
        assert_eq!(configured.leeway, 17);
        assert_eq!(configured.aud.as_ref().map(|aud| aud.len()), Some(2));

        for audiences in [None, Some(Vec::new())] {
            let disabled = build_validation(JwtAlgorithm::HS256, audiences, 0);
            assert!(disabled.required_spec_claims.contains("exp"));
            assert!(!disabled.required_spec_claims.contains("aud"));
            assert!(!disabled.validate_aud);
            assert!(disabled.validate_nbf);
        }
    }

    #[test]
    fn prepared_key_and_validation_are_reused() {
        let credential = ResolvedJwtCredential {
            key: Some("test-kid".to_string()),
            secret: Some("a-secret-long-enough-for-the-test".to_string()),
            public_key: None,
        };
        let store = JwtIssuerGroupStore::new(
            HashMap::from([(
                "https://test.example".to_string(),
                group(JwtAlgorithm::HS256, credential),
            )]),
            false,
            9,
        );
        let first_group = store.lookup_by_iss("https://test.example").unwrap();
        let second_group = store.lookup_by_iss("https://test.example").unwrap();
        assert!(std::ptr::eq(first_group, second_group));
        assert!(std::ptr::eq(first_group.validation(), second_group.validation()));
        assert!(std::ptr::eq(
            first_group.lookup_decoding_key("test-kid").unwrap().unwrap(),
            second_group.lookup_decoding_key("test-kid").unwrap().unwrap(),
        ));
    }
}
