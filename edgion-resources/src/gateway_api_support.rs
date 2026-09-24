//! Code-owned Gateway API identity inventory. This does not claim bundle or feature support.

pub const GROUP: &str = "gateway.networking.k8s.io";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceState {
    /// The canonical typed resource is used throughout the Controller and transport.
    Canonical,
    /// Source adapter paths are wired; bundle and conformance evidence is incomplete.
    ImplementedUnverified,
    /// An alternate source path verified across declared ingress and transport.
    SupportedAlternate,
    /// A candidate with no implemented Edgion source adapter.
    Proposed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayApiSource {
    pub kind: &'static str,
    pub version: &'static str,
    pub state: SourceState,
    pub paths: SourcePaths,
    pub evidence: SourceEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceEvidence {
    /// Standard-channel bundle versions tested for this exact source GVK.
    pub standard_bundles: &'static [&'static str],
    /// Experimental-channel bundle versions tested for this exact source GVK.
    pub experimental_bundles: &'static [&'static str],
    /// Suite version tested for this exact source GVK, not a profile-wide run.
    pub conformance_suite: Option<&'static str>,
    /// Feature evidence is tracked separately from source GVK identity.
    pub feature_evidence: &'static [&'static str],
}

const NO_SOURCE_EVIDENCE: SourceEvidence = SourceEvidence {
    standard_bundles: &[],
    experimental_bundles: &[],
    conformance_suite: None,
    feature_evidence: &[],
};

/// The declared feature list has one canonical owner; source rows do not copy it.
pub fn declared_supported_features() -> &'static [&'static str] {
    crate::resources::gateway_class::SUPPORTED_FEATURES
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathState {
    Wired,
    Partial,
    Missing,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePaths {
    pub kubernetes_watch: PathState,
    pub kubernetes_crud: PathState,
    pub status_transport: PathState,
    pub raw_type_meta_guard: PathState,
    pub converter: PathState,
}

const CANONICAL: SourcePaths = SourcePaths {
    kubernetes_watch: PathState::Wired,
    kubernetes_crud: PathState::Wired,
    status_transport: PathState::Wired,
    raw_type_meta_guard: PathState::Wired,
    converter: PathState::NotApplicable,
};
const CANONICAL_NO_STATUS: SourcePaths = SourcePaths {
    status_transport: PathState::NotApplicable,
    ..CANONICAL
};
const BETA_ADAPTER: SourcePaths = SourcePaths {
    // Both watcher transports enter one canonical processor and raw store.
    kubernetes_watch: PathState::Wired,
    kubernetes_crud: PathState::Wired,
    status_transport: PathState::NotApplicable,
    raw_type_meta_guard: PathState::Wired,
    converter: PathState::Wired,
};
const BETA_STATUS_ADAPTER: SourcePaths = SourcePaths {
    status_transport: PathState::Wired,
    ..BETA_ADAPTER
};
const ALPHA_ADAPTER: SourcePaths = SourcePaths {
    kubernetes_watch: PathState::Wired,
    kubernetes_crud: PathState::Wired,
    status_transport: PathState::Wired,
    raw_type_meta_guard: PathState::Wired,
    converter: PathState::Wired,
};
/// Source GVKs are listed explicitly. `Proposed` and `ImplementedUnverified` are
/// deliberately not advertised as conformance-verified source versions.
pub const SOURCES: &[GatewayApiSource] = &[
    GatewayApiSource {
        kind: "GatewayClass",
        version: "v1",
        state: SourceState::Canonical,
        paths: CANONICAL,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "GatewayClass",
        version: "v1beta1",
        state: SourceState::ImplementedUnverified,
        paths: BETA_STATUS_ADAPTER,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "Gateway",
        version: "v1",
        state: SourceState::Canonical,
        paths: CANONICAL,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "Gateway",
        version: "v1beta1",
        state: SourceState::ImplementedUnverified,
        paths: BETA_STATUS_ADAPTER,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "HTTPRoute",
        version: "v1",
        state: SourceState::Canonical,
        paths: CANONICAL,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "HTTPRoute",
        version: "v1beta1",
        state: SourceState::ImplementedUnverified,
        paths: BETA_STATUS_ADAPTER,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "GRPCRoute",
        version: "v1",
        state: SourceState::Canonical,
        paths: CANONICAL,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "GRPCRoute",
        version: "v1alpha2",
        state: SourceState::ImplementedUnverified,
        paths: ALPHA_ADAPTER,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "TLSRoute",
        version: "v1",
        state: SourceState::Canonical,
        paths: CANONICAL,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "TLSRoute",
        version: "v1alpha3",
        state: SourceState::ImplementedUnverified,
        paths: ALPHA_ADAPTER,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "ReferenceGrant",
        version: "v1",
        state: SourceState::Canonical,
        paths: CANONICAL_NO_STATUS,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "ReferenceGrant",
        version: "v1beta1",
        state: SourceState::ImplementedUnverified,
        paths: BETA_ADAPTER,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "TCPRoute",
        version: "v1",
        state: SourceState::Canonical,
        paths: CANONICAL,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "TCPRoute",
        version: "v1alpha2",
        state: SourceState::ImplementedUnverified,
        paths: ALPHA_ADAPTER,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "UDPRoute",
        version: "v1",
        state: SourceState::Canonical,
        paths: CANONICAL,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "UDPRoute",
        version: "v1alpha2",
        state: SourceState::ImplementedUnverified,
        paths: ALPHA_ADAPTER,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "BackendTLSPolicy",
        version: "v1",
        state: SourceState::Canonical,
        paths: CANONICAL,
        evidence: NO_SOURCE_EVIDENCE,
    },
    GatewayApiSource {
        kind: "BackendTLSPolicy",
        version: "v1alpha3",
        state: SourceState::ImplementedUnverified,
        paths: ALPHA_ADAPTER,
        evidence: NO_SOURCE_EVIDENCE,
    },
];

pub fn source_for(kind: &str, version: &str) -> Option<&'static GatewayApiSource> {
    SOURCES
        .iter()
        .find(|source| source.kind == kind && source.version == version && source.state != SourceState::Proposed)
}

pub fn canonical_sources() -> impl Iterator<Item = &'static GatewayApiSource> {
    SOURCES.iter().filter(|source| source.state == SourceState::Canonical)
}

pub fn complete_alternate_sources() -> impl Iterator<Item = &'static GatewayApiSource> {
    SOURCES
        .iter()
        .filter(|source| source.state == SourceState::SupportedAlternate)
}

/// Assert that a production watcher or status writer uses its inventoried
/// canonical resource identity. Partial source adapters are checked separately.
pub fn assert_canonical_type_identity<K: kube::Resource>(kind: &str)
where
    K::DynamicType: Default,
{
    let dt = K::DynamicType::default();
    if canonical_sources().any(|source| source.kind == kind) {
        let source = SOURCES
            .iter()
            .find(|source| source.kind == kind && source.version == K::version(&dt))
            .expect("Gateway API watcher/status version is absent from source inventory");
        assert_eq!(K::group(&dt), GROUP);
        assert_eq!(K::kind(&dt), source.kind);
        assert!(matches!(
            source.state,
            SourceState::Canonical | SourceState::ImplementedUnverified
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::*;
    use kube::Resource;
    use std::collections::HashSet;

    fn typed_identity<T: Resource<DynamicType = ()>>() -> (String, String, String) {
        (
            T::group(&()).to_string(),
            T::version(&()).to_string(),
            T::kind(&()).to_string(),
        )
    }

    #[test]
    fn canonical_inventory_matches_the_actual_rust_resource_types() {
        let types = [
            typed_identity::<GatewayClass>(),
            typed_identity::<Gateway>(),
            typed_identity::<HTTPRoute>(),
            typed_identity::<GRPCRoute>(),
            typed_identity::<TLSRoute>(),
            typed_identity::<ReferenceGrant>(),
            typed_identity::<TCPRoute>(),
            typed_identity::<UDPRoute>(),
            typed_identity::<BackendTLSPolicy>(),
        ];
        let inventory: HashSet<_> = canonical_sources()
            .map(|source| (GROUP.to_string(), source.version.to_string(), source.kind.to_string()))
            .collect();
        assert_eq!(inventory.len(), types.len());
        assert_eq!(inventory, HashSet::from(types));
    }

    #[test]
    fn alternate_sources_are_not_advertised_as_complete() {
        assert!(SOURCES
            .iter()
            .filter(|source| source.version == "v1beta1")
            .all(|source| source.state != SourceState::Canonical));
        let beta = SOURCES
            .iter()
            .find(|source| source.kind == "ReferenceGrant" && source.version == "v1beta1")
            .unwrap();
        assert_eq!(ReferenceGrantBeta::version(&()), beta.version);
        assert_eq!(ReferenceGrantBeta::kind(&()), "ReferenceGrant");
        assert_eq!(beta.paths.kubernetes_watch, PathState::Wired);
        assert_eq!(beta.paths.kubernetes_crud, PathState::Wired);
        assert_eq!(beta.paths.status_transport, PathState::NotApplicable);
        assert_eq!(beta.paths.raw_type_meta_guard, PathState::Wired);
        assert_eq!(beta.paths.converter, PathState::Wired);
        let route = SOURCES
            .iter()
            .find(|source| source.kind == "HTTPRoute" && source.version == "v1beta1")
            .unwrap();
        assert_eq!(HTTPRouteBeta::version(&()), route.version);
        assert_eq!(route.state, SourceState::ImplementedUnverified);
        assert_eq!(route.paths.status_transport, PathState::Wired);
    }

    #[test]
    fn reference_grant_beta_conversion_preserves_identity_and_rules() {
        let beta: ReferenceGrantBeta = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1",
            "kind": "ReferenceGrant",
            "metadata": {"name":"allow", "namespace":"source", "resourceVersion":"17"},
            "spec": {
                "from": [{"group":"gateway.networking.k8s.io", "kind":"HTTPRoute", "namespace":"client"}],
                "to": [{"group":"", "kind":"Service", "name":"backend"}]
            }
        }))
        .unwrap();
        let expected_metadata = beta.metadata.clone();
        let expected_from = beta.spec.from.clone();
        let expected_to = beta.spec.to.clone();
        let canonical: ReferenceGrant = beta.into();
        assert_eq!(canonical.metadata, expected_metadata);
        assert_eq!(canonical.spec.from, expected_from);
        assert_eq!(canonical.spec.to, expected_to);
    }

    #[test]
    fn inventory_cannot_promote_an_incomplete_source_path() {
        let mut identities = HashSet::new();
        for source in SOURCES {
            assert!(identities.insert((source.kind, source.version)));
            if source.state == SourceState::Canonical {
                assert_eq!(source.paths.kubernetes_watch, PathState::Wired);
                assert_eq!(source.paths.kubernetes_crud, PathState::Wired);
                assert_eq!(source.paths.raw_type_meta_guard, PathState::Wired);
            } else if source.state == SourceState::SupportedAlternate {
                assert_eq!(source.paths.kubernetes_watch, PathState::Wired);
                assert_eq!(source.paths.kubernetes_crud, PathState::Wired);
                assert!(matches!(
                    source.paths.status_transport,
                    PathState::Wired | PathState::NotApplicable
                ));
                assert_eq!(source.paths.raw_type_meta_guard, PathState::Wired);
                assert_eq!(source.paths.converter, PathState::Wired);
                assert!(
                    !source.evidence.standard_bundles.is_empty() || !source.evidence.experimental_bundles.is_empty()
                );
                assert!(source.evidence.conformance_suite.is_some());
            } else {
                assert!(source.evidence.conformance_suite.is_none());
            }
        }
        assert_eq!(
            declared_supported_features(),
            crate::resources::gateway_class::SUPPORTED_FEATURES
        );
        assert_eq!(complete_alternate_sources().count(), 0);
    }
}
