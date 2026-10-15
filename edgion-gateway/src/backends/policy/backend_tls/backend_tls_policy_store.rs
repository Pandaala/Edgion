//! Typed immutable runtime store for `BackendTLSPolicy` resources.

use arc_swap::ArcSwap;
use edgion_resources::constants::secret_keys::tls::CA_CERT;
use edgion_resources::resources::common::policy_precedence::policy_precedence_key;
use edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKey;
#[cfg(test)]
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKind, ResolvedPolicyTargetClass};
use edgion_resources::resources::refs::policy_fields::BackendTlsPolicyTargets;
use edgion_resources::resources::BackendTLSPolicy;
use rpds::{HashTrieMapSync as PersistentHashMap, HashTrieSetSync as PersistentHashSet};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

static GLOBAL_BACKEND_TLS_POLICY_STORE: LazyLock<Arc<BackendTLSPolicyStore>> =
    LazyLock::new(|| Arc::new(BackendTLSPolicyStore::new()));

pub fn get_global_backend_tls_policy_store() -> Arc<BackendTLSPolicyStore> {
    GLOBAL_BACKEND_TLS_POLICY_STORE.clone()
}

pub(crate) const BACKEND_TLS_PROFILE_DOMAIN: &[u8] = b"edgion/backend-tls-profile/v1\0";

type BackendTLSPolicyMap = HashMap<String, Arc<BackendTLSPolicy>>;
// Partial publications clone only HAMT roots and changed paths. Unaffected
// policy and target entries remain structurally shared with the prior snapshot.
type PersistentBackendTLSPolicyMap = PersistentHashMap<String, Arc<BackendTLSPolicy>>;

#[derive(Clone)]
pub enum BackendTlsPolicyLookup {
    Absent,
    Effective(Arc<EffectiveBackendTlsPolicy>),
    Invalid,
}

impl BackendTlsPolicyLookup {
    #[inline]
    pub fn effective(&self) -> Option<&Arc<EffectiveBackendTlsPolicy>> {
        match self {
            Self::Effective(policy) => Some(policy),
            Self::Absent | Self::Invalid => None,
        }
    }

    #[inline]
    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid)
    }

    #[inline]
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    pub fn debug_label(&self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Effective(_) => "effective",
            Self::Invalid => "invalid",
        }
    }

    #[cfg(test)]
    pub(crate) fn effective_from_policy(policy: Arc<BackendTLSPolicy>) -> Self {
        Self::Effective(EffectiveBackendTlsPolicy::from_policy(policy))
    }
}

impl std::fmt::Debug for BackendTlsPolicyLookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.debug_label())
    }
}

pub struct EffectiveBackendTlsPolicy {
    pub ancestor_admission:
        Option<edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission>,
    pub policy: Arc<BackendTLSPolicy>,
    pub profile: Arc<BackendTlsProfileIdentity>,
    #[cfg(feature = "boringssl")]
    pub(in crate::backends) prepared: Arc<crate::backends::prepared_backend_tls::PreparedBackendTls>,
}

pub struct BackendTlsProfileIdentity {
    pub fingerprint: [u8; 32],
}

impl EffectiveBackendTlsPolicy {
    #[cfg(test)]
    pub(crate) fn from_policy(policy: Arc<BackendTLSPolicy>) -> Arc<Self> {
        Arc::new(Self {
            profile: BackendTLSPolicyStore::profile_identity(&policy),
            ancestor_admission: policy
                .spec
                .prepared_ancestor_admission
                .clone()
                .map(|value| value.validated()),
            #[cfg(feature = "boringssl")]
            prepared: Arc::new(crate::backends::prepared_backend_tls::PreparedBackendTls::new(&policy)),
            policy,
        })
    }
}

impl BackendTlsProfileIdentity {
    pub fn group_key(&self) -> u64 {
        u64::from_be_bytes(self.fingerprint[..8].try_into().expect("fingerprint is 32 bytes"))
    }
}

#[derive(Default)]
struct ValidBuckets {
    exact: HashMap<Arc<str>, Arc<EffectiveBackendTlsPolicy>>,
    whole: Option<Arc<EffectiveBackendTlsPolicy>>,
}

#[derive(Default)]
struct InvalidBuckets {
    exact: HashSet<Arc<str>>,
    whole: bool,
    any_section_scoped: bool,
}

#[derive(Default)]
struct TargetBuckets {
    // All canonicalizable policy attachments for this typed target, including
    // invalid candidates. Keeping losers makes winner removal locally repairable.
    candidates: PersistentHashSet<String>,
    valid: ValidBuckets,
    invalid: InvalidBuckets,
}

struct BackendTLSPolicySnapshotData {
    generation: u64,
    policies: PersistentBackendTLSPolicyMap,
    targets: PersistentHashMap<PolicyTargetKey, Arc<TargetBuckets>>,
}

/// One immutable policy generation. Captured handles remain valid after reload.
#[derive(Clone)]
pub struct BackendTLSPolicySnapshot {
    inner: Arc<BackendTLSPolicySnapshotData>,
}

impl BackendTLSPolicySnapshot {
    #[inline]
    pub fn generation(&self) -> u64 {
        self.inner.generation
    }

    #[inline]
    pub fn get(&self, key: &str) -> Option<Arc<BackendTLSPolicy>> {
        self.inner.policies.get(key).cloned()
    }

    #[cfg(test)]
    fn target_buckets(&self, target: &PolicyTargetKey) -> Option<Arc<TargetBuckets>> {
        self.inner.targets.get(target).cloned()
    }

    #[inline]
    pub fn has_candidate(&self, target: &PolicyTargetKey) -> bool {
        self.inner.targets.contains_key(target)
    }

    #[inline]
    pub fn any_section_scoped(&self, target: &PolicyTargetKey) -> bool {
        self.inner
            .targets
            .get(target)
            .is_some_and(|buckets| !buckets.valid.exact.is_empty() || buckets.invalid.any_section_scoped)
    }

    /// Occupancy-first exact-section then whole-target lookup.
    pub fn lookup(&self, target: &PolicyTargetKey, section: Option<&str>) -> BackendTlsPolicyLookup {
        let buckets = self.inner.targets.get(target);
        if let Some(section) = section {
            if let Some(winner) = buckets.and_then(|buckets| buckets.valid.exact.get(section)) {
                return BackendTlsPolicyLookup::Effective(Arc::clone(winner));
            }
            if buckets.is_some_and(|buckets| buckets.invalid.exact.contains(section)) {
                return BackendTlsPolicyLookup::Invalid;
            }
        }
        if let Some(winner) = buckets.and_then(|buckets| buckets.valid.whole.as_ref()) {
            return BackendTlsPolicyLookup::Effective(Arc::clone(winner));
        }
        if buckets.is_some_and(|buckets| buckets.invalid.whole) {
            return BackendTlsPolicyLookup::Invalid;
        }
        BackendTlsPolicyLookup::Absent
    }
}

pub struct BackendTLSPolicyStore {
    snapshot: ArcSwap<BackendTLSPolicySnapshotData>,
}

impl BackendTLSPolicyStore {
    pub fn new() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(BackendTLSPolicySnapshotData {
                generation: 0,
                policies: PersistentHashMap::new_sync(),
                targets: PersistentHashMap::new_sync(),
            }),
        }
    }

    #[inline]
    pub fn snapshot(&self) -> BackendTLSPolicySnapshot {
        BackendTLSPolicySnapshot {
            inner: self.snapshot.load_full(),
        }
    }

    #[inline]
    fn policy_resource_equivalent(left: &BackendTLSPolicy, right: &BackendTLSPolicy) -> bool {
        left.metadata == right.metadata
            && left.spec.prepared_precedence == right.spec.prepared_precedence
            && left.spec.prepared_ancestor_admission == right.spec.prepared_ancestor_admission
            && left.spec.target_refs == right.spec.target_refs
            && left.spec.resolved_target_class == right.spec.resolved_target_class
            && left.spec.validation == right.spec.validation
            && left.spec.options == right.spec.options
            && left.spec.resolved_ca_certificates == right.spec.resolved_ca_certificates
            && left.spec.resolved_client_certificate == right.spec.resolved_client_certificate
            && left.spec.use_system_ca == right.spec.use_system_ca
            && left.spec.current_status.as_ref().map(|status| &status.ancestors)
                == right.spec.current_status.as_ref().map(|status| &status.ancestors)
    }

    fn valid_target_key(policy: &BackendTLSPolicy) -> Option<PolicyTargetKey> {
        edgion_resources::resources::common::policy_candidates::prepared_backend_tls_target(policy)
    }

    fn sort_policies_by_precedence(policies: &mut [Arc<BackendTLSPolicy>]) {
        // Unprepared policies remain in invalid occupancy, never valid winners.
        policies.sort_by(|a, b| {
            let key = |policy: &BackendTLSPolicy| {
                policy
                    .spec
                    .prepared_precedence
                    .and_then(|prepared| prepared.creation_timestamp)
            };
            policy_precedence_key(key(a), a.name()).cmp(&policy_precedence_key(key(b), b.name()))
        });
    }

    pub(crate) fn profile_identity(policy: &BackendTLSPolicy) -> Arc<BackendTlsProfileIdentity> {
        let mut digest = Sha256::new();
        digest.update(BACKEND_TLS_PROFILE_DOMAIN);
        if policy.spec.use_system_ca {
            write_len_prefixed(&mut digest, b"system");
        } else {
            write_len_prefixed(&mut digest, b"custom");
        }
        let mut ca_payloads: Vec<&[u8]> = policy
            .spec
            .resolved_ca_certificates
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|secret| secret.data.as_ref()?.get(CA_CERT).map(|value| value.0.as_slice()))
            .collect();
        ca_payloads.sort_unstable();
        ca_payloads.dedup();
        digest.update((ca_payloads.len() as u64).to_be_bytes());
        for payload in ca_payloads {
            write_len_prefixed(&mut digest, payload);
        }
        write_len_prefixed(&mut digest, policy.spec.validation.hostname.as_bytes());
        if let Some(sans) = policy.spec.validation.subject_alt_names.as_ref() {
            digest.update((sans.len() as u64).to_be_bytes());
            for san in sans {
                match san.san_type {
                    edgion_resources::resources::backend_tls_policy::SubjectAltNameType::Hostname => {
                        digest.update([b'H']);
                        write_len_prefixed(&mut digest, san.hostname.as_deref().unwrap_or("").as_bytes());
                    }
                    edgion_resources::resources::backend_tls_policy::SubjectAltNameType::URI => {
                        digest.update([b'U']);
                        write_len_prefixed(&mut digest, san.uri.as_deref().unwrap_or("").as_bytes());
                    }
                }
            }
        } else {
            digest.update(0u64.to_be_bytes());
        }
        if let Some(client) = policy.spec.resolved_client_certificate.as_ref() {
            write_len_prefixed(&mut digest, b"policy");
            if let Some(data) = client.data.as_ref() {
                let mut keys: Vec<_> = data.keys().collect();
                keys.sort();
                for key in keys {
                    write_len_prefixed(&mut digest, key.as_bytes());
                    write_len_prefixed(&mut digest, data[key].0.as_slice());
                }
            }
        } else {
            write_len_prefixed(&mut digest, b"none");
        }
        digest.update([u8::from(policy.spec.use_system_ca)]);
        Arc::new(BackendTlsProfileIdentity {
            fingerprint: digest.finalize().into(),
        })
    }

    fn mark_invalid(buckets: &mut InvalidBuckets, section: Option<&str>) {
        match section {
            Some(section) if !section.is_empty() => {
                buckets.exact.insert(Arc::from(section));
                buckets.any_section_scoped = true;
            }
            _ => buckets.whole = true,
        }
    }

    fn reuse_or_build_effective(
        current_valid: Option<&ValidBuckets>,
        section: Option<&str>,
        policy: &Arc<BackendTLSPolicy>,
    ) -> Arc<EffectiveBackendTlsPolicy> {
        let existing = current_valid.and_then(|buckets| match section {
            Some(name) if !name.is_empty() => buckets.exact.get(name),
            _ => buckets.whole.as_ref(),
        });
        if let Some(existing) = existing.filter(|existing| Arc::ptr_eq(&existing.policy, policy)) {
            return Arc::clone(existing);
        }
        Arc::new(EffectiveBackendTlsPolicy {
            policy: Arc::clone(policy),
            ancestor_admission: policy
                .spec
                .prepared_ancestor_admission
                .clone()
                .map(|value| value.validated()),
            profile: Self::profile_identity(policy),
            #[cfg(feature = "boringssl")]
            prepared: existing
                .filter(|existing| {
                    crate::backends::prepared_backend_tls::PreparedBackendTls::inputs_equivalent(
                        &existing.policy,
                        policy,
                    )
                })
                .map(|existing| Arc::clone(&existing.prepared))
                .unwrap_or_else(|| Arc::new(crate::backends::prepared_backend_tls::PreparedBackendTls::new(policy))),
        })
    }

    fn canonical_targets(policy: &BackendTLSPolicy) -> HashSet<PolicyTargetKey> {
        BackendTlsPolicyTargets::from_policy(policy)
            .fields()
            .filter_map(|field| field.canonical_key().ok())
            .collect()
    }

    fn build_target_buckets(
        target: &PolicyTargetKey,
        candidates: PersistentHashSet<String>,
        policies: &PersistentBackendTLSPolicyMap,
        current: Option<&TargetBuckets>,
    ) -> TargetBuckets {
        let mut ordered: Vec<Arc<BackendTLSPolicy>> =
            candidates.iter().filter_map(|key| policies.get(key).cloned()).collect();
        Self::sort_policies_by_precedence(&mut ordered);
        let mut valid = ValidBuckets::default();
        let mut invalid = InvalidBuckets::default();
        for policy in ordered {
            if Self::valid_target_key(&policy).as_ref() == Some(target) {
                let section = policy.spec.target_refs[0].section_name.clone();
                match section {
                    Some(section) if !section.is_empty() => {
                        if !valid.exact.contains_key(section.as_str()) {
                            let effective = Self::reuse_or_build_effective(
                                current.map(|current| &current.valid),
                                Some(&section),
                                &policy,
                            );
                            valid.exact.insert(Arc::from(section), effective);
                        }
                    }
                    _ => {
                        if valid.whole.is_none() {
                            valid.whole = Some(Self::reuse_or_build_effective(
                                current.map(|current| &current.valid),
                                None,
                                &policy,
                            ));
                        }
                    }
                }
                continue;
            }
            for field in BackendTlsPolicyTargets::from_policy(&policy).fields() {
                if field.canonical_key().as_ref() == Ok(target) {
                    Self::mark_invalid(&mut invalid, field.section_name());
                }
            }
        }
        TargetBuckets {
            candidates,
            valid,
            invalid,
        }
    }

    fn policies_unchanged(current: &PersistentBackendTLSPolicyMap, next: &PersistentBackendTLSPolicyMap) -> bool {
        current.size() == next.size()
            && next
                .iter()
                .all(|(key, policy)| current.get(key).is_some_and(|old| Arc::ptr_eq(old, policy)))
    }

    fn build_full_targets(
        policies: &PersistentBackendTLSPolicyMap,
        current: &PersistentHashMap<PolicyTargetKey, Arc<TargetBuckets>>,
    ) -> PersistentHashMap<PolicyTargetKey, Arc<TargetBuckets>> {
        let mut candidates_by_target = HashMap::<PolicyTargetKey, PersistentHashSet<String>>::new();
        for (key, policy) in policies {
            for target in Self::canonical_targets(policy) {
                candidates_by_target.entry(target).or_default().insert_mut(key.clone());
            }
        }
        candidates_by_target
            .into_iter()
            .map(|(target, candidates)| {
                let buckets =
                    Self::build_target_buckets(&target, candidates, policies, current.get(&target).map(Arc::as_ref));
                (target, Arc::new(buckets))
            })
            .collect()
    }

    fn publish_full(&self, incoming: BackendTLSPolicyMap) {
        let current = self.snapshot.load_full();
        let policies: PersistentBackendTLSPolicyMap = incoming
            .into_iter()
            .map(|(key, policy)| {
                let policy = current
                    .policies
                    .get(&key)
                    .filter(|old| Self::policy_resource_equivalent(old, &policy))
                    .cloned()
                    .unwrap_or(policy);
                (key, policy)
            })
            .collect();
        if Self::policies_unchanged(&current.policies, &policies) {
            return;
        }
        let targets = Self::build_full_targets(&policies, &current.targets);
        self.snapshot.store(Arc::new(BackendTLSPolicySnapshotData {
            generation: current
                .generation
                .checked_add(1)
                .expect("BackendTLSPolicy snapshot generation exhausted"),
            policies,
            targets,
        }));
    }

    fn build_incremental_snapshot(
        current: &BackendTLSPolicySnapshotData,
        add: BackendTLSPolicyMap,
        update: BackendTLSPolicyMap,
        remove: &HashSet<String>,
    ) -> Option<Arc<BackendTLSPolicySnapshotData>> {
        // Preserve the handler's defensive overlap semantics: update replaces
        // add, and remove replaces both. Production compressed batches are
        // normally disjoint, but direct callers rely on this deterministic rule.
        let mut changes = HashMap::<String, Option<Arc<BackendTLSPolicy>>>::new();
        changes.extend(add.into_iter().map(|(key, policy)| (key, Some(policy))));
        changes.extend(update.into_iter().map(|(key, policy)| (key, Some(policy))));
        changes.extend(remove.iter().cloned().map(|key| (key, None)));

        let mut policies = current.policies.clone();
        let mut candidate_deltas = HashMap::<PolicyTargetKey, (HashSet<String>, HashSet<String>)>::new();
        let mut changed = false;
        for (key, incoming) in changes {
            let previous = current.policies.get(&key);
            let incoming = incoming.map(|policy| {
                previous
                    .filter(|old| Self::policy_resource_equivalent(old, &policy))
                    .cloned()
                    .unwrap_or(policy)
            });
            if matches!((previous, incoming.as_ref()), (None, None))
                || matches!((previous, incoming.as_ref()), (Some(old), Some(new)) if Arc::ptr_eq(old, new))
            {
                continue;
            }
            changed = true;
            if let Some(previous) = previous {
                for target in Self::canonical_targets(previous) {
                    candidate_deltas.entry(target).or_default().0.insert(key.clone());
                }
            }
            if let Some(incoming) = incoming {
                for target in Self::canonical_targets(&incoming) {
                    candidate_deltas.entry(target).or_default().1.insert(key.clone());
                }
                policies.insert_mut(key, incoming);
            } else {
                policies.remove_mut(&key);
            }
        }
        if !changed {
            return None;
        }

        let mut targets = current.targets.clone();
        for (target, (removed, added)) in candidate_deltas {
            let mut candidates = current
                .targets
                .get(&target)
                .map_or_else(PersistentHashSet::new_sync, |buckets| buckets.candidates.clone());
            for key in removed {
                candidates.remove_mut(&key);
            }
            for key in added {
                candidates.insert_mut(key);
            }
            if candidates.is_empty() {
                targets.remove_mut(&target);
                continue;
            }
            let buckets = Self::build_target_buckets(
                &target,
                candidates,
                &policies,
                current.targets.get(&target).map(Arc::as_ref),
            );
            targets.insert_mut(target, Arc::new(buckets));
        }

        Some(Arc::new(BackendTLSPolicySnapshotData {
            generation: current
                .generation
                .checked_add(1)
                .expect("BackendTLSPolicy snapshot generation exhausted"),
            policies,
            targets,
        }))
    }

    pub fn contains(&self, key: &str) -> bool {
        self.snapshot.load().policies.contains_key(key)
    }

    pub fn get(&self, key: &str) -> Option<Arc<BackendTLSPolicy>> {
        self.snapshot().get(key)
    }

    pub fn get_by_ns_name(&self, namespace: &str, name: &str) -> Option<Arc<BackendTLSPolicy>> {
        self.get(&format!("{namespace}/{name}"))
    }

    pub fn replace_all(&self, new_policies: BackendTLSPolicyMap) {
        self.publish_full(new_policies);
    }

    pub fn update(&self, add: BackendTLSPolicyMap, update: BackendTLSPolicyMap, remove: &HashSet<String>) {
        if add.is_empty() && update.is_empty() && remove.is_empty() {
            return;
        }
        // Production calls are serialized by the conf-sync CacheData write lock.
        // A second writer would require RCU/CAS publication to avoid lost updates.
        let current = self.snapshot.load();
        if let Some(next) = Self::build_incremental_snapshot(&current, add, update, remove) {
            self.snapshot.store(next);
        }
    }

    pub fn count(&self) -> usize {
        self.snapshot.load().policies.size()
    }

    pub fn stats(&self) -> BackendTLSPolicyStoreStats {
        let snapshot = self.snapshot.load();
        BackendTLSPolicyStoreStats {
            policies: snapshot.policies.size(),
            reverse_index_targets: snapshot
                .targets
                .values()
                .filter(|buckets| !buckets.valid.exact.is_empty() || buckets.valid.whole.is_some())
                .count(),
            invalid_index_targets: snapshot
                .targets
                .values()
                .filter(|buckets| !buckets.invalid.exact.is_empty() || buckets.invalid.whole)
                .count(),
        }
    }
}

fn write_len_prefixed(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackendTLSPolicyStoreStats {
    pub policies: usize,
    pub reverse_index_targets: usize,
    pub invalid_index_targets: usize,
}

impl Default for BackendTLSPolicyStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/policy/backend_tls/backend_tls_policy_store_test.rs"]
mod tests;
