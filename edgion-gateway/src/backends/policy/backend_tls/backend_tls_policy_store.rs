//! Typed immutable runtime store for `BackendTLSPolicy` resources.

use arc_swap::ArcSwap;
use edgion_resources::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use edgion_resources::resources::backend_tls_policy::{
    backend_tls_identity_validation_errors, parse_client_certificate_option, WellKnownCACertificates,
    OPTION_CLIENT_CERTIFICATE_REF,
};
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
    backend_store: Arc<crate::ai::EdgionBackendStore>,
    snapshot: ArcSwap<BackendTLSPolicySnapshotData>,
}

impl BackendTLSPolicyStore {
    pub fn new() -> Self {
        Self::with_backend_store(crate::ai::get_global_edgion_backend_store())
    }

    pub(crate) fn with_backend_store(backend_store: Arc<crate::ai::EdgionBackendStore>) -> Self {
        Self {
            backend_store,
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
                    | (ResolvedPolicyTargetClass::McpService, PolicyTargetKind::EdgionBackend)
                    | (ResolvedPolicyTargetClass::McpExternal, PolicyTargetKind::EdgionBackend)
            )
        });
        let section_supported = policy.spec.target_refs.iter().all(|target| {
            target.section_name.is_none()
                || keys
                    .first()
                    .is_some_and(|key| key.target_kind == PolicyTargetKind::Service)
        });
        let mcp_runtime_supported = keys
            .first()
            .is_none_or(|key| self.mcp_runtime_supported(policy, resolved_class, key));
        (all_refs_valid && class_matches && section_supported && mcp_runtime_supported)
            .then_some(keys)
            .unwrap_or_default()
    }

    fn mcp_runtime_supported(
        &self,
        policy: &BackendTLSPolicy,
        class: ResolvedPolicyTargetClass,
        target: &PolicyTargetKey,
    ) -> bool {
        if !matches!(
            class,
            ResolvedPolicyTargetClass::McpService | ResolvedPolicyTargetClass::McpExternal
        ) {
            return true;
        }
        if !backend_tls_identity_validation_errors(&policy.spec.validation).is_empty() {
            return false;
        }
        if policy
            .spec
            .options
            .as_ref()
            .is_some_and(|options| options.keys().any(|key| key != OPTION_CLIENT_CERTIFICATE_REF))
        {
            return false;
        }
        let policy_namespace = policy.metadata.namespace.as_deref().unwrap_or_default();
        let ca_refs = policy.spec.validation.ca_certificate_refs.as_deref();
        let well_known = policy.spec.validation.well_known_ca_certificates.as_ref();
        let ca_ready = match (ca_refs, well_known) {
            (Some(_), Some(_)) => false,
            (None, Some(WellKnownCACertificates::System)) => {
                policy.spec.use_system_ca && policy.spec.resolved_ca_certificates.is_none()
            }
            (Some(refs), None) if !refs.is_empty() && !policy.spec.use_system_ca => {
                let reference_names = refs
                    .iter()
                    .map(|reference| reference.name.as_str())
                    .collect::<HashSet<_>>();
                reference_names.len() == refs.len()
                    && refs.iter().all(|reference| {
                        matches!(reference.group.as_str(), "" | "core")
                            && matches!(reference.kind.as_str(), "Secret" | "ConfigMap")
                            && reference.namespace.is_none()
                            && valid_kube_name(&reference.name)
                    })
                    && policy.spec.resolved_ca_certificates.as_ref().is_some_and(|resolved| {
                        let resolved_names = resolved
                            .iter()
                            .filter_map(|secret| secret.metadata.name.as_deref())
                            .collect::<HashSet<_>>();
                        resolved.len() == refs.len()
                            && resolved_names.len() == resolved.len()
                            && resolved_names == reference_names
                            && resolved.iter().all(|secret| {
                                secret.metadata.namespace.as_deref() == Some(policy_namespace)
                                    && secret
                                        .data
                                        .as_ref()
                                        .and_then(|data| data.get(CA_CERT))
                                        .is_some_and(|value| !value.0.is_empty())
                            })
                    })
            }
            _ => false,
        };
        let client_ready = match parse_client_certificate_option(policy.spec.options.as_ref()) {
            Ok(Some(reference)) => policy.spec.resolved_client_certificate.as_ref().is_some_and(|secret| {
                secret.metadata.name.as_deref() == Some(reference.name.as_str())
                    && secret.metadata.namespace.as_deref() == Some(policy_namespace)
                    && secret.data.as_ref().is_some_and(|data| {
                        data.get(CERT).is_some_and(|value| !value.0.is_empty())
                            && data.get(KEY).is_some_and(|value| !value.0.is_empty())
                    })
            }),
            Ok(None) => policy.spec.resolved_client_certificate.is_none(),
            Err(_) => false,
        };
        if !ca_ready || !client_ready {
            return false;
        }
        let key = format!("{}/{}", target.namespace, target.name);
        let snapshot = self.backend_store.edgion_backend_snapshot();
        let Some(runtime) = snapshot.get_mcp_by_key(&key) else {
            return false;
        };
        matches!(
            (class, &runtime.target),
            (
                ResolvedPolicyTargetClass::McpService,
                crate::ai::RuntimeMcpTarget::Service { .. }
            ) | (
                ResolvedPolicyTargetClass::McpExternal,
                crate::ai::RuntimeMcpTarget::External(crate::ai::RuntimeExternalHttpEndpoint { use_tls: true, .. })
            )
        )
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

fn valid_kube_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && label.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && label.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
        })
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
    use edgion_common::conf_sync::traits::ConfHandler;
    use edgion_resources::resources::backend_tls_policy::{
        BackendTLSPolicyCACertificateRef, BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation,
        SubjectAltName, SubjectAltNameType, WellKnownCACertificates,
    };
    use edgion_resources::resources::edgion_backend::{
        BackendServiceReference, EdgionBackend, EdgionBackendSpec, McpBackendSpec, McpBackendTarget, McpTransport,
        McpTransportType, ResolvedMcpBackend, ResolvedMcpTarget,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::{api::core::v1::Secret, ByteString};
    use std::collections::BTreeMap;

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

    fn mcp_backend(endpoint: &str) -> Arc<EdgionBackend> {
        let normalized_endpoint = endpoint.trim_end_matches('/').to_string();
        let mut backend = EdgionBackend::new(
            "shared",
            EdgionBackendSpec {
                ai: None,
                a2a: None,
                mcp: Some(McpBackendSpec {
                    target: McpBackendTarget {
                        service_ref: None,
                        endpoint: Some(endpoint.to_string()),
                        path: None,
                    },
                    allow_insecure_http: endpoint.starts_with("http://"),
                    transport: McpTransport {
                        r#type: McpTransportType::StreamableHTTP,
                    },
                    authentication: None,
                }),
                resolved_a2a: None,
                resolved_mcp: Some(ResolvedMcpBackend {
                    target: ResolvedMcpTarget::External { normalized_endpoint },
                }),
                reconcile_outcome: None,
            },
        );
        backend.metadata.namespace = Some("tenant".to_string());
        Arc::new(backend)
    }

    fn mcp_service_backend() -> Arc<EdgionBackend> {
        let mut backend = EdgionBackend::new(
            "shared",
            EdgionBackendSpec {
                ai: None,
                a2a: None,
                mcp: Some(McpBackendSpec {
                    target: McpBackendTarget {
                        service_ref: Some(BackendServiceReference {
                            name: "tools-service".to_string(),
                            namespace: Some("tenant".to_string()),
                            port: 8080,
                        }),
                        endpoint: None,
                        path: Some("/mcp".to_string()),
                    },
                    allow_insecure_http: false,
                    transport: McpTransport {
                        r#type: McpTransportType::StreamableHTTP,
                    },
                    authentication: None,
                }),
                resolved_a2a: None,
                resolved_mcp: Some(ResolvedMcpBackend {
                    target: ResolvedMcpTarget::Service {
                        namespace: "tenant".to_string(),
                        name: "tools-service".to_string(),
                        port: 8080,
                        path: "/mcp".to_string(),
                    },
                }),
                reconcile_outcome: None,
            },
        );
        backend.metadata.namespace = Some("tenant".to_string());
        Arc::new(backend)
    }

    fn mcp_tls_policy() -> Arc<BackendTLSPolicy> {
        let mut policy = policy(
            "mcp-tls",
            PolicyTargetKind::EdgionBackend,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::McpExternal),
        );
        let spec = &mut Arc::get_mut(&mut policy).expect("new policy fixture").spec;
        spec.validation.well_known_ca_certificates = Some(WellKnownCACertificates::System);
        policy
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
    fn mcp_external_tls_policy_fails_closed_against_current_runtime_scheme() {
        let backend_store = Arc::new(crate::ai::EdgionBackendStore::new());
        let store = BackendTLSPolicyStore::with_backend_store(Arc::clone(&backend_store));
        let target = target(PolicyTargetKind::EdgionBackend);

        backend_store.full_set(&HashMap::from([(
            "tenant/shared".to_string(),
            mcp_backend("https://backend.example.com/mcp"),
        )]));
        store.replace_all(HashMap::from([("tenant/mcp-tls".to_string(), mcp_tls_policy())]));
        assert!(store.snapshot().get_policy(&target, None).is_some());

        backend_store.full_set(&HashMap::from([(
            "tenant/shared".to_string(),
            mcp_backend("http://backend.example.com/mcp"),
        )]));
        store.replace_all(HashMap::from([("tenant/mcp-tls".to_string(), mcp_tls_policy())]));
        assert!(
            store.snapshot().get_policy(&target, None).is_none(),
            "an external HTTP runtime must withdraw a forged MCP TLS attachment"
        );

        backend_store.full_set(&HashMap::from([(
            "tenant/shared".to_string(),
            mcp_backend("https://backend.example.com/mcp"),
        )]));
        store.replace_all(HashMap::from([("tenant/mcp-tls".to_string(), mcp_tls_policy())]));
        assert!(
            store.snapshot().get_policy(&target, None).is_some(),
            "HTTP-to-HTTPS transition must make the current external class eligible"
        );

        backend_store.full_set(&HashMap::from([("tenant/shared".to_string(), mcp_service_backend())]));
        store.replace_all(HashMap::from([("tenant/mcp-tls".to_string(), mcp_tls_policy())]));
        assert!(
            store.snapshot().get_policy(&target, None).is_none(),
            "an external marker must not attach to an MCP Service runtime"
        );
        let mut service_tls = (*mcp_tls_policy()).clone();
        service_tls.spec.resolved_target_class = Some(ResolvedPolicyTargetClass::McpService);
        store.replace_all(HashMap::from([("tenant/mcp-tls".to_string(), Arc::new(service_tls))]));
        assert!(
            store.snapshot().get_policy(&target, None).is_some(),
            "MCP Service accepts its direct typed TLS policy"
        );

        backend_store.full_set(&HashMap::new());
        store.replace_all(HashMap::from([("tenant/mcp-tls".to_string(), mcp_tls_policy())]));
        assert!(store.snapshot().get_policy(&target, None).is_none());
    }

    #[test]
    fn mcp_tls_policy_rejects_forged_or_incomplete_resolved_material() {
        let backend_store = Arc::new(crate::ai::EdgionBackendStore::new());
        backend_store.full_set(&HashMap::from([(
            "tenant/shared".to_string(),
            mcp_backend("https://backend.example.com/mcp"),
        )]));
        let store = BackendTLSPolicyStore::with_backend_store(backend_store);
        let target = target(PolicyTargetKind::EdgionBackend);
        let publish = |store: &BackendTLSPolicyStore, policy: Arc<BackendTLSPolicy>| {
            store.replace_all(HashMap::from([("tenant/mcp-tls".to_string(), policy)]));
            store.snapshot().get_policy(&target, None).is_some()
        };

        let ca_ref = BackendTLSPolicyCACertificateRef {
            group: String::new(),
            kind: "Secret".to_string(),
            name: "backend-ca".to_string(),
            namespace: None,
        };
        let secret = |name: &str, entries: &[(&str, &[u8])]| Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some("tenant".to_string()),
                ..Default::default()
            },
            data: Some(
                entries
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), ByteString(value.to_vec())))
                    .collect::<BTreeMap<_, _>>(),
            ),
            ..Default::default()
        };

        let mut both_modes = (*mcp_tls_policy()).clone();
        both_modes.spec.validation.ca_certificate_refs = Some(vec![ca_ref.clone()]);
        assert!(!publish(&store, Arc::new(both_modes)));

        for forged_ref in [
            BackendTLSPolicyCACertificateRef {
                group: "external.example".to_string(),
                ..ca_ref.clone()
            },
            BackendTLSPolicyCACertificateRef {
                kind: "Deployment".to_string(),
                ..ca_ref.clone()
            },
            BackendTLSPolicyCACertificateRef {
                namespace: Some("other".to_string()),
                ..ca_ref.clone()
            },
            BackendTLSPolicyCACertificateRef {
                name: "Invalid_Name".to_string(),
                ..ca_ref.clone()
            },
        ] {
            let mut forged = (*mcp_tls_policy()).clone();
            forged.spec.validation.well_known_ca_certificates = None;
            forged.spec.use_system_ca = false;
            forged.spec.validation.ca_certificate_refs = Some(vec![forged_ref]);
            forged.spec.resolved_ca_certificates = Some(vec![secret("backend-ca", &[(CA_CERT, b"ca")])]);
            assert!(!publish(&store, Arc::new(forged)));
        }

        let mut ca_policy = (*mcp_tls_policy()).clone();
        ca_policy.spec.validation.well_known_ca_certificates = None;
        ca_policy.spec.use_system_ca = false;
        ca_policy.spec.validation.ca_certificate_refs = Some(vec![ca_ref.clone()]);
        for resolved in [
            None,
            Some(vec![secret("other-ca", &[(CA_CERT, b"ca")])]),
            Some(vec![secret("backend-ca", &[(CA_CERT, b"")])]),
        ] {
            ca_policy.spec.resolved_ca_certificates = resolved;
            assert!(!publish(&store, Arc::new(ca_policy.clone())));
        }
        ca_policy.spec.resolved_ca_certificates = Some(vec![secret("backend-ca", &[(CA_CERT, b"ca")])]);
        assert!(publish(&store, Arc::new(ca_policy)));

        let mut duplicate_ref_with_extra_ca = (*mcp_tls_policy()).clone();
        duplicate_ref_with_extra_ca.spec.validation.well_known_ca_certificates = None;
        duplicate_ref_with_extra_ca.spec.use_system_ca = false;
        duplicate_ref_with_extra_ca.spec.validation.ca_certificate_refs = Some(vec![ca_ref.clone(), ca_ref.clone()]);
        duplicate_ref_with_extra_ca.spec.resolved_ca_certificates = Some(vec![
            secret("backend-ca", &[(CA_CERT, b"ca")]),
            secret("extra-ca", &[(CA_CERT, b"extra")]),
        ]);
        assert!(!publish(&store, Arc::new(duplicate_ref_with_extra_ca)));

        let mut wrong_ca_namespace = (*mcp_tls_policy()).clone();
        wrong_ca_namespace.spec.validation.well_known_ca_certificates = None;
        wrong_ca_namespace.spec.use_system_ca = false;
        wrong_ca_namespace.spec.validation.ca_certificate_refs = Some(vec![BackendTLSPolicyCACertificateRef {
            group: String::new(),
            kind: "Secret".to_string(),
            name: "backend-ca".to_string(),
            namespace: None,
        }]);
        let mut wrong_namespace_secret = secret("backend-ca", &[(CA_CERT, b"ca")]);
        wrong_namespace_secret.metadata.namespace = Some("other".to_string());
        wrong_ca_namespace.spec.resolved_ca_certificates = Some(vec![wrong_namespace_secret]);
        assert!(!publish(&store, Arc::new(wrong_ca_namespace)));

        let mut client_policy = (*mcp_tls_policy()).clone();
        client_policy.spec.options = Some(HashMap::from([(
            OPTION_CLIENT_CERTIFICATE_REF.to_string(),
            "client-cert".to_string(),
        )]));
        for resolved in [
            None,
            Some(secret("other-cert", &[(CERT, b"cert"), (KEY, b"key")])),
            Some(secret("client-cert", &[(CERT, b"cert")])),
            Some(secret("client-cert", &[(CERT, b""), (KEY, b"key")])),
        ] {
            client_policy.spec.resolved_client_certificate = resolved;
            assert!(!publish(&store, Arc::new(client_policy.clone())));
        }
        client_policy.spec.resolved_client_certificate = Some(secret("client-cert", &[(CERT, b"cert"), (KEY, b"key")]));
        assert!(publish(&store, Arc::new(client_policy)));

        let mut wrong_client_namespace = (*mcp_tls_policy()).clone();
        wrong_client_namespace.spec.options = Some(HashMap::from([(
            OPTION_CLIENT_CERTIFICATE_REF.to_string(),
            "client-cert".to_string(),
        )]));
        let mut wrong_namespace_secret = secret("client-cert", &[(CERT, b"cert"), (KEY, b"key")]);
        wrong_namespace_secret.metadata.namespace = Some("other".to_string());
        wrong_client_namespace.spec.resolved_client_certificate = Some(wrong_namespace_secret);
        assert!(!publish(&store, Arc::new(wrong_client_namespace)));

        for (options, resolved_client_certificate) in [
            (
                HashMap::from([("example.io/unknown".to_string(), "value".to_string())]),
                None,
            ),
            (
                HashMap::from([(OPTION_CLIENT_CERTIFICATE_REF.to_string(), String::new())]),
                None,
            ),
            (
                HashMap::from([(OPTION_CLIENT_CERTIFICATE_REF.to_string(), "   ".to_string())]),
                None,
            ),
            (
                HashMap::from([(OPTION_CLIENT_CERTIFICATE_REF.to_string(), "Invalid_Name".to_string())]),
                Some(secret("Invalid_Name", &[(CERT, b"cert"), (KEY, b"key")])),
            ),
        ] {
            let mut forged = (*mcp_tls_policy()).clone();
            forged.spec.options = Some(options);
            forged.spec.resolved_client_certificate = resolved_client_certificate;
            assert!(!publish(&store, Arc::new(forged)));
        }

        let mut valid_sans = (*mcp_tls_policy()).clone();
        valid_sans.spec.validation.subject_alt_names = Some(vec![
            SubjectAltName {
                san_type: SubjectAltNameType::Hostname,
                hostname: Some("*.example.com".to_string()),
                uri: None,
            },
            SubjectAltName {
                san_type: SubjectAltNameType::URI,
                hostname: None,
                uri: Some("spiffe://trust.example/workload".to_string()),
            },
        ]);
        assert!(publish(&store, Arc::new(valid_sans)));

        for (hostname, sans) in [
            ("", None),
            ("backend.example.com", Some(Vec::new())),
            (
                "backend.example.com",
                Some(vec![SubjectAltName {
                    san_type: SubjectAltNameType::Hostname,
                    hostname: None,
                    uri: Some("spiffe://trust.example/workload".to_string()),
                }]),
            ),
            (
                "backend.example.com",
                Some(vec![SubjectAltName {
                    san_type: SubjectAltNameType::URI,
                    hostname: Some("backend.example.com".to_string()),
                    uri: Some("spiffe://trust.example/workload".to_string()),
                }]),
            ),
            (
                "backend.example.com",
                Some(vec![SubjectAltName {
                    san_type: SubjectAltNameType::URI,
                    hostname: None,
                    uri: Some(format!("https://example.com/{}", "a".repeat(240))),
                }]),
            ),
        ] {
            let mut forged = (*mcp_tls_policy()).clone();
            forged.spec.validation.hostname = hostname.to_string();
            forged.spec.validation.subject_alt_names = sans;
            assert!(!publish(&store, Arc::new(forged)));
        }
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
