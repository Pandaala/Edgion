//! Typed immutable runtime store for `BackendTLSPolicy` resources.

use arc_swap::ArcSwap;
use edgion_resources::resources::edgion_backend_traffic_policy::precedence_key;
use edgion_resources::resources::edgion_backend_traffic_policy::{
    PolicyTargetKey, PolicyTargetKind, ResolvedPolicyTargetClass,
};
use edgion_resources::resources::BackendTLSPolicy;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

static GLOBAL_BACKEND_TLS_POLICY_STORE: LazyLock<Arc<BackendTLSPolicyStore>> =
    LazyLock::new(|| Arc::new(BackendTLSPolicyStore::new()));

pub fn get_global_backend_tls_policy_store() -> Arc<BackendTLSPolicyStore> {
    GLOBAL_BACKEND_TLS_POLICY_STORE.clone()
}

type BackendTLSPolicyMap = HashMap<String, Arc<BackendTLSPolicy>>;
type ReverseIndexMap = HashMap<PolicyTargetKey, Vec<Arc<BackendTLSPolicy>>>;

struct BackendTLSPolicySnapshotData {
    generation: u64,
    policies: BackendTLSPolicyMap,
    reverse_index: ReverseIndexMap,
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
    pub fn get_policies_for_target(&self, target: &PolicyTargetKey) -> Vec<Arc<BackendTLSPolicy>> {
        self.inner.reverse_index.get(target).cloned().unwrap_or_default()
    }

    /// Whether any synced policy directly names this typed target, including a
    /// present-but-ineligible policy omitted from the effective reverse index.
    pub fn has_direct_candidate(&self, target: &PolicyTargetKey) -> bool {
        self.inner.policies.values().any(|policy| {
            let namespace = policy.metadata.namespace.as_deref().unwrap_or_default();
            policy
                .spec
                .target_refs
                .iter()
                .any(|target_ref| target_ref.canonical_key(namespace).as_ref() == Ok(target))
        })
    }

    /// Oldest-wins within the most specific applicable attachment bucket.
    /// An unrelated section is never used as a fallback.
    pub fn get_policy(&self, target: &PolicyTargetKey, section: Option<&str>) -> Option<Arc<BackendTLSPolicy>> {
        let candidates = self.inner.reverse_index.get(target)?;
        if let Some(section) = section {
            if let Some(policy) = candidates.iter().find(|policy| {
                policy.spec.target_refs.iter().any(|target_ref| {
                    target_ref.canonical_key(&target.namespace).as_ref() == Ok(target)
                        && target_ref.section_name.as_deref() == Some(section)
                })
            }) {
                return Some(Arc::clone(policy));
            }
        }
        candidates
            .iter()
            .find(|policy| {
                policy.spec.target_refs.iter().any(|target_ref| {
                    target_ref.canonical_key(&target.namespace).as_ref() == Ok(target)
                        && target_ref.section_name.is_none()
                })
            })
            .cloned()
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
                reverse_index: HashMap::new(),
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

    /// Controller eligibility marker plus a defensive target-kind check.
    fn policy_target_keys(&self, policy: &BackendTLSPolicy) -> Vec<PolicyTargetKey> {
        let Some(resolved_class) = policy.spec.resolved_target_class else {
            return Vec::new();
        };
        if policy.spec.target_refs.len() != 1 {
            return Vec::new();
        }
        let namespace = policy.namespace().unwrap_or("");
        let keys: Vec<_> = policy
            .spec
            .target_refs
            .iter()
            .filter_map(|target| target.canonical_key(namespace).ok())
            .collect();
        let all_refs_valid = keys.len() == policy.spec.target_refs.len();
        let class_matches = keys.iter().all(|key| {
            matches!(
                (resolved_class, key.target_kind),
                (ResolvedPolicyTargetClass::Service, PolicyTargetKind::Service)
                    | (ResolvedPolicyTargetClass::Ai, PolicyTargetKind::EdgionBackend)
                    | (ResolvedPolicyTargetClass::A2aService, PolicyTargetKind::EdgionBackend)
                    | (ResolvedPolicyTargetClass::A2aExternal, PolicyTargetKind::EdgionBackend)
            )
        });
        let section_supported = policy.spec.target_refs.iter().all(|target| {
            target.section_name.is_none()
                || keys
                    .first()
                    .is_some_and(|key| key.target_kind == PolicyTargetKind::Service)
        });
        (all_refs_valid && class_matches && section_supported)
            .then_some(keys)
            .unwrap_or_default()
    }

    fn sort_policies_by_precedence(policies: &mut [Arc<BackendTLSPolicy>]) {
        policies.sort_by_key(|policy| precedence_key(&policy.metadata));
    }

    fn build_reverse_index(&self, policies: &BackendTLSPolicyMap) -> ReverseIndexMap {
        let mut index: ReverseIndexMap = HashMap::new();
        for policy in policies.values() {
            for target in self.policy_target_keys(policy) {
                index.entry(target).or_default().push(Arc::clone(policy));
            }
        }
        for candidates in index.values_mut() {
            Self::sort_policies_by_precedence(candidates);
        }
        index
    }

    fn reverse_index_equivalent(left: &ReverseIndexMap, right: &ReverseIndexMap) -> bool {
        left.len() == right.len()
            && right.iter().all(|(target, policies)| {
                left.get(target).is_some_and(|existing| {
                    existing.len() == policies.len()
                        && existing
                            .iter()
                            .zip(policies)
                            .all(|(left, right)| Arc::ptr_eq(left, right))
                })
            })
    }

    fn publish(&self, incoming: BackendTLSPolicyMap) {
        // Config sync serializes ConfHandler writes for this resource kind.
        // Keep this load-build-store path single-writer unless publication is
        // changed to provide its own writer synchronization.
        let current = self.snapshot.load_full();
        let policies = Self::retain_equivalent_arcs(&current.policies, incoming);
        let reverse_index = self.build_reverse_index(&policies);
        let unchanged = policies.len() == current.policies.len()
            && policies
                .iter()
                .all(|(key, policy)| current.policies.get(key).is_some_and(|old| Arc::ptr_eq(old, policy)))
            && Self::reverse_index_equivalent(&current.reverse_index, &reverse_index);
        if unchanged {
            return;
        }
        self.snapshot.store(Arc::new(BackendTLSPolicySnapshotData {
            generation: current
                .generation
                .checked_add(1)
                .expect("BackendTLSPolicy snapshot generation exhausted"),
            policies,
            reverse_index,
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

    pub fn get_policies_for_target(&self, target: &PolicyTargetKey) -> Vec<Arc<BackendTLSPolicy>> {
        self.snapshot().get_policies_for_target(target)
    }

    pub fn count(&self) -> usize {
        self.snapshot.load().policies.len()
    }

    pub fn stats(&self) -> BackendTLSPolicyStoreStats {
        let snapshot = self.snapshot.load();
        BackendTLSPolicyStoreStats {
            policies: snapshot.policies.len(),
            reverse_index_targets: snapshot.reverse_index.len(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackendTLSPolicyStoreStats {
    pub policies: usize,
    pub reverse_index_targets: usize,
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
        BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

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
        assert!(snapshot.get_policy(&target(PolicyTargetKind::Service), None).is_none());
        assert_eq!(
            snapshot
                .get_policy(&target(PolicyTargetKind::Service), Some("grpc"))
                .unwrap()
                .name(),
            "service-grpc"
        );
        assert_eq!(
            snapshot
                .get_policy(&target(PolicyTargetKind::EdgionBackend), None)
                .unwrap()
                .name(),
            "backend"
        );
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
        let first_policy = first.get_policy(&target(PolicyTargetKind::Service), None).unwrap();

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
            &equivalent.get_policy(&target(PolicyTargetKind::Service), None).unwrap()
        ));

        store.replace_all(HashMap::new());
        assert_eq!(store.snapshot().generation(), first.generation() + 1);
        assert_eq!(first_policy.name(), "policy", "old Arc remains usable");
    }

    #[test]
    fn absent_or_mismatched_marker_is_not_eligible() {
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
            .get_policy(&target(PolicyTargetKind::Service), None)
            .is_none());
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
            store
                .snapshot()
                .get_policy(&target(PolicyTargetKind::Service), None)
                .unwrap()
                .name(),
            "dated"
        );

        let first = store.snapshot().get("tenant/dated").unwrap();
        let mut recreated = (*first).clone();
        recreated.metadata.uid = Some("replacement-uid".to_string());
        store.replace_all(HashMap::from([("tenant/dated".to_string(), Arc::new(recreated))]));
        let second = store.snapshot().get("tenant/dated").unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
    }
}
