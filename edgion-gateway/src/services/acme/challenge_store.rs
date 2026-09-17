//! ACME HTTP-01 Challenge Token Store (Gateway side)
//!
//! Maintains an in-memory store of active ACME challenge tokens.
//! Checked in `request_filter` after request metadata is populated to serve
//! HTTP-01 challenge responses.
//!
//! The store is populated by the ConfHandler when EdgionAcme resources
//! with active_challenges are synced from the Controller.

use crate::observe::metrics::record_acme_challenge_lookup;
use crate::runtime::hostname_matches_listener;
use crate::runtime::store::GatewayInfoIndex;
use arc_swap::ArcSwap;
use bytes::Bytes;
use edgion_resources::resources::edgion_acme::{EdgionAcme, MAX_ACME_RESOLVED_ATTACHMENTS};
use edgion_resources::resources::edgion_tls::GatewayAttachmentProof;
use edgion_resources::ListenerKey;
use parking_lot::RwLock;
use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single challenge entry in the store.
///
/// `key_authorization` is stored as `Bytes` so the request hot path can return
/// it via cheap refcount-bumping clones instead of allocating a fresh `String`
/// per lookup hit. Downstream writes the body as `Bytes` regardless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledAcmeAttachment {
    listener_key: ListenerKey,
    gateway_key: Arc<str>,
    listener_name: Arc<str>,
    gateway_proof: GatewayAttachmentProof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChallengeEntry {
    resource_key: Arc<str>,
    pub domain: Arc<str>,
    pub key_authorization: Bytes,
    pub expire_at: u64,
    attachments: Arc<[CompiledAcmeAttachment]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChallengeBucket {
    Candidates(Arc<[ChallengeEntry]>),
    Overflow,
}

#[derive(Debug, Clone)]
struct ResourceChallenge {
    token: String,
    entry: ChallengeEntry,
}

const MAX_ACME_TOKEN_CANDIDATES: usize = 64;

type ChallengeTokenMap = HashMap<String, ChallengeBucket>;
type ResourceChallengeMap = HashMap<String, Vec<ResourceChallenge>>;

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
    /// Per-resource compiled challenges used to rebuild token buckets without
    /// losing colliding candidates when a resource is updated or deleted.
    resource_challenges: RwLock<ResourceChallengeMap>,
}

impl AcmeChallengeStore {
    pub fn new() -> Self {
        Self {
            tokens: ArcSwap::from_pointee(HashMap::new()),
            resource_challenges: RwLock::new(HashMap::new()),
        }
    }

    fn compile_resource(resource_key: &str, acme: &EdgionAcme) -> Vec<ResourceChallenge> {
        let Some(resolved) = acme.spec.resolved_listener_attachments.as_deref() else {
            return Vec::new();
        };
        if resolved.is_empty() || resolved.len() > MAX_ACME_RESOLVED_ATTACHMENTS {
            return Vec::new();
        }

        let mut attachments = Vec::with_capacity(resolved.len());
        for attachment in resolved {
            if attachment.gateway_namespace.is_empty()
                || attachment.gateway_name.is_empty()
                || attachment.listener_name.is_empty()
                || attachment.listener_key.protocol() != "HTTP"
                || !attachment.gateway_proof.is_resolved()
            {
                return Vec::new();
            }
            attachments.push(CompiledAcmeAttachment {
                listener_key: attachment.listener_key.clone(),
                gateway_key: Arc::from(format!("{}/{}", attachment.gateway_namespace, attachment.gateway_name)),
                listener_name: Arc::from(attachment.listener_name.as_str()),
                gateway_proof: attachment.gateway_proof.clone(),
            });
        }
        attachments.sort_by(compiled_attachment_cmp);
        attachments.dedup();
        let attachments: Arc<[CompiledAcmeAttachment]> = Arc::from(attachments);
        let resource_key: Arc<str> = Arc::from(resource_key);

        acme.active_challenges()
            .iter()
            .filter(|challenge| {
                !challenge.token.is_empty() && !challenge.domain.is_empty() && !challenge.key_authorization.is_empty()
            })
            .map(|challenge| ResourceChallenge {
                token: challenge.token.clone(),
                entry: ChallengeEntry {
                    resource_key: Arc::clone(&resource_key),
                    domain: Arc::from(challenge.domain.as_str()),
                    key_authorization: Bytes::copy_from_slice(challenge.key_authorization.as_bytes()),
                    expire_at: challenge.expire_at,
                    attachments: Arc::clone(&attachments),
                },
            })
            .collect()
    }

    fn build_token_snapshot(resources: &ResourceChallengeMap) -> ChallengeTokenMap {
        let mut building: HashMap<String, Option<Vec<ChallengeEntry>>> = HashMap::new();
        for challenge in resources.values().flatten() {
            match building.entry(challenge.token.clone()) {
                Entry::Vacant(slot) => {
                    slot.insert(Some(vec![challenge.entry.clone()]));
                }
                Entry::Occupied(mut slot) => {
                    let Some(candidates) = slot.get_mut() else {
                        continue;
                    };
                    if candidates.iter().any(|existing| existing == &challenge.entry) {
                        continue;
                    }
                    if candidates.len() == MAX_ACME_TOKEN_CANDIDATES {
                        slot.insert(None);
                    } else {
                        candidates.push(challenge.entry.clone());
                    }
                }
            }
        }

        building
            .into_iter()
            .map(|(token, candidates)| {
                let bucket = match candidates {
                    Some(mut candidates) => {
                        candidates.sort_by(challenge_entry_cmp);
                        ChallengeBucket::Candidates(Arc::from(candidates))
                    }
                    None => ChallengeBucket::Overflow,
                };
                (token, bucket)
            })
            .collect()
    }

    fn build_token_bucket(resources: &ResourceChallengeMap, token: &str) -> Option<ChallengeBucket> {
        let mut candidates = Vec::new();
        for challenge in resources
            .values()
            .flatten()
            .filter(|challenge| challenge.token == token)
        {
            if candidates.iter().any(|existing| existing == &challenge.entry) {
                continue;
            }
            if candidates.len() == MAX_ACME_TOKEN_CANDIDATES {
                return Some(ChallengeBucket::Overflow);
            }
            candidates.push(challenge.entry.clone());
        }
        if candidates.is_empty() {
            return None;
        }
        candidates.sort_by(challenge_entry_cmp);
        Some(ChallengeBucket::Candidates(Arc::from(candidates)))
    }

    /// Check if the store has any active challenges.
    /// This is the fast path check in the ACME request short-circuit.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tokens.load().is_empty()
    }

    /// Look up a challenge token and return the key authorization if found and not expired.
    /// Also validates the domain matches.
    pub fn lookup_for_listener(
        &self,
        listener_key: &ListenerKey,
        gateway_index: &GatewayInfoIndex,
        token: &str,
        challenge_host: &str,
        listener_host: &str,
    ) -> Option<Bytes> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.lookup_for_listener_at_epoch_seconds(
            listener_key,
            gateway_index,
            token,
            challenge_host,
            listener_host,
            now,
        )
    }

    fn lookup_for_listener_at_epoch_seconds(
        &self,
        listener_key: &ListenerKey,
        gateway_index: &GatewayInfoIndex,
        token: &str,
        challenge_host: &str,
        listener_host: &str,
        now: u64,
    ) -> Option<Bytes> {
        let tokens = self.tokens.load();
        let Some(ChallengeBucket::Candidates(candidates)) = tokens.get(token) else {
            record_acme_challenge_lookup("not_found");
            return None;
        };
        if challenge_host.is_empty() {
            record_acme_challenge_lookup("empty_host");
            return None;
        }
        if !canonical_host_identity(challenge_host).eq_ignore_ascii_case(canonical_host_identity(listener_host)) {
            record_acme_challenge_lookup("domain_mismatch");
            return None;
        }

        let mut saw_domain = false;
        let mut saw_unexpired_domain = false;
        let mut matched: Option<&Bytes> = None;
        for candidate in candidates.iter() {
            if !candidate.domain.eq_ignore_ascii_case(challenge_host) {
                continue;
            }
            saw_domain = true;
            if now >= candidate.expire_at {
                continue;
            }
            saw_unexpired_domain = true;
            if !candidate
                .attachments
                .iter()
                .any(|attachment| attachment_matches(attachment, listener_key, gateway_index, listener_host))
            {
                continue;
            }

            match matched {
                Some(existing) if existing != &candidate.key_authorization => {
                    record_acme_challenge_lookup("not_found");
                    return None;
                }
                Some(_) => {}
                None => matched = Some(&candidate.key_authorization),
            }
        }

        if let Some(key_authorization) = matched {
            record_acme_challenge_lookup("hit");
            return Some(key_authorization.clone());
        }
        if saw_domain && !saw_unexpired_domain {
            record_acme_challenge_lookup("expired");
        } else if !saw_domain {
            record_acme_challenge_lookup("domain_mismatch");
        } else {
            record_acme_challenge_lookup("not_found");
        }
        None
    }

    /// Replace all tokens with a new set from EdgionAcme resources.
    /// Called by the ConfHandler on full_set.
    pub fn full_set(&self, data: &HashMap<String, EdgionAcme>) {
        let new_resource_challenges: ResourceChallengeMap = data
            .iter()
            .filter_map(|(key, acme)| {
                let challenges = Self::compile_resource(key, acme);
                (!challenges.is_empty()).then(|| (key.clone(), challenges))
            })
            .collect();
        let new_tokens = Self::build_token_snapshot(&new_resource_challenges);

        let token_count = new_tokens.len();

        {
            let mut resource_challenges = self.resource_challenges.write();
            *resource_challenges = new_resource_challenges;
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
        let mut resource_challenges = self.resource_challenges.write();
        let mut touched_tokens = HashSet::new();

        // Remove tokens for deleted resources
        for key in remove_keys {
            if let Some(previous) = resource_challenges.remove(key) {
                touched_tokens.extend(previous.into_iter().map(|challenge| challenge.token));
            }
        }

        // Upsert tokens for added/updated resources
        for (key, acme) in upsert {
            if let Some(previous) = resource_challenges.remove(key) {
                touched_tokens.extend(previous.into_iter().map(|challenge| challenge.token));
            }
            let challenges = Self::compile_resource(key, acme);
            touched_tokens.extend(challenges.iter().map(|challenge| challenge.token.clone()));
            if !challenges.is_empty() {
                resource_challenges.insert(key.clone(), challenges);
            }
        }

        let current = self.tokens.load();
        let mut new_tokens = (**current).clone();
        for token in touched_tokens {
            match Self::build_token_bucket(&resource_challenges, &token) {
                Some(bucket) => {
                    new_tokens.insert(token, bucket);
                }
                None => {
                    new_tokens.remove(&token);
                }
            }
        }
        self.tokens.store(Arc::new(new_tokens));
    }
}

fn compiled_attachment_cmp(left: &CompiledAcmeAttachment, right: &CompiledAcmeAttachment) -> Ordering {
    left.listener_key
        .cmp(&right.listener_key)
        .then_with(|| left.gateway_key.cmp(&right.gateway_key))
        .then_with(|| left.listener_name.cmp(&right.listener_name))
        .then_with(|| left.gateway_proof.instance.cmp(&right.gateway_proof.instance))
        .then_with(|| left.gateway_proof.generation.cmp(&right.gateway_proof.generation))
        .then_with(|| left.gateway_proof.epoch.cmp(&right.gateway_proof.epoch))
        .then_with(|| {
            left.gateway_proof
                .attachment_digest
                .cmp(&right.gateway_proof.attachment_digest)
        })
}

fn challenge_entry_cmp(left: &ChallengeEntry, right: &ChallengeEntry) -> Ordering {
    left.resource_key
        .cmp(&right.resource_key)
        .then_with(|| left.domain.cmp(&right.domain))
        .then_with(|| left.key_authorization.cmp(&right.key_authorization))
        .then_with(|| left.expire_at.cmp(&right.expire_at))
}

fn attachment_matches(
    attachment: &CompiledAcmeAttachment,
    listener_key: &ListenerKey,
    gateway_index: &GatewayInfoIndex,
    host: &str,
) -> bool {
    if listener_key.protocol() != "HTTP" || &attachment.listener_key != listener_key {
        return false;
    }
    let Some(live) = gateway_index.get(&attachment.gateway_key, &attachment.listener_name) else {
        return false;
    };
    if live.gateway_proof.as_ref() != Some(&attachment.gateway_proof) {
        return false;
    }
    live.hostname
        .as_deref()
        .is_none_or(|hostname| hostname_matches_listener(host, hostname))
}

fn canonical_host_identity(host: &str) -> &str {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    host.strip_suffix('.').unwrap_or(host)
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
    use crate::runtime::gateway_info::GatewayInfo;
    use crate::runtime::store::listener_gateway_info::ListenerEntry;
    use edgion_resources::resources::edgion_acme::ActiveHttpChallenge;
    use edgion_resources::resources::edgion_tls::ResolvedListenerAttachment;

    fn test_proof() -> GatewayAttachmentProof {
        GatewayAttachmentProof {
            instance: "controller-a".to_string(),
            generation: 1,
            epoch: "epoch-a".to_string(),
            attachment_digest: "a".repeat(64),
        }
    }

    fn test_listener_key() -> ListenerKey {
        ListenerKey("0.0.0.0:80/HTTP".to_string())
    }

    fn test_gateway_index(proof: Option<GatewayAttachmentProof>) -> GatewayInfoIndex {
        test_gateway_index_for("gw", "http", None, proof)
    }

    fn test_gateway_index_for(
        gateway_name: &str,
        listener_name: &str,
        hostname: Option<&str>,
        proof: Option<GatewayAttachmentProof>,
    ) -> GatewayInfoIndex {
        GatewayInfoIndex::from_entries_for_test(vec![(
            format!("default/{gateway_name}"),
            ListenerEntry {
                info: Arc::new(GatewayInfo::new(
                    Some("default".to_string()),
                    gateway_name.to_string(),
                    Some(listener_name.to_string()),
                    None,
                    None,
                    None,
                    Vec::new(),
                    None,
                )),
                gateway_proof: proof,
                hostname: hostname.map(str::to_string),
                allowed_routes: None,
            },
        )])
    }

    impl AcmeChallengeStore {
        fn lookup(&self, token: &str, host: &str) -> Option<Bytes> {
            self.lookup_for_listener(
                &test_listener_key(),
                &test_gateway_index(Some(test_proof())),
                token,
                host,
                host,
            )
        }

        fn lookup_at_epoch_seconds(&self, token: &str, host: &str, now: u64) -> Option<Bytes> {
            self.lookup_for_listener_at_epoch_seconds(
                &test_listener_key(),
                &test_gateway_index(Some(test_proof())),
                token,
                host,
                host,
                now,
            )
        }
    }

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
                "resolvedListenerAttachments": [{
                    "gatewayNamespace": "default",
                    "gatewayName": "gw",
                    "listenerName": "http",
                    "listenerKey": "0.0.0.0:80/HTTP",
                    "gatewayProof": {
                        "instance": "controller-a",
                        "generation": 1,
                        "epoch": "epoch-a",
                        "attachmentDigest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    }
                }],
            },
            "status": {
                "activeChallenges": active,
            }
        }))
        .unwrap()
    }

    fn make_acme_for(
        challenges: Vec<ActiveHttpChallenge>,
        gateway_name: &str,
        listener_name: &str,
        listener_key: ListenerKey,
        proof: GatewayAttachmentProof,
    ) -> EdgionAcme {
        let mut acme = make_acme(challenges);
        acme.spec.resolved_listener_attachments = Some(vec![ResolvedListenerAttachment {
            gateway_namespace: "default".to_string(),
            gateway_name: gateway_name.to_string(),
            listener_name: listener_name.to_string(),
            listener_key,
            gateway_proof: proof,
        }]);
        acme
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
    fn test_expiration_boundary_is_exclusive() {
        let store = AcmeChallengeStore::new();
        let expire_at = 100;

        let mut data = HashMap::new();
        data.insert(
            "ns/acme1".to_string(),
            make_acme(vec![challenge(
                "example.com",
                "boundary-token",
                "boundary-token.thumbprint",
                expire_at,
            )]),
        );
        store.full_set(&data);

        assert_eq!(
            store.lookup_at_epoch_seconds("boundary-token", "example.com", expire_at - 1),
            Some(Bytes::from_static(b"boundary-token.thumbprint"))
        );
        assert!(store
            .lookup_at_epoch_seconds("boundary-token", "example.com", expire_at)
            .is_none());
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

        let mut replacement = HashMap::new();
        replacement.insert(
            "ns/a".to_string(),
            make_acme(vec![challenge("a.com", "token-new", "auth-new", ft)]),
        );
        store.partial_update(&replacement, &HashSet::new());
        assert!(store.lookup("token-old", "a.com").is_none());
        assert_eq!(
            store.lookup("token-new", "a.com"),
            Some(Bytes::from_static(b"auth-new"))
        );
    }

    #[test]
    fn partial_update_drops_token_when_attachments_or_challenges_are_revoked() {
        let store = AcmeChallengeStore::new();
        let listener_key = test_listener_key();
        let index = test_gateway_index(Some(test_proof()));
        let ft = future_time();
        let key = "default/acme".to_string();
        let serving = make_acme(vec![challenge(
            "example.com",
            "ownership-token",
            "ownership-token.authorization",
            ft,
        )]);
        store.partial_update(&HashMap::from([(key.clone(), serving.clone())]), &HashSet::new());
        assert_eq!(
            store.lookup_for_listener(&listener_key, &index, "ownership-token", "example.com", "example.com",),
            Some(Bytes::from_static(b"ownership-token.authorization"))
        );

        let mut without_attachments = serving.clone();
        without_attachments.spec.resolved_listener_attachments = None;
        store.partial_update(&HashMap::from([(key.clone(), without_attachments)]), &HashSet::new());
        assert!(store
            .lookup_for_listener(&listener_key, &index, "ownership-token", "example.com", "example.com",)
            .is_none());

        store.partial_update(&HashMap::from([(key.clone(), serving.clone())]), &HashSet::new());
        assert!(store
            .lookup_for_listener(&listener_key, &index, "ownership-token", "example.com", "example.com",)
            .is_some());

        let mut without_challenges = serving;
        without_challenges
            .status
            .as_mut()
            .expect("make_acme writes status")
            .active_challenges
            .clear();
        store.partial_update(&HashMap::from([(key, without_challenges)]), &HashSet::new());
        assert!(store
            .lookup_for_listener(&listener_key, &index, "ownership-token", "example.com", "example.com",)
            .is_none());
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

    #[test]
    fn lookup_requires_physical_logical_proof_and_hostname_ownership() {
        let store = AcmeChallengeStore::new();
        let listener_key = test_listener_key();
        let proof = test_proof();
        let mut data = HashMap::new();
        data.insert(
            "default/acme".to_string(),
            make_acme_for(
                vec![challenge("api.example.com", "owned", "authorization", future_time())],
                "gw",
                "http",
                listener_key.clone(),
                proof.clone(),
            ),
        );
        store.full_set(&data);

        let matching = test_gateway_index_for("gw", "http", Some("*.example.com"), Some(proof.clone()));
        assert_eq!(
            store.lookup_for_listener(&listener_key, &matching, "owned", "API.Example.com", "api.example.com",),
            Some(Bytes::from_static(b"authorization"))
        );
        assert!(store
            .lookup_for_listener(
                &listener_key,
                &matching,
                "owned",
                "api.example.com",
                "other.example.com",
            )
            .is_none());

        let wrong_gateway = test_gateway_index_for("other", "http", None, Some(proof.clone()));
        assert!(store
            .lookup_for_listener(
                &listener_key,
                &wrong_gateway,
                "owned",
                "api.example.com",
                "api.example.com",
            )
            .is_none());
        let wrong_listener = test_gateway_index_for("gw", "other", None, Some(proof.clone()));
        assert!(store
            .lookup_for_listener(
                &listener_key,
                &wrong_listener,
                "owned",
                "api.example.com",
                "api.example.com",
            )
            .is_none());
        let stale = test_gateway_index_for(
            "gw",
            "http",
            None,
            Some(GatewayAttachmentProof {
                generation: 2,
                ..proof.clone()
            }),
        );
        assert!(store
            .lookup_for_listener(&listener_key, &stale, "owned", "api.example.com", "api.example.com",)
            .is_none());
        let wrong_hostname = test_gateway_index_for("gw", "http", Some("other.example.com"), Some(proof));
        assert!(store
            .lookup_for_listener(
                &listener_key,
                &wrong_hostname,
                "owned",
                "api.example.com",
                "api.example.com",
            )
            .is_none());
        let wrong_physical = ListenerKey("127.0.0.1:80/HTTP".to_string());
        assert!(store
            .lookup_for_listener(
                &wrong_physical,
                &matching,
                "owned",
                "api.example.com",
                "api.example.com",
            )
            .is_none());
    }

    #[test]
    fn conflicting_authorizations_fail_closed_but_identical_authorizations_coexist() {
        let store = AcmeChallengeStore::new();
        let listener_key = test_listener_key();
        let index = test_gateway_index(Some(test_proof()));
        let ft = future_time();
        let mut data = HashMap::new();
        for (resource, authorization) in [("a", "same"), ("b", "same")] {
            data.insert(
                format!("default/{resource}"),
                make_acme(vec![challenge("example.com", "collision", authorization, ft)]),
            );
        }
        store.full_set(&data);
        assert_eq!(
            store.lookup_for_listener(&listener_key, &index, "collision", "example.com", "example.com",),
            Some(Bytes::from_static(b"same"))
        );

        data.insert(
            "default/b".to_string(),
            make_acme(vec![challenge("example.com", "collision", "different", ft)]),
        );
        store.full_set(&data);
        assert!(store
            .lookup_for_listener(&listener_key, &index, "collision", "example.com", "example.com",)
            .is_none());
    }

    #[test]
    fn token_candidate_overflow_fails_closed_and_partial_removal_recovers() {
        let store = AcmeChallengeStore::new();
        let listener_key = test_listener_key();
        let index = test_gateway_index(Some(test_proof()));
        let ft = future_time();
        let mut data = HashMap::new();
        for i in 0..MAX_ACME_TOKEN_CANDIDATES {
            data.insert(
                format!("default/acme-{i}"),
                make_acme(vec![challenge("example.com", "shared", "same", ft)]),
            );
        }
        store.full_set(&data);
        assert_eq!(
            store.lookup_for_listener(&listener_key, &index, "shared", "example.com", "example.com",),
            Some(Bytes::from_static(b"same"))
        );

        let overflow_key = format!("default/acme-{}", MAX_ACME_TOKEN_CANDIDATES);
        let mut upsert = HashMap::new();
        upsert.insert(
            overflow_key.clone(),
            make_acme(vec![challenge("example.com", "shared", "same", ft)]),
        );
        store.partial_update(&upsert, &HashSet::new());
        assert!(store
            .lookup_for_listener(&listener_key, &index, "shared", "example.com", "example.com",)
            .is_none());

        let mut remove = HashSet::new();
        remove.insert(overflow_key);
        store.partial_update(&HashMap::new(), &remove);
        assert_eq!(
            store.lookup_for_listener(&listener_key, &index, "shared", "example.com", "example.com",),
            Some(Bytes::from_static(b"same"))
        );
    }

    #[test]
    fn forged_oversized_attachment_vector_is_not_published() {
        let store = AcmeChallengeStore::new();
        let mut resource = make_acme(vec![challenge(
            "example.com",
            "oversized",
            "authorization",
            future_time(),
        )]);
        let attachment = resource.spec.resolved_listener_attachments.as_ref().unwrap()[0].clone();
        resource.spec.resolved_listener_attachments = Some(vec![attachment; MAX_ACME_RESOLVED_ATTACHMENTS + 1]);
        store.full_set(&HashMap::from([("default/oversized".to_string(), resource)]));
        assert!(store.lookup("oversized", "example.com").is_none());
    }

    #[test]
    fn full_and_partial_updates_build_the_same_canonical_buckets() {
        let ft = future_time();
        let data = HashMap::from([
            (
                "default/a".to_string(),
                make_acme(vec![challenge("a.example.com", "token", "same", ft)]),
            ),
            (
                "default/b".to_string(),
                make_acme(vec![challenge("b.example.com", "token", "same", ft)]),
            ),
        ]);
        let full = AcmeChallengeStore::new();
        full.full_set(&data);

        let partial = AcmeChallengeStore::new();
        for (key, acme) in &data {
            partial.partial_update(&HashMap::from([(key.clone(), acme.clone())]), &HashSet::new());
        }

        assert_eq!(**full.tokens.load(), **partial.tokens.load());
    }
}
