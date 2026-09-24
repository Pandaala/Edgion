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
#[path = "../../../tests/unit/services/acme/challenge_store_test.rs"]
mod tests;
