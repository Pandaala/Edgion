//! Typed immutable runtime store for `BackendTLSPolicy` resources.

use arc_swap::ArcSwap;
use edgion_resources::constants::secret_keys::tls::CA_CERT;
use edgion_resources::resources::backend_tls_policy::backend_tls_identity_validation_errors;
use edgion_resources::resources::edgion_backend_traffic_policy::precedence_key;
use edgion_resources::resources::edgion_backend_traffic_policy::{
    PolicyTargetKey, PolicyTargetKind, ResolvedPolicyTargetClass,
};
use edgion_resources::resources::BackendTLSPolicy;
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
    pub policy: Arc<BackendTLSPolicy>,
    pub profile: Arc<BackendTlsProfileIdentity>,
}

pub struct BackendTlsProfileIdentity {
    pub fingerprint: [u8; 32],
}

impl EffectiveBackendTlsPolicy {
    #[cfg(test)]
    pub(crate) fn from_policy(policy: Arc<BackendTLSPolicy>) -> Arc<Self> {
        Arc::new(Self {
            profile: BackendTLSPolicyStore::profile_identity(&policy),
            policy,
        })
    }
}

impl BackendTlsProfileIdentity {
    pub fn group_key(&self) -> u64 {
        u64::from_be_bytes(self.fingerprint[..8].try_into().expect("fingerprint is 32 bytes"))
    }
}

#[derive(Clone, Default)]
struct ValidBuckets {
    exact: HashMap<Arc<str>, Arc<EffectiveBackendTlsPolicy>>,
    whole: Option<Arc<EffectiveBackendTlsPolicy>>,
}

#[derive(Clone, Default)]
struct InvalidBuckets {
    exact: HashSet<Arc<str>>,
    whole: bool,
    any_section_scoped: bool,
}

struct BackendTLSPolicySnapshotData {
    generation: u64,
    policies: BackendTLSPolicyMap,
    valid_index: HashMap<PolicyTargetKey, ValidBuckets>,
    invalid_index: HashMap<PolicyTargetKey, InvalidBuckets>,
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

    #[inline]
    pub fn has_candidate(&self, target: &PolicyTargetKey) -> bool {
        self.inner.valid_index.contains_key(target) || self.inner.invalid_index.contains_key(target)
    }

    #[inline]
    pub fn any_section_scoped(&self, target: &PolicyTargetKey) -> bool {
        self.inner
            .valid_index
            .get(target)
            .is_some_and(|buckets| !buckets.exact.is_empty())
            || self
                .inner
                .invalid_index
                .get(target)
                .is_some_and(|buckets| buckets.any_section_scoped)
    }

    /// Occupancy-first exact-section then whole-target lookup.
    pub fn lookup(&self, target: &PolicyTargetKey, section: Option<&str>) -> BackendTlsPolicyLookup {
        let valid = self.inner.valid_index.get(target);
        let invalid = self.inner.invalid_index.get(target);
        if let Some(section) = section {
            if let Some(winner) = valid.and_then(|buckets| buckets.exact.get(section)) {
                return BackendTlsPolicyLookup::Effective(Arc::clone(winner));
            }
            if invalid.is_some_and(|buckets| buckets.exact.contains(section)) {
                return BackendTlsPolicyLookup::Invalid;
            }
        }
        if let Some(winner) = valid.and_then(|buckets| buckets.whole.as_ref()) {
            return BackendTlsPolicyLookup::Effective(Arc::clone(winner));
        }
        if invalid.is_some_and(|buckets| buckets.whole) {
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
                policies: HashMap::new(),
                valid_index: HashMap::new(),
                invalid_index: HashMap::new(),
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
        left.metadata.name == right.metadata.name
            && left.metadata.namespace == right.metadata.namespace
            && left.metadata.uid == right.metadata.uid
            && left.metadata.creation_timestamp == right.metadata.creation_timestamp
            && left.spec.target_refs == right.spec.target_refs
            && left.spec.resolved_target_class == right.spec.resolved_target_class
            && left.spec.validation == right.spec.validation
            && left.spec.options == right.spec.options
            && Self::secret_data_vec_eq(
                left.spec.resolved_ca_certificates.as_deref(),
                right.spec.resolved_ca_certificates.as_deref(),
            )
            && Self::secret_data_eq(
                left.spec.resolved_client_certificate.as_ref(),
                right.spec.resolved_client_certificate.as_ref(),
            )
            && left.spec.use_system_ca == right.spec.use_system_ca
    }

    fn secret_data_eq(
        left: Option<&k8s_openapi::api::core::v1::Secret>,
        right: Option<&k8s_openapi::api::core::v1::Secret>,
    ) -> bool {
        match (left, right) {
            (None, None) => true,
            (Some(left), Some(right)) => {
                left.metadata.name == right.metadata.name
                    && left.metadata.namespace == right.metadata.namespace
                    && left.type_ == right.type_
                    && left.data == right.data
            }
            _ => false,
        }
    }

    fn secret_data_vec_eq(
        left: Option<&[k8s_openapi::api::core::v1::Secret]>,
        right: Option<&[k8s_openapi::api::core::v1::Secret]>,
    ) -> bool {
        match (left, right) {
            (None, None) => true,
            (Some(left), Some(right)) if left.len() == right.len() => left.iter().zip(right).all(|(left, right)| {
                left.metadata.name == right.metadata.name
                    && left.metadata.namespace == right.metadata.namespace
                    && left.type_ == right.type_
                    && left.data == right.data
            }),
            _ => false,
        }
    }

    fn retain_equivalent_arcs(current: &BackendTLSPolicyMap, incoming: BackendTLSPolicyMap) -> BackendTLSPolicyMap {
        incoming
            .into_iter()
            .map(|(key, policy)| {
                let policy = current
                    .get(&key)
                    .filter(|old| Self::policy_resource_equivalent(old, &policy))
                    .cloned()
                    .unwrap_or(policy);
                (key, policy)
            })
            .collect()
    }

    fn is_valid_candidate(policy: &BackendTLSPolicy) -> bool {
        if policy.spec.target_refs.len() != 1 {
            return false;
        }
        let Some(resolved_class) = policy.spec.resolved_target_class else {
            return false;
        };
        let namespace = policy.namespace().unwrap_or("");
        let Some(target_ref) = policy.spec.target_refs.first() else {
            return false;
        };
        let Ok(key) = target_ref.canonical_key(namespace) else {
            return false;
        };
        let class_matches = matches!(
            (resolved_class, key.target_kind),
            (ResolvedPolicyTargetClass::Service, PolicyTargetKind::Service)
                | (ResolvedPolicyTargetClass::Ai, PolicyTargetKind::EdgionBackend)
        );
        if !class_matches {
            return false;
        }
        if target_ref.section_name.is_some() && key.target_kind != PolicyTargetKind::Service {
            return false;
        }
        if !backend_tls_identity_validation_errors(&policy.spec.validation).is_empty() {
            return false;
        }
        if !policy.spec.use_system_ca
            && policy
                .spec
                .validation
                .ca_certificate_refs
                .as_ref()
                .is_some_and(|refs| !refs.is_empty())
            && !Self::has_resolved_ca_payload(policy)
        {
            return false;
        }
        true
    }

    fn has_resolved_ca_payload(policy: &BackendTLSPolicy) -> bool {
        policy.spec.resolved_ca_certificates.as_ref().is_some_and(|secrets| {
            secrets.iter().any(|secret| {
                secret
                    .data
                    .as_ref()
                    .and_then(|data| data.get(CA_CERT))
                    .is_some_and(|value| !value.0.is_empty())
            })
        })
    }

    fn valid_target_key(policy: &BackendTLSPolicy) -> Option<PolicyTargetKey> {
        Self::is_valid_candidate(policy).then(|| {
            let namespace = policy.namespace().unwrap_or("");
            policy.spec.target_refs[0]
                .canonical_key(namespace)
                .expect("valid candidate has a typed target key")
        })
    }

    fn sort_policies_by_precedence(policies: &mut [Arc<BackendTLSPolicy>]) {
        policies.sort_by_key(|policy| precedence_key(&policy.metadata));
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

    fn mark_invalid(
        index: &mut HashMap<PolicyTargetKey, InvalidBuckets>,
        target: PolicyTargetKey,
        section: Option<&str>,
    ) {
        let buckets = index.entry(target).or_default();
        match section {
            Some(section) if !section.is_empty() => {
                buckets.exact.insert(Arc::from(section));
                buckets.any_section_scoped = true;
            }
            _ => buckets.whole = true,
        }
    }

    fn reuse_or_build_effective(
        current_valid: &HashMap<PolicyTargetKey, ValidBuckets>,
        target: &PolicyTargetKey,
        section: Option<&str>,
        policy: &Arc<BackendTLSPolicy>,
    ) -> Arc<EffectiveBackendTlsPolicy> {
        let existing = current_valid.get(target).and_then(|buckets| match section {
            Some(name) if !name.is_empty() => buckets.exact.get(name),
            _ => buckets.whole.as_ref(),
        });
        if let Some(existing) = existing.filter(|existing| Arc::ptr_eq(&existing.policy, policy)) {
            return Arc::clone(existing);
        }
        Arc::new(EffectiveBackendTlsPolicy {
            policy: Arc::clone(policy),
            profile: Self::profile_identity(policy),
        })
    }

    fn build_indexes(
        policies: &BackendTLSPolicyMap,
        current_valid: &HashMap<PolicyTargetKey, ValidBuckets>,
    ) -> (
        HashMap<PolicyTargetKey, ValidBuckets>,
        HashMap<PolicyTargetKey, InvalidBuckets>,
    ) {
        let mut ordered: Vec<Arc<BackendTLSPolicy>> = policies.values().cloned().collect();
        Self::sort_policies_by_precedence(&mut ordered);
        let mut valid_index = HashMap::<PolicyTargetKey, ValidBuckets>::new();
        let mut invalid_index = HashMap::<PolicyTargetKey, InvalidBuckets>::new();
        for policy in ordered {
            if let Some(target) = Self::valid_target_key(&policy) {
                let section = policy.spec.target_refs[0].section_name.clone();
                let effective = Self::reuse_or_build_effective(current_valid, &target, section.as_deref(), &policy);
                let buckets = valid_index.entry(target).or_default();
                match section {
                    Some(section) if !section.is_empty() => {
                        buckets.exact.entry(Arc::from(section)).or_insert(effective);
                    }
                    _ => {
                        if buckets.whole.is_none() {
                            buckets.whole = Some(effective);
                        }
                    }
                }
                continue;
            }
            let namespace = policy.namespace().unwrap_or("");
            for target_ref in &policy.spec.target_refs {
                if let Ok(target) = target_ref.canonical_key(namespace) {
                    Self::mark_invalid(&mut invalid_index, target, target_ref.section_name.as_deref());
                }
            }
        }
        (valid_index, invalid_index)
    }

    fn indexes_equivalent(
        left_valid: &HashMap<PolicyTargetKey, ValidBuckets>,
        right_valid: &HashMap<PolicyTargetKey, ValidBuckets>,
        left_invalid: &HashMap<PolicyTargetKey, InvalidBuckets>,
        right_invalid: &HashMap<PolicyTargetKey, InvalidBuckets>,
    ) -> bool {
        left_valid.len() == right_valid.len()
            && right_valid.iter().all(|(target, right)| {
                left_valid.get(target).is_some_and(|left| {
                    let whole_eq = match (&left.whole, &right.whole) {
                        (None, None) => true,
                        (Some(left_policy), Some(right_policy)) => Arc::ptr_eq(left_policy, right_policy),
                        _ => false,
                    };
                    whole_eq
                        && left.exact.len() == right.exact.len()
                        && right.exact.iter().all(|(section, policy)| {
                            left.exact
                                .get(section)
                                .is_some_and(|existing| Arc::ptr_eq(existing, policy))
                        })
                })
            })
            && left_invalid.len() == right_invalid.len()
            && right_invalid.iter().all(|(target, right)| {
                left_invalid.get(target).is_some_and(|left| {
                    left.whole == right.whole
                        && left.any_section_scoped == right.any_section_scoped
                        && left.exact == right.exact
                })
            })
    }

    fn publish(&self, incoming: BackendTLSPolicyMap) {
        let current = self.snapshot.load_full();
        let policies = Self::retain_equivalent_arcs(&current.policies, incoming);
        let (valid_index, invalid_index) = Self::build_indexes(&policies, &current.valid_index);
        let unchanged = policies.len() == current.policies.len()
            && policies
                .iter()
                .all(|(key, policy)| current.policies.get(key).is_some_and(|old| Arc::ptr_eq(old, policy)))
            && Self::indexes_equivalent(
                &current.valid_index,
                &valid_index,
                &current.invalid_index,
                &invalid_index,
            );
        if unchanged {
            return;
        }
        self.snapshot.store(Arc::new(BackendTLSPolicySnapshotData {
            generation: current
                .generation
                .checked_add(1)
                .expect("BackendTLSPolicy snapshot generation exhausted"),
            policies,
            valid_index,
            invalid_index,
        }));
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
        self.publish(new_policies);
    }

    pub fn update(&self, add_or_update: BackendTLSPolicyMap, remove: &HashSet<String>) {
        if add_or_update.is_empty() && remove.is_empty() {
            return;
        }
        let mut policies = self.snapshot.load().policies.clone();
        policies.extend(add_or_update);
        for key in remove {
            policies.remove(key);
        }
        self.publish(policies);
    }

    pub fn count(&self) -> usize {
        self.snapshot.load().policies.len()
    }

    pub fn stats(&self) -> BackendTLSPolicyStoreStats {
        let snapshot = self.snapshot.load();
        BackendTLSPolicyStoreStats {
            policies: snapshot.policies.len(),
            reverse_index_targets: snapshot.valid_index.len(),
            invalid_index_targets: snapshot.invalid_index.len(),
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
mod tests {
    use super::*;
    use edgion_resources::resources::backend_tls_policy::{
        BackendTLSPolicyCACertificateRef, BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation,
        SubjectAltName, SubjectAltNameType,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::ByteString;

    fn policy(
        resource_name: &str,
        target_kind: PolicyTargetKind,
        target_name: &str,
        section: Option<&str>,
        marker: Option<ResolvedPolicyTargetClass>,
    ) -> Arc<BackendTLSPolicy> {
        let (group, kind) = match target_kind {
            PolicyTargetKind::Service => ("", "Service"),
            PolicyTargetKind::EdgionBackend => ("edgion.io", "EdgionBackend"),
        };
        Arc::new(BackendTLSPolicy {
            metadata: ObjectMeta {
                namespace: Some("tenant".to_string()),
                name: Some(resource_name.to_string()),
                uid: Some(format!("uid-{resource_name}")),
                ..Default::default()
            },
            spec: BackendTLSPolicySpec {
                target_refs: vec![BackendTLSPolicyTargetRef {
                    group: group.to_string(),
                    kind: kind.to_string(),
                    name: target_name.to_string(),
                    section_name: section.map(str::to_string),
                }],
                resolved_target_class: marker,
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: None,
                    hostname: "backend.example.com".to_string(),
                    subject_alt_names: None,
                    well_known_ca_certificates: None,
                },
                options: None,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
                use_system_ca: true,
            },
            status: None,
        })
    }

    fn target(kind: PolicyTargetKind) -> PolicyTargetKey {
        PolicyTargetKey {
            target_kind: kind,
            namespace: "tenant".to_string(),
            name: "shared".to_string(),
        }
    }

    fn effective_name(lookup: BackendTlsPolicyLookup) -> Option<String> {
        lookup.effective().map(|policy| policy.policy.name().to_string())
    }

    #[test]
    fn typed_targets_and_sections_do_not_leak() {
        let store = BackendTLSPolicyStore::new();
        store.replace_all(HashMap::from([
            (
                "tenant/service-grpc".to_string(),
                policy(
                    "service-grpc",
                    PolicyTargetKind::Service,
                    "shared",
                    Some("grpc"),
                    Some(ResolvedPolicyTargetClass::Service),
                ),
            ),
            (
                "tenant/backend".to_string(),
                policy(
                    "backend",
                    PolicyTargetKind::EdgionBackend,
                    "shared",
                    None,
                    Some(ResolvedPolicyTargetClass::Ai),
                ),
            ),
        ]));
        let snapshot = store.snapshot();
        assert!(snapshot.lookup(&target(PolicyTargetKind::Service), None).is_absent());
        assert_eq!(
            effective_name(snapshot.lookup(&target(PolicyTargetKind::Service), Some("grpc"))).as_deref(),
            Some("service-grpc")
        );
        assert_eq!(
            effective_name(snapshot.lookup(&target(PolicyTargetKind::EdgionBackend), None)).as_deref(),
            Some("backend")
        );
    }

    #[test]
    fn policy_lookup_does_not_rebuild_target_keys() {
        let store = BackendTLSPolicyStore::new();
        store.replace_all(HashMap::from([(
            "tenant/service-https".to_string(),
            policy(
                "service-https",
                PolicyTargetKind::Service,
                "shared",
                Some("https"),
                Some(ResolvedPolicyTargetClass::Service),
            ),
        )]));
        let snapshot = store.snapshot();
        let target = target(PolicyTargetKind::Service);

        let (selected, allocations) =
            crate::test_allocation_counter::count_allocation_stats(|| snapshot.lookup(&target, Some("https")));

        assert_eq!(effective_name(selected).as_deref(), Some("service-https"));
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }

    #[test]
    fn equivalent_update_retains_generation_arc_and_old_handle() {
        let store = BackendTLSPolicyStore::new();
        let input = policy(
            "policy",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        );
        store.replace_all(HashMap::from([("tenant/policy".to_string(), input)]));
        let first = store.snapshot();
        let first_policy = first
            .lookup(&target(PolicyTargetKind::Service), None)
            .effective()
            .cloned()
            .unwrap();

        store.replace_all(HashMap::from([(
            "tenant/policy".to_string(),
            policy(
                "policy",
                PolicyTargetKind::Service,
                "shared",
                None,
                Some(ResolvedPolicyTargetClass::Service),
            ),
        )]));
        let equivalent = store.snapshot();
        assert_eq!(first.generation(), equivalent.generation());
        assert!(Arc::ptr_eq(
            &first_policy,
            equivalent
                .lookup(&target(PolicyTargetKind::Service), None)
                .effective()
                .unwrap()
        ));

        store.replace_all(HashMap::new());
        assert_eq!(store.snapshot().generation(), first.generation() + 1);
        assert_eq!(first_policy.policy.name(), "policy", "old Arc remains usable");
    }

    #[test]
    fn marker_absent_or_mismatched_is_invalid_not_absent() {
        let store = BackendTLSPolicyStore::new();
        store.replace_all(HashMap::from([
            (
                "tenant/absent".to_string(),
                policy("absent", PolicyTargetKind::Service, "shared", None, None),
            ),
            (
                "tenant/mismatch".to_string(),
                policy(
                    "mismatch",
                    PolicyTargetKind::Service,
                    "shared",
                    None,
                    Some(ResolvedPolicyTargetClass::Ai),
                ),
            ),
        ]));
        assert!(store
            .snapshot()
            .lookup(&target(PolicyTargetKind::Service), None)
            .is_invalid());
    }

    #[test]
    fn multiple_target_refs_are_invalid_on_every_named_service() {
        let store = BackendTLSPolicyStore::new();
        let mut multi = policy(
            "multi",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        );
        Arc::make_mut(&mut multi)
            .spec
            .target_refs
            .push(BackendTLSPolicyTargetRef {
                group: String::new(),
                kind: "Service".to_string(),
                name: "other".to_string(),
                section_name: None,
            });
        store.replace_all(HashMap::from([("tenant/multi".to_string(), multi)]));

        assert!(store
            .snapshot()
            .lookup(&target(PolicyTargetKind::Service), None)
            .is_invalid());
        assert!(store
            .snapshot()
            .lookup(
                &PolicyTargetKey {
                    target_kind: PolicyTargetKind::Service,
                    namespace: "tenant".to_string(),
                    name: "other".to_string(),
                },
                None
            )
            .is_invalid());
    }

    #[test]
    fn invalid_exact_section_does_not_fall_back_to_whole_target() {
        let store = BackendTLSPolicyStore::new();
        let mut invalid_exact = policy(
            "bad-https",
            PolicyTargetKind::Service,
            "shared",
            Some("https"),
            Some(ResolvedPolicyTargetClass::Service),
        );
        Arc::make_mut(&mut invalid_exact).spec.validation.hostname = "not a hostname".to_string();
        store.replace_all(HashMap::from([
            ("tenant/bad-https".to_string(), invalid_exact),
            (
                "tenant/whole".to_string(),
                policy(
                    "whole",
                    PolicyTargetKind::Service,
                    "shared",
                    None,
                    Some(ResolvedPolicyTargetClass::Service),
                ),
            ),
        ]));
        let snapshot = store.snapshot();
        assert!(snapshot
            .lookup(&target(PolicyTargetKind::Service), Some("https"))
            .is_invalid());
        assert_eq!(
            effective_name(snapshot.lookup(&target(PolicyTargetKind::Service), Some("grpc"))).as_deref(),
            Some("whole")
        );
    }

    #[test]
    fn valid_exact_section_skips_older_invalid_exact_section() {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

        let store = BackendTLSPolicyStore::new();
        let mut older_invalid = policy(
            "older",
            PolicyTargetKind::Service,
            "shared",
            Some("https"),
            Some(ResolvedPolicyTargetClass::Service),
        );
        Arc::make_mut(&mut older_invalid).metadata.creation_timestamp =
            Some(Time("2026-01-01T00:00:00Z".parse().unwrap()));
        Arc::make_mut(&mut older_invalid).spec.validation.hostname = "bad host".to_string();
        let mut newer_valid = policy(
            "newer",
            PolicyTargetKind::Service,
            "shared",
            Some("https"),
            Some(ResolvedPolicyTargetClass::Service),
        );
        Arc::make_mut(&mut newer_valid).metadata.creation_timestamp =
            Some(Time("2026-02-01T00:00:00Z".parse().unwrap()));
        store.replace_all(HashMap::from([
            ("tenant/older".to_string(), older_invalid),
            ("tenant/newer".to_string(), newer_valid),
        ]));
        assert_eq!(
            effective_name(
                store
                    .snapshot()
                    .lookup(&target(PolicyTargetKind::Service), Some("https"))
            )
            .as_deref(),
            Some("newer")
        );
    }

    #[test]
    fn incomplete_custom_ca_is_invalid() {
        let store = BackendTLSPolicyStore::new();
        let mut custom = policy(
            "custom",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        );
        Arc::make_mut(&mut custom).spec.use_system_ca = false;
        Arc::make_mut(&mut custom).spec.validation.ca_certificate_refs = Some(vec![BackendTLSPolicyCACertificateRef {
            group: String::new(),
            kind: "ConfigMap".to_string(),
            name: "ca".to_string(),
            namespace: None,
        }]);
        store.replace_all(HashMap::from([("tenant/custom".to_string(), custom)]));
        assert!(store
            .snapshot()
            .lookup(&target(PolicyTargetKind::Service), None)
            .is_invalid());
    }

    #[test]
    fn profile_fingerprint_changes_with_ca_bytes_and_sans() {
        let store = BackendTLSPolicyStore::new();
        let mut first = policy(
            "tls",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        );
        Arc::make_mut(&mut first).spec.use_system_ca = false;
        Arc::make_mut(&mut first).spec.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
            data: Some(std::collections::BTreeMap::from([(
                CA_CERT.to_string(),
                ByteString(b"ca-a".to_vec()),
            )])),
            ..Default::default()
        }]);
        store.replace_all(HashMap::from([("tenant/tls".to_string(), first)]));
        let first_fp = store
            .snapshot()
            .lookup(&target(PolicyTargetKind::Service), None)
            .effective()
            .unwrap()
            .profile
            .fingerprint;

        let mut second = policy(
            "tls",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        );
        Arc::make_mut(&mut second).spec.use_system_ca = false;
        Arc::make_mut(&mut second).spec.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
            data: Some(std::collections::BTreeMap::from([(
                CA_CERT.to_string(),
                ByteString(b"ca-b".to_vec()),
            )])),
            ..Default::default()
        }]);
        store.replace_all(HashMap::from([("tenant/tls".to_string(), second)]));
        let second_fp = store
            .snapshot()
            .lookup(&target(PolicyTargetKind::Service), None)
            .effective()
            .unwrap()
            .profile
            .fingerprint;
        assert_ne!(first_fp, second_fp);

        let mut with_san = policy(
            "tls",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        );
        Arc::make_mut(&mut with_san).spec.use_system_ca = false;
        Arc::make_mut(&mut with_san).spec.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
            data: Some(std::collections::BTreeMap::from([(
                CA_CERT.to_string(),
                ByteString(b"ca-b".to_vec()),
            )])),
            ..Default::default()
        }]);
        Arc::make_mut(&mut with_san).spec.validation.subject_alt_names = Some(vec![SubjectAltName {
            san_type: SubjectAltNameType::Hostname,
            hostname: Some("alt.example.com".to_string()),
            uri: None,
        }]);
        store.replace_all(HashMap::from([("tenant/tls".to_string(), with_san)]));
        let san_fp = store
            .snapshot()
            .lookup(&target(PolicyTargetKind::Service), None)
            .effective()
            .unwrap()
            .profile
            .fingerprint;
        assert_ne!(second_fp, san_fp);
    }

    #[test]
    fn profile_fingerprint_is_stable_for_identical_inputs() {
        let store = BackendTLSPolicyStore::new();
        let first = policy(
            "tls",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        );
        store.replace_all(HashMap::from([("tenant/tls".to_string(), Arc::clone(&first))]));
        let first_fp = store
            .snapshot()
            .lookup(&target(PolicyTargetKind::Service), None)
            .effective()
            .unwrap()
            .profile
            .fingerprint;

        let second = policy(
            "tls",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        );
        store.replace_all(HashMap::from([("tenant/tls".to_string(), second)]));
        let second_fp = store
            .snapshot()
            .lookup(&target(PolicyTargetKind::Service), None)
            .effective()
            .unwrap()
            .profile
            .fingerprint;
        assert_eq!(first_fp, second_fp);
        assert_eq!(
            BackendTlsProfileIdentity { fingerprint: first_fp }.group_key(),
            BackendTlsProfileIdentity { fingerprint: second_fp }.group_key()
        );
    }

    #[test]
    fn full_and_partial_publication_produce_the_same_lookup() {
        let full = BackendTLSPolicyStore::new();
        let partial = BackendTLSPolicyStore::new();
        let https = policy(
            "https-a",
            PolicyTargetKind::Service,
            "shared",
            Some("https"),
            Some(ResolvedPolicyTargetClass::Service),
        );
        let whole = policy(
            "whole-a",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        );
        full.replace_all(HashMap::from([
            ("tenant/https-a".to_string(), Arc::clone(&https)),
            ("tenant/whole-a".to_string(), Arc::clone(&whole)),
        ]));
        partial.update(HashMap::from([("tenant/https-a".to_string(), https)]), &HashSet::new());
        partial.update(HashMap::from([("tenant/whole-a".to_string(), whole)]), &HashSet::new());

        let full_snap = full.snapshot();
        let partial_snap = partial.snapshot();
        let target = target(PolicyTargetKind::Service);
        assert_eq!(
            effective_name(full_snap.lookup(&target, Some("https"))).as_deref(),
            effective_name(partial_snap.lookup(&target, Some("https"))).as_deref()
        );
        assert_eq!(
            effective_name(full_snap.lookup(&target, Some("grpc"))).as_deref(),
            effective_name(partial_snap.lookup(&target, Some("grpc"))).as_deref()
        );
        assert_eq!(
            full_snap
                .lookup(&target, Some("https"))
                .effective()
                .unwrap()
                .profile
                .fingerprint,
            partial_snap
                .lookup(&target, Some("https"))
                .effective()
                .unwrap()
                .profile
                .fingerprint
        );
    }

    #[test]
    fn missing_timestamp_loses_and_uid_change_replaces_arc() {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

        let store = BackendTLSPolicyStore::new();
        let mut missing = policy(
            "missing",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        );
        Arc::make_mut(&mut missing).metadata.creation_timestamp = None;
        let mut dated = policy(
            "dated",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        );
        Arc::make_mut(&mut dated).metadata.creation_timestamp = Some(Time("2026-01-01T00:00:00Z".parse().unwrap()));
        store.replace_all(HashMap::from([
            ("tenant/missing".to_string(), missing),
            ("tenant/dated".to_string(), dated),
        ]));
        assert_eq!(
            effective_name(store.snapshot().lookup(&target(PolicyTargetKind::Service), None)).as_deref(),
            Some("dated")
        );

        let first = store.snapshot().get("tenant/dated").unwrap();
        let mut recreated = (*first).clone();
        recreated.metadata.uid = Some("replacement-uid".to_string());
        store.replace_all(HashMap::from([("tenant/dated".to_string(), Arc::new(recreated))]));
        let second = store.snapshot().get("tenant/dated").unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
    }
}
