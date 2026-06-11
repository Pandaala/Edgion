//! ACME HTTP-01 Challenge Token Store (Gateway side)
//!
//! Maintains an in-memory store of active ACME challenge tokens.
//! Checked in `request_filter` after request metadata is populated to serve
//! HTTP-01 challenge responses.
//!
//! The store is populated by the ConfHandler when EdgionAcme resources
//! with active_challenges are synced from the Controller.

use crate::core::gateway::observe::metrics::record_acme_challenge_lookup;
use arc_swap::ArcSwap;
use bytes::Bytes;
use edgion_resources::resources::edgion_acme::EdgionAcme;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single challenge entry in the store.
///
/// `key_authorization` is stored as `Bytes` so the request hot path can return
/// it via cheap refcount-bumping clones instead of allocating a fresh `String`
/// per lookup hit. Downstream writes the body as `Bytes` regardless.
#[derive(Debug, Clone)]
pub struct ChallengeEntry {
    pub domain: String,
    pub key_authorization: Bytes,
    pub expire_at: u64,
}

type ChallengeTokenMap = HashMap<String, ChallengeEntry>;
type ResourceTokenMap = HashMap<String, Vec<String>>;

/// Global challenge token store
///
/// Thread-safe store designed for extremely fast lookup in the request hot path:
/// - `is_empty()` is an O(1) ArcSwap snapshot load in the common case
/// - Token lookup is an O(1) ArcSwap snapshot load + HashMap access
///
/// Internally tracks tokens per resource key so that `partial_update` can
/// correctly add/remove tokens without losing challenges from other resources.
pub struct AcmeChallengeStore {
    /// Flat map: token -> ChallengeEntry (lock-free snapshot reads in hot path)
    tokens: ArcSwap<ChallengeTokenMap>,
    /// Per-resource tracking: resource_key -> list of token strings
    /// Used to correctly remove tokens when a resource is updated or deleted.
    resource_tokens: RwLock<ResourceTokenMap>,
}

impl AcmeChallengeStore {
    pub fn new() -> Self {
        Self {
            tokens: ArcSwap::from_pointee(HashMap::new()),
            resource_tokens: RwLock::new(HashMap::new()),
        }
    }

    fn build_entry(domain: &str, key_authorization: &str, expire_at: u64) -> ChallengeEntry {
        ChallengeEntry {
            domain: domain.to_string(),
            key_authorization: Bytes::from(key_authorization.to_string()),
            expire_at,
        }
    }

    /// Check if the store has any active challenges.
    /// This is the fast path check in the ACME request short-circuit.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tokens.load().is_empty()
    }

    /// Look up a challenge token and return the key authorization if found and not expired.
    /// Also validates the domain matches.
    pub fn lookup(&self, token: &str, host: &str) -> Option<Bytes> {
        let tokens = self.tokens.load();
        if let Some(entry) = tokens.get(token) {
            // Check expiry
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if now > entry.expire_at {
                record_acme_challenge_lookup("expired");
                return None;
            }

            // Check domain matches (ASCII-case-insensitive per RFC 4343).
            // An empty/unparseable host can no longer match: ACME HTTP-01
            // validation always carries a Host header equal to the domain being
            // validated, so an empty host is never a legitimate hit and must not
            // bypass the per-domain binding.
            if host.is_empty() {
                record_acme_challenge_lookup("empty_host");
                return None;
            }
            if entry.domain.eq_ignore_ascii_case(host) {
                record_acme_challenge_lookup("hit");
                return Some(entry.key_authorization.clone());
            }

            record_acme_challenge_lookup("domain_mismatch");
            return None;
        }
        record_acme_challenge_lookup("not_found");
        None
    }

    /// Replace all tokens with a new set from EdgionAcme resources.
    /// Called by the ConfHandler on full_set.
    pub fn full_set(&self, data: &HashMap<String, EdgionAcme>) {
        let mut new_tokens: ChallengeTokenMap = HashMap::new();
        let mut new_resource_tokens: ResourceTokenMap = HashMap::new();

        for (key, acme) in data {
            let challenges = acme.active_challenges();
            if !challenges.is_empty() {
                let mut keys_for_resource = Vec::new();
                for c in challenges {
                    keys_for_resource.push(c.token.clone());
                    new_tokens.insert(
                        c.token.clone(),
                        Self::build_entry(&c.domain, &c.key_authorization, c.expire_at),
                    );
                }
                new_resource_tokens.insert(key.clone(), keys_for_resource);
            }
        }

        let token_count = new_tokens.len();

        {
            let mut resource_tokens = self.resource_tokens.write();
            *resource_tokens = new_resource_tokens;
            self.tokens.store(Arc::new(new_tokens));
        }

        if token_count != 0 {
            tracing::info!(count = token_count, "ACME challenge store updated (full_set)");
        }
    }

    /// Incrementally update tokens for specific resources.
    ///
    /// - `upsert`: resources that were added or updated — replace their tokens
    /// - `remove_keys`: resource keys that were deleted — remove their tokens
    pub fn partial_update(&self, upsert: &HashMap<String, EdgionAcme>, remove_keys: &HashSet<String>) {
        let mut resource_tokens = self.resource_tokens.write();
        let current = self.tokens.load();
        let mut new_tokens: ChallengeTokenMap = (**current).clone();

        // Remove tokens for deleted resources
        for key in remove_keys {
            if let Some(old_tokens) = resource_tokens.remove(key) {
                for t in old_tokens {
                    new_tokens.remove(&t);
                }
            }
        }

        // Upsert tokens for added/updated resources
        for (key, acme) in upsert {
            // Remove old tokens for this resource first
            if let Some(old_tokens) = resource_tokens.remove(key) {
                for t in &old_tokens {
                    new_tokens.remove(t);
                }
            }

            // Insert new tokens
            let challenges = acme.active_challenges();
            if !challenges.is_empty() {
                let mut keys_for_resource = Vec::new();
                for c in challenges {
                    keys_for_resource.push(c.token.clone());
                    new_tokens.insert(
                        c.token.clone(),
                        Self::build_entry(&c.domain, &c.key_authorization, c.expire_at),
                    );
                }
                resource_tokens.insert(key.clone(), keys_for_resource);
            }
        }

        self.tokens.store(Arc::new(new_tokens));
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
    use edgion_resources::resources::edgion_acme::ActiveHttpChallenge;

    /// Helper: build a minimal EdgionAcme from JSON with optional active challenges.
    fn make_acme(challenges: Vec<ActiveHttpChallenge>) -> EdgionAcme {
        let active = serde_json::to_value(&challenges).unwrap();

        serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionAcme",
            "metadata": { "name": "test", "namespace": "default" },
            "spec": {
                "email": "test@example.com",
                "privateKeySecretRef": { "name": "test-acme-account" },
                "domains": ["example.com"],
                "challenge": {
                    "type": "http-01",
                    "gatewayRef": { "name": "gw" },
                },
                "storage": { "secretName": "test-cert" },
            },
            "status": {
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
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 300
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
            make_acme(vec![challenge(
                "example.com",
                "test-token-123",
                "test-token-123.thumbprint",
                ft,
            )]),
        );
        store.full_set(&data);

        assert!(!store.is_empty());

        // Valid lookup
        let result = store.lookup("test-token-123", "example.com");
        assert_eq!(result, Some(Bytes::from_static(b"test-token-123.thumbprint")));

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
            make_acme(vec![challenge(
                "example.com",
                "expired-token",
                "expired-token.thumbprint",
                0,
            )]),
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
        data.insert(
            "ns/a".to_string(),
            make_acme(vec![challenge("a.com", "token-a", "auth-a", ft)]),
        );
        store.full_set(&data);
        assert!(store.lookup("token-a", "a.com").is_some());

        // Replace with new set
        let mut data2 = HashMap::new();
        data2.insert(
            "ns/b".to_string(),
            make_acme(vec![challenge("b.com", "token-b", "auth-b", ft)]),
        );
        store.full_set(&data2);

        // Old token gone
        assert!(store.lookup("token-a", "a.com").is_none());
        // New token available
        assert!(store.lookup("token-b", "b.com").is_some());
    }

    #[test]
    fn test_empty_host_no_longer_matches() {
        let store = AcmeChallengeStore::new();
        let ft = future_time();

        let mut data = HashMap::new();
        data.insert(
            "ns/acme1".to_string(),
            make_acme(vec![challenge("example.com", "token-1", "auth-1", ft)]),
        );
        store.full_set(&data);

        // An empty/unparseable Host must NOT match: it can no longer bypass the
        // per-domain binding the domain check enforces. The token still exists
        // and is unexpired, so this exercises the new `empty_host` miss path.
        assert!(store.lookup("token-1", "").is_none());
    }

    #[test]
    fn test_lookup_case_insensitive_domain() {
        let store = AcmeChallengeStore::new();
        let ft = future_time();

        let mut data = HashMap::new();
        data.insert(
            "ns/acme1".to_string(),
            make_acme(vec![challenge("Example.COM", "token-1", "auth-1", ft)]),
        );
        store.full_set(&data);

        // Stored domain is mixed-case; request host in lower-case must hit
        // (RFC 4343: DNS/Host comparisons are case-insensitive).
        assert_eq!(
            store.lookup("token-1", "example.com"),
            Some(Bytes::from_static(b"auth-1"))
        );
        // Request host in a different mixed-case must also hit.
        assert_eq!(
            store.lookup("token-1", "EXAMPLE.com"),
            Some(Bytes::from_static(b"auth-1"))
        );
        // Negative guard: a genuinely different domain still misses
        // (case-insensitivity must not collapse into "match any").
        assert!(store.lookup("token-1", "other.com").is_none());
    }

    #[test]
    fn test_partial_update_add_and_remove() {
        let store = AcmeChallengeStore::new();
        let ft = future_time();

        // Start with two resources
        let mut data = HashMap::new();
        data.insert(
            "ns/a".to_string(),
            make_acme(vec![challenge("a.com", "token-a", "auth-a", ft)]),
        );
        data.insert(
            "ns/b".to_string(),
            make_acme(vec![challenge("b.com", "token-b", "auth-b", ft)]),
        );
        store.full_set(&data);

        assert!(store.lookup("token-a", "a.com").is_some());
        assert!(store.lookup("token-b", "b.com").is_some());

        // Partial update: remove resource "ns/a", add resource "ns/c"
        let mut upsert = HashMap::new();
        upsert.insert(
            "ns/c".to_string(),
            make_acme(vec![challenge("c.com", "token-c", "auth-c", ft)]),
        );
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
        data.insert(
            "ns/a".to_string(),
            make_acme(vec![challenge("a.com", "token-old", "auth-old", ft)]),
        );
        store.full_set(&data);

        assert!(store.lookup("token-old", "a.com").is_some());

        // Update "ns/a" with new token
        let mut upsert = HashMap::new();
        upsert.insert(
            "ns/a".to_string(),
            make_acme(vec![challenge("a.com", "token-new", "auth-new", ft)]),
        );
        store.partial_update(&upsert, &HashSet::new());

        assert!(store.lookup("token-old", "a.com").is_none());
        assert!(store.lookup("token-new", "a.com").is_some());
    }

    #[test]
    fn test_partial_update_after_full_set_uses_latest_resource_index() {
        let store = AcmeChallengeStore::new();
        let ft = future_time();

        let mut data_a = HashMap::new();
        data_a.insert(
            "ns/a".to_string(),
            make_acme(vec![challenge("a.com", "token-a", "auth-a", ft)]),
        );
        store.full_set(&data_a);

        let mut data_b = HashMap::new();
        data_b.insert(
            "ns/b".to_string(),
            make_acme(vec![challenge("b.com", "token-b", "auth-b", ft)]),
        );
        store.full_set(&data_b);

        assert!(store.lookup("token-a", "a.com").is_none());
        assert_eq!(store.lookup("token-b", "b.com"), Some(Bytes::from_static(b"auth-b")));

        let mut remove = HashSet::new();
        remove.insert("ns/b".to_string());
        store.partial_update(&HashMap::new(), &remove);

        assert!(store.is_empty());
        assert!(store.lookup("token-a", "a.com").is_none());
        assert!(store.lookup("token-b", "b.com").is_none());
    }

    #[test]
    fn test_concurrent_reads_during_snapshot_updates() {
        use std::sync::mpsc;
        use std::sync::Arc as StdArc;
        use std::thread;
        use std::time::Duration;

        let store = StdArc::new(AcmeChallengeStore::new());
        let ft = future_time();
        let (published_tx, published_rx) = mpsc::channel::<usize>();
        let (ack_tx, ack_rx) = mpsc::channel::<()>();

        let reader_store = StdArc::clone(&store);
        let reader = thread::spawn(move || {
            let mut reads = 0;

            while let Ok(i) = published_rx.recv() {
                assert!(!reader_store.is_empty());
                if i % 2 == 0 {
                    assert_eq!(
                        reader_store.lookup("token-a", "a.com"),
                        Some(Bytes::from_static(b"auth-a"))
                    );
                    assert!(reader_store.lookup("token-b", "b.com").is_none());
                } else {
                    assert_eq!(
                        reader_store.lookup("token-b", "b.com"),
                        Some(Bytes::from_static(b"auth-b"))
                    );
                    assert!(reader_store.lookup("token-a", "a.com").is_none());
                }

                reads += 1;
                ack_tx.send(()).unwrap();
            }

            reads
        });

        for i in 0..500 {
            let mut data = HashMap::new();
            let token = if i % 2 == 0 { "token-a" } else { "token-b" };
            let domain = if i % 2 == 0 { "a.com" } else { "b.com" };
            let auth = if i % 2 == 0 { "auth-a" } else { "auth-b" };
            data.insert(
                format!("ns/{}", token),
                make_acme(vec![challenge(domain, token, auth, ft)]),
            );
            store.full_set(&data);
            published_tx.send(i).unwrap();
            ack_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }

        drop(published_tx);
        let reads = reader.join().unwrap();

        assert_eq!(reads, 500);
        assert!(store.lookup("token-a", "a.com").is_none());
        assert_eq!(store.lookup("token-b", "b.com"), Some(Bytes::from_static(b"auth-b")));
    }
}
