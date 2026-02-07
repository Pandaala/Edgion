//! ACME HTTP-01 Challenge Token Store (Gateway side)
//!
//! Maintains an in-memory store of active ACME challenge tokens.
//! Checked in `early_request_filter` to serve HTTP-01 challenge responses.
//!
//! The store is populated by the ConfHandler when EdgionAcme resources
//! with active_challenges are synced from the Controller.

use crate::types::resources::edgion_acme::EdgionAcme;
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single challenge entry in the store
#[derive(Debug, Clone)]
pub struct ChallengeEntry {
    pub domain: String,
    pub key_authorization: String,
    pub expire_at: u64,
}

/// Global challenge token store
///
/// Thread-safe store designed for extremely fast lookup in the request hot path:
/// - `is_empty()` check is O(1) and the common case (no active challenges)
/// - Token lookup is O(1) HashMap access
///
/// Internally tracks tokens per resource key so that `partial_update` can
/// correctly add/remove tokens without losing challenges from other resources.
pub struct AcmeChallengeStore {
    /// Flat map: token -> ChallengeEntry (for fast lookup in hot path)
    tokens: RwLock<HashMap<String, ChallengeEntry>>,
    /// Per-resource tracking: resource_key -> list of token strings
    /// Used to correctly remove tokens when a resource is updated or deleted.
    resource_tokens: RwLock<HashMap<String, Vec<String>>>,
}

impl AcmeChallengeStore {
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
            resource_tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Check if the store has any active challenges.
    /// This is the fast path check in early_request_filter.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tokens.read().unwrap().is_empty()
    }

    /// Look up a challenge token and return the key authorization if found and not expired.
    /// Also validates the domain matches.
    pub fn lookup(&self, token: &str, host: &str) -> Option<String> {
        let tokens = self.tokens.read().unwrap();
        if let Some(entry) = tokens.get(token) {
            // Check expiry
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if now > entry.expire_at {
                tracing::debug!(
                    token = token,
                    domain = %entry.domain,
                    "ACME challenge token expired"
                );
                return None;
            }

            // Check domain matches
            if entry.domain == host || host.is_empty() {
                return Some(entry.key_authorization.clone());
            }

            tracing::debug!(
                token = token,
                expected_domain = %entry.domain,
                actual_host = %host,
                "ACME challenge domain mismatch"
            );
        }
        None
    }

    /// Replace all tokens with a new set from EdgionAcme resources.
    /// Called by the ConfHandler on full_set.
    pub fn full_set(&self, data: &HashMap<String, EdgionAcme>) {
        let mut tokens = self.tokens.write().unwrap();
        let mut resource_tokens = self.resource_tokens.write().unwrap();
        tokens.clear();
        resource_tokens.clear();

        for (key, acme) in data {
            if let Some(challenges) = &acme.spec.active_challenges {
                let mut keys_for_resource = Vec::new();
                for c in challenges {
                    keys_for_resource.push(c.token.clone());
                    tokens.insert(
                        c.token.clone(),
                        ChallengeEntry {
                            domain: c.domain.clone(),
                            key_authorization: c.key_authorization.clone(),
                            expire_at: c.expire_at,
                        },
                    );
                }
                if !keys_for_resource.is_empty() {
                    resource_tokens.insert(key.clone(), keys_for_resource);
                }
            }
        }

        if !tokens.is_empty() {
            tracing::info!(
                count = tokens.len(),
                "ACME challenge store updated (full_set)"
            );
        }
    }

    /// Incrementally update tokens for specific resources.
    ///
    /// - `upsert`: resources that were added or updated — replace their tokens
    /// - `remove_keys`: resource keys that were deleted — remove their tokens
    pub fn partial_update(
        &self,
        upsert: &HashMap<String, EdgionAcme>,
        remove_keys: &HashSet<String>,
    ) {
        let mut tokens = self.tokens.write().unwrap();
        let mut resource_tokens = self.resource_tokens.write().unwrap();

        // Remove tokens for deleted resources
        for key in remove_keys {
            if let Some(old_tokens) = resource_tokens.remove(key) {
                for t in old_tokens {
                    tokens.remove(&t);
                }
            }
        }

        // Upsert tokens for added/updated resources
        for (key, acme) in upsert {
            // Remove old tokens for this resource first
            if let Some(old_tokens) = resource_tokens.remove(key) {
                for t in &old_tokens {
                    tokens.remove(t);
                }
            }

            // Insert new tokens
            if let Some(challenges) = &acme.spec.active_challenges {
                let mut keys_for_resource = Vec::new();
                for c in challenges {
                    keys_for_resource.push(c.token.clone());
                    tokens.insert(
                        c.token.clone(),
                        ChallengeEntry {
                            domain: c.domain.clone(),
                            key_authorization: c.key_authorization.clone(),
                            expire_at: c.expire_at,
                        },
                    );
                }
                if !keys_for_resource.is_empty() {
                    resource_tokens.insert(key.clone(), keys_for_resource);
                }
            }
        }
    }
}

impl Default for AcmeChallengeStore {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Global singleton
// ============================================================================

static GLOBAL_CHALLENGE_STORE: std::sync::OnceLock<AcmeChallengeStore> = std::sync::OnceLock::new();

/// Get the global challenge store instance
pub fn get_global_challenge_store() -> &'static AcmeChallengeStore {
    GLOBAL_CHALLENGE_STORE.get_or_init(AcmeChallengeStore::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::edgion_acme::ActiveHttpChallenge;

    /// Helper: build a minimal EdgionAcme from JSON with optional active challenges.
    fn make_acme(challenges: Vec<ActiveHttpChallenge>) -> EdgionAcme {
        let active = if challenges.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::to_value(&challenges).unwrap()
        };

        serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionAcme",
            "metadata": { "name": "test", "namespace": "default" },
            "spec": {
                "email": "test@example.com",
                "domains": ["example.com"],
                "challenge": { "type": "http-01" },
                "storage": { "secretName": "test-cert" },
                "activeChallenges": active,
            }
        }))
        .unwrap()
    }

    /// Helper: build an ActiveHttpChallenge with the given fields.
    fn challenge(domain: &str, token: &str, key_auth: &str, expire_at: u64) -> ActiveHttpChallenge {
        ActiveHttpChallenge {
            domain: domain.to_string(),
            token: token.to_string(),
            key_authorization: key_auth.to_string(),
            expire_at,
        }
    }

    fn future_time() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 300
    }

    #[test]
    fn test_empty_store() {
        let store = AcmeChallengeStore::new();
        assert!(store.is_empty());
        assert!(store.lookup("any-token", "any-host").is_none());
    }

    #[test]
    fn test_lookup_valid_token() {
        let store = AcmeChallengeStore::new();
        let ft = future_time();

        let mut data = HashMap::new();
        data.insert(
            "ns/acme1".to_string(),
            make_acme(vec![challenge("example.com", "test-token-123", "test-token-123.thumbprint", ft)]),
        );
        store.full_set(&data);

        assert!(!store.is_empty());

        // Valid lookup
        let result = store.lookup("test-token-123", "example.com");
        assert_eq!(result, Some("test-token-123.thumbprint".to_string()));

        // Wrong token
        assert!(store.lookup("wrong-token", "example.com").is_none());

        // Wrong domain
        assert!(store.lookup("test-token-123", "wrong.com").is_none());
    }

    #[test]
    fn test_expired_token() {
        let store = AcmeChallengeStore::new();

        let mut data = HashMap::new();
        data.insert(
            "ns/acme1".to_string(),
            make_acme(vec![challenge("example.com", "expired-token", "expired-token.thumbprint", 0)]),
        );
        store.full_set(&data);

        assert!(!store.is_empty());
        assert!(store.lookup("expired-token", "example.com").is_none());
    }

    #[test]
    fn test_full_set_replaces() {
        let store = AcmeChallengeStore::new();
        let ft = future_time();

        let mut data = HashMap::new();
        data.insert("ns/a".to_string(), make_acme(vec![challenge("a.com", "token-a", "auth-a", ft)]));
        store.full_set(&data);
        assert!(store.lookup("token-a", "a.com").is_some());

        // Replace with new set
        let mut data2 = HashMap::new();
        data2.insert("ns/b".to_string(), make_acme(vec![challenge("b.com", "token-b", "auth-b", ft)]));
        store.full_set(&data2);

        // Old token gone
        assert!(store.lookup("token-a", "a.com").is_none());
        // New token available
        assert!(store.lookup("token-b", "b.com").is_some());
    }

    #[test]
    fn test_empty_host_matches_any() {
        let store = AcmeChallengeStore::new();
        let ft = future_time();

        let mut data = HashMap::new();
        data.insert(
            "ns/acme1".to_string(),
            make_acme(vec![challenge("example.com", "token-1", "auth-1", ft)]),
        );
        store.full_set(&data);

        // Empty host should match (for cases where Host header is missing)
        assert!(store.lookup("token-1", "").is_some());
    }

    #[test]
    fn test_partial_update_add_and_remove() {
        let store = AcmeChallengeStore::new();
        let ft = future_time();

        // Start with two resources
        let mut data = HashMap::new();
        data.insert("ns/a".to_string(), make_acme(vec![challenge("a.com", "token-a", "auth-a", ft)]));
        data.insert("ns/b".to_string(), make_acme(vec![challenge("b.com", "token-b", "auth-b", ft)]));
        store.full_set(&data);

        assert!(store.lookup("token-a", "a.com").is_some());
        assert!(store.lookup("token-b", "b.com").is_some());

        // Partial update: remove resource "ns/a", add resource "ns/c"
        let mut upsert = HashMap::new();
        upsert.insert("ns/c".to_string(), make_acme(vec![challenge("c.com", "token-c", "auth-c", ft)]));
        let mut remove = HashSet::new();
        remove.insert("ns/a".to_string());
        store.partial_update(&upsert, &remove);

        // "a" should be gone, "b" untouched, "c" added
        assert!(store.lookup("token-a", "a.com").is_none());
        assert!(store.lookup("token-b", "b.com").is_some());
        assert!(store.lookup("token-c", "c.com").is_some());
    }

    #[test]
    fn test_partial_update_replaces_resource_tokens() {
        let store = AcmeChallengeStore::new();
        let ft = future_time();

        let mut data = HashMap::new();
        data.insert("ns/a".to_string(), make_acme(vec![challenge("a.com", "token-old", "auth-old", ft)]));
        store.full_set(&data);

        assert!(store.lookup("token-old", "a.com").is_some());

        // Update "ns/a" with new token
        let mut upsert = HashMap::new();
        upsert.insert("ns/a".to_string(), make_acme(vec![challenge("a.com", "token-new", "auth-new", ft)]));
        store.partial_update(&upsert, &HashSet::new());

        assert!(store.lookup("token-old", "a.com").is_none());
        assert!(store.lookup("token-new", "a.com").is_some());
    }
}
