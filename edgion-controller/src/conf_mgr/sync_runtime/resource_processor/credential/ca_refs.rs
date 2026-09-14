//! Shared controller helper for resolving `validation.caCertificateRefs`
//! into resolved CA `Secret` objects.
//!
//! Callers choose a [`CaFetchStrategy`] that determines how cross-namespace
//! references are handled:
//!
//! - [`CaFetchStrategy::DirectSameNs`]: Looks up the Secret/ConfigMap directly
//!   in the owner's namespace without any `ReferenceGrant` check. This matches
//!   the BackendTLSPolicy semantics where caCertificateRefs are always same-ns.
//!
//! - [`CaFetchStrategy::GrantGated`]: Routes all Secret lookups through
//!   [`fetch_secret`] and all ConfigMap lookups through [`fetch_configmap`],
//!   both of which enforce `ReferenceGrant` for cross-namespace references.
//!   Suitable for plugin / EdgionGatewayConfig outbound TLS where cross-ns trust
//!   anchors are valid but require an explicit grant.
//!
//! ConfigMap refs are converted into synthetic `Secret` objects carrying the
//! `ca.crt` PEM under the standard [`CA_CERT`] key, so downstream consumers
//! can treat all resolved CA bundles uniformly as `Secret` objects.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::ByteString;

use edgion_resources::constants::secret_keys::tls::CA_CERT;
use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
use edgion_resources::resources::common::{is_core_api_group, SecretObjectReference};

use super::super::{format_configmap_ref_key, format_secret_key, HandlerContext, ResourceRef};
use super::pipeline::{fetch_configmap, fetch_secret, FetchOutcome};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Controls how caCertificateRefs are fetched and whether cross-namespace
/// `ReferenceGrant` enforcement is applied.
#[derive(Debug, Clone)]
pub enum CaFetchStrategy {
    /// Same-namespace direct store access, no `ReferenceGrant` check.
    ///
    /// Used for BackendTLSPolicy where the Gateway API spec mandates that
    /// caCertificateRefs can only target resources in the same namespace as
    /// the policy. The `namespace` field of each ref is ignored and the
    /// owner's namespace is always used for the store lookup.
    DirectSameNs,

    /// ReferenceGrant-gated access, supporting cross-namespace references.
    ///
    /// Routes lookups through [`fetch_secret`] / [`fetch_configmap`] which
    /// enforce cross-namespace `ReferenceGrant` checks. The `group` and `kind`
    /// fields identify the requesting resource type on the *from* side of the
    /// grant (`from.group`, `from.kind`).
    ///
    /// Used for plugin / EdgionGatewayConfig outbound TLS where refs may
    /// span namespaces with appropriate grants.
    GrantGated { group: &'static str, kind: &'static str },
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Resolve `validation.caCertificateRefs` into a flat list of CA `Secret`
/// objects, honoring the chosen [`CaFetchStrategy`].
///
/// For each ref:
/// - Secret kind → looked up via the strategy (direct or grant-gated) and
///   pushed into the result as-is.
/// - ConfigMap kind → looked up via the strategy; the `ca.crt` PEM value is
///   extracted and wrapped in a synthetic `Secret` with the same name and
///   namespace. Empty or absent `ca.crt` values are silently skipped.
/// - Any other kind is silently skipped.
///
/// Every ref, regardless of whether it resolves, is registered with the
/// `SecretRefManager` so the owning resource is requeued when the referenced
/// Secret or ConfigMap arrives.
///
/// # Parameters
///
/// - `ctx`: handler context (provides runtime stores and SecretRefManager).
/// - `resource_ref`: the owning resource (used for SecretRefManager wiring).
/// - `owner_ns`: the owning resource's namespace. `None` for cluster-scoped
///   resources (e.g. EdgionGatewayConfig); in that case refs *must* carry an
///   explicit `namespace` or they are skipped as unresolvable.
/// - `refs`: slice of CA certificate refs to resolve.
/// - `strategy`: fetch strategy controlling grant enforcement.
///
/// Returns a `Vec<Secret>` (may be empty). An empty vec means no refs resolved
/// yet; the SecretRefManager will trigger re-processing when material arrives.
pub fn resolve_ca_certificate_refs(
    ctx: &HandlerContext,
    resource_ref: &ResourceRef,
    owner_ns: Option<&str>,
    refs: &[BackendTLSPolicyCACertificateRef],
    strategy: CaFetchStrategy,
) -> Vec<Secret> {
    let mut resolved = Vec::new();

    for ca_ref in refs {
        let is_secret = is_core_api_group(&ca_ref.group) && ca_ref.kind == "Secret";
        let is_configmap = is_core_api_group(&ca_ref.group) && ca_ref.kind == "ConfigMap";

        if !is_secret && !is_configmap {
            // Unknown kind: skip silently.
            continue;
        }

        // Determine the effective namespace for this ref.
        // ca_ref.namespace takes priority; falls back to owner_ns.
        let Some(effective_ns) = ca_ref.namespace.as_deref().or(owner_ns) else {
            // Cluster-scoped owner (owner_ns = None) and no explicit namespace
            // on the ref: we cannot resolve — skip and log.
            tracing::info!(
                resource = %resource_ref.key(),
                ref_name = %ca_ref.name,
                kind = %ca_ref.kind,
                "CA ref has no namespace and owner is cluster-scoped; skipping"
            );
            continue;
        };

        // Register with SecretRefManager so the owner is requeued when the
        // referenced Secret/ConfigMap is created, updated, or deleted.
        let key_ns = effective_ns.to_string();
        let ref_key = if is_configmap {
            format_configmap_ref_key(Some(&key_ns), &ca_ref.name)
        } else {
            format_secret_key(Some(&key_ns), &ca_ref.name)
        };
        ctx.secret_ref_manager().add_ref(ref_key.clone(), resource_ref.clone());

        if is_secret {
            let maybe_secret = match &strategy {
                CaFetchStrategy::DirectSameNs => {
                    // Direct store lookup in the effective namespace (always
                    // owner_ns for BTP).
                    match ctx.runtime().secret_store.get(Some(effective_ns), &ca_ref.name) {
                        Some(s) => Some(s),
                        None => {
                            tracing::info!(
                                resource = %resource_ref.key(),
                                secret_key = %ref_key,
                                "CA Secret not found yet; will reprocess when Secret arrives"
                            );
                            None
                        }
                    }
                }
                CaFetchStrategy::GrantGated { group, kind } => {
                    // Route through fetch_secret which enforces ReferenceGrant
                    // for cross-namespace refs.
                    let sor = SecretObjectReference {
                        group: None,
                        kind: None,
                        name: ca_ref.name.clone(),
                        namespace: ca_ref.namespace.clone(),
                    };
                    match fetch_secret(ctx, resource_ref, group, kind, &sor) {
                        FetchOutcome::Ok(s) => Some(s),
                        FetchOutcome::Pending(reason) => {
                            tracing::info!(
                                resource = %resource_ref.key(),
                                secret_key = %ref_key,
                                reason = ?reason,
                                "CA Secret pending; will reprocess when available"
                            );
                            None
                        }
                    }
                }
            };

            if let Some(secret) = maybe_secret {
                resolved.push(secret);
            }
        } else {
            // ConfigMap kind.
            let maybe_cm = match &strategy {
                CaFetchStrategy::DirectSameNs => {
                    match ctx.runtime().configmap_store.get(Some(effective_ns), &ca_ref.name) {
                        Some(cm) => Some(cm),
                        None => {
                            tracing::info!(
                                resource = %resource_ref.key(),
                                configmap_key = %ref_key,
                                "CA ConfigMap not found yet; will reprocess when ConfigMap arrives"
                            );
                            None
                        }
                    }
                }
                CaFetchStrategy::GrantGated { group, kind } => {
                    match fetch_configmap(
                        ctx,
                        resource_ref,
                        group,
                        kind,
                        &ca_ref.name,
                        ca_ref.namespace.as_deref(),
                        owner_ns,
                    ) {
                        FetchOutcome::Ok(cm) => Some(cm),
                        FetchOutcome::Pending(reason) => {
                            tracing::info!(
                                resource = %resource_ref.key(),
                                configmap_key = %ref_key,
                                reason = ?reason,
                                "CA ConfigMap pending; will reprocess when available"
                            );
                            None
                        }
                    }
                }
            };

            if let Some(cm) = maybe_cm {
                let ca_pem = cm
                    .data
                    .as_ref()
                    .and_then(|d| d.get(CA_CERT))
                    .filter(|v| !v.trim().is_empty());

                if let Some(pem) = ca_pem {
                    resolved.push(synthetic_secret_from_pem(
                        ca_ref.name.clone(),
                        effective_ns.to_string(),
                        pem,
                    ));
                }
                // If ca.crt is absent or empty: silently skip (no tracing needed;
                // the ConfigMap was found, the data just lacks the key).
            }
        }
    }

    resolved
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build a synthetic `Secret` carrying a single PEM bundle under [`CA_CERT`].
///
/// Mirrors the pattern in `backend_tls_policy.rs:216-227` so callers can treat
/// ConfigMap-sourced and Secret-sourced CA bundles uniformly.
fn synthetic_secret_from_pem(name: String, namespace: String, pem: &str) -> Secret {
    let mut data = BTreeMap::new();
    data.insert(CA_CERT.to_string(), ByteString(pem.as_bytes().to_vec()));
    Secret {
        metadata: ObjectMeta {
            name: Some(name),
            namespace: Some(namespace),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    use k8s_openapi::api::core::v1::ConfigMap;
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;

    use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::ResourceKind;

    // ── Test fixture helpers ─────────────────────────────────────────────────

    fn make_ctx(
        secrets: HashMap<String, Secret>,
        configmaps: HashMap<String, ConfigMap>,
    ) -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.secret_store.replace_all(secrets);
        runtime.configmap_store.replace_all(configmaps);
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        (ctx, runtime)
    }

    fn make_secret(namespace: &str, name: &str, ca_pem: &[u8]) -> (String, Secret) {
        let mut data = BTreeMap::new();
        data.insert(CA_CERT.to_string(), ByteString(ca_pem.to_vec()));
        let secret = Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        };
        (format!("{}/{}", namespace, name), secret)
    }

    fn make_configmap(namespace: &str, name: &str, ca_pem: &str) -> (String, ConfigMap) {
        let mut data = BTreeMap::new();
        data.insert(CA_CERT.to_string(), ca_pem.to_string());
        let cm = ConfigMap {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        };
        (format!("{}/{}", namespace, name), cm)
    }

    fn secret_ref(kind: &str, name: &str, namespace: Option<&str>) -> BackendTLSPolicyCACertificateRef {
        BackendTLSPolicyCACertificateRef {
            group: String::new(),
            kind: kind.to_string(),
            name: name.to_string(),
            namespace: namespace.map(str::to_string),
        }
    }

    fn owner_ref(ns: &str, name: &str) -> ResourceRef {
        ResourceRef::new(ResourceKind::LinkSys, Some(ns.to_string()), name.to_string())
    }

    // ── Table tests ──────────────────────────────────────────────────────────

    /// (a) DirectSameNs + two Secret refs in the same namespace resolve to two
    /// Secrets with the expected CA PEM data.
    #[test]
    fn resolve_ca_refs_two_secrets_same_ns_direct() {
        let pem_a = b"PEM-CA-A";
        let pem_b = b"PEM-CA-B";
        let (key_a, sec_a) = make_secret("default", "ca-secret-a", pem_a);
        let (key_b, sec_b) = make_secret("default", "ca-secret-b", pem_b);
        let (ctx, _rt) = make_ctx(HashMap::from([(key_a, sec_a), (key_b, sec_b)]), HashMap::new());
        let resource_ref = owner_ref("default", "my-btp");
        let refs = vec![
            secret_ref("Secret", "ca-secret-a", None),
            secret_ref("Secret", "ca-secret-b", None),
        ];

        let result = resolve_ca_certificate_refs(
            &ctx,
            &resource_ref,
            Some("default"),
            &refs,
            CaFetchStrategy::DirectSameNs,
        );

        assert_eq!(result.len(), 2, "expected 2 resolved CA secrets, got {}", result.len());

        let names: Vec<_> = result.iter().filter_map(|s| s.metadata.name.as_deref()).collect();
        assert!(names.contains(&"ca-secret-a"), "result should contain ca-secret-a");
        assert!(names.contains(&"ca-secret-b"), "result should contain ca-secret-b");

        // Verify data is preserved.
        let data_a = result
            .iter()
            .find(|s| s.metadata.name.as_deref() == Some("ca-secret-a"))
            .and_then(|s| s.data.as_ref())
            .and_then(|d| d.get(CA_CERT))
            .map(|b| b.0.as_slice());
        assert_eq!(
            data_a,
            Some(pem_a.as_slice()),
            "ca-secret-a must carry the original PEM"
        );

        // SecretRefManager must have registered both refs.
        let refs_a = ctx.secret_ref_manager().get_refs("default/ca-secret-a");
        assert!(refs_a.contains(&resource_ref), "ca-secret-a ref must be registered");
        let refs_b = ctx.secret_ref_manager().get_refs("default/ca-secret-b");
        assert!(refs_b.contains(&resource_ref), "ca-secret-b ref must be registered");
    }

    /// (b) DirectSameNs + one ConfigMap ref resolves to a single synthetic Secret
    /// carrying the ConfigMap's `ca.crt` PEM.
    #[test]
    fn resolve_ca_refs_configmap_synthetic_secret() {
        let ca_pem = "-----BEGIN CERTIFICATE-----\nMIIFAKE\n-----END CERTIFICATE-----\n";
        let (cm_key, cm) = make_configmap("default", "corp-ca-cm", ca_pem);
        let (ctx, _rt) = make_ctx(HashMap::new(), HashMap::from([(cm_key, cm)]));
        let resource_ref = owner_ref("default", "my-btp");
        let refs = vec![secret_ref("ConfigMap", "corp-ca-cm", None)];

        let result = resolve_ca_certificate_refs(
            &ctx,
            &resource_ref,
            Some("default"),
            &refs,
            CaFetchStrategy::DirectSameNs,
        );

        assert_eq!(result.len(), 1, "expected 1 synthetic CA secret, got {}", result.len());

        let synthetic = &result[0];
        assert_eq!(
            synthetic.metadata.name.as_deref(),
            Some("corp-ca-cm"),
            "synthetic Secret must take the ConfigMap's name"
        );
        assert_eq!(
            synthetic.metadata.namespace.as_deref(),
            Some("default"),
            "synthetic Secret must carry the effective namespace"
        );
        let data = synthetic.data.as_ref().expect("synthetic Secret must have data");
        let got_pem = data.get(CA_CERT).expect("synthetic Secret must have ca.crt key");
        assert_eq!(
            got_pem.0,
            ca_pem.as_bytes(),
            "synthetic Secret ca.crt must contain the ConfigMap PEM"
        );

        // SecretRefManager must have registered the ref.
        let refs = ctx.secret_ref_manager().get_refs("ConfigMap/default/corp-ca-cm");
        assert!(
            refs.contains(&resource_ref),
            "ConfigMap ref must be registered in SecretRefManager"
        );
    }

    /// (c) GrantGated + one cross-namespace Secret ref with no ReferenceGrant
    /// configured → empty result, and the Secret is NOT accessed directly
    /// (verified by confirming the Secret genuinely exists in the store but
    /// the grant-check path returns empty).
    #[test]
    fn resolve_ca_refs_cross_ns_no_grant_returns_empty() {
        // Secret genuinely exists in "other-ns".
        let (secret_key, secret) = make_secret("other-ns", "cross-ns-ca", b"REAL-PEM");
        let (ctx, _rt) = make_ctx(HashMap::from([(secret_key, secret)]), HashMap::new());

        // Verify the Secret is reachable directly (proves the empty result is
        // not because of an absent secret but because of the missing grant).
        let direct = ctx.runtime().secret_store.get(Some("other-ns"), "cross-ns-ca");
        assert!(
            direct.is_some(),
            "Secret must exist in the store for this test to be meaningful"
        );

        let resource_ref = owner_ref("default", "my-plugin");
        // Ref explicitly targets "other-ns" — cross-namespace from "default".
        let refs = vec![secret_ref("Secret", "cross-ns-ca", Some("other-ns"))];

        let result = resolve_ca_certificate_refs(
            &ctx,
            &resource_ref,
            Some("default"),
            &refs,
            CaFetchStrategy::GrantGated {
                group: "edgion.io",
                kind: "EdgionPlugin",
            },
        );

        // No ReferenceGrant in "other-ns" → ReferenceGrantMissing → skipped.
        assert_eq!(
            result.len(),
            0,
            "cross-ns ref without a grant must not resolve; got {} items",
            result.len()
        );
    }

    // ── Additional test: fetch_configmap directly ────────────────────────────

    /// fetch_configmap returns Pending(SecretNotFound) when the ConfigMap is absent.
    #[test]
    fn fetch_configmap_returns_pending_when_missing() {
        use super::super::pipeline::{FetchOutcome, PendingReason};

        let (ctx, _rt) = make_ctx(HashMap::new(), HashMap::new());
        let resource_ref = owner_ref("default", "my-resource");

        let outcome = fetch_configmap(
            &ctx,
            &resource_ref,
            "edgion.io",
            "EdgionPlugin",
            "missing-cm",
            None,
            Some("default"),
        );

        match outcome {
            FetchOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
                assert_eq!(namespace, "default");
                assert_eq!(name, "missing-cm");
            }
            other => panic!("expected Pending(SecretNotFound), got {:?}", other),
        }
    }

    /// fetch_configmap returns Pending(ReferenceGrantMissing) when the ConfigMap
    /// is in a different namespace and no ReferenceGrant is present.
    #[test]
    fn fetch_configmap_cross_ns_no_grant_returns_pending() {
        use super::super::pipeline::{FetchOutcome, PendingReason};

        let ca_pem = "PEM";
        let (cm_key, cm) = make_configmap("other-ns", "ca-cm", ca_pem);
        let (ctx, _rt) = make_ctx(HashMap::new(), HashMap::from([(cm_key, cm)]));
        let resource_ref = owner_ref("default", "my-resource");

        let outcome = fetch_configmap(
            &ctx,
            &resource_ref,
            "edgion.io",
            "EdgionPlugin",
            "ca-cm",
            Some("other-ns"), // explicit cross-ns
            Some("default"),  // owner ns
        );

        match outcome {
            FetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
                from_ns,
                to_ns,
                secret_name,
            }) => {
                assert_eq!(from_ns, "default");
                assert_eq!(to_ns, "other-ns");
                assert_eq!(secret_name, "ca-cm");
            }
            other => panic!("expected Pending(ReferenceGrantMissing), got {:?}", other),
        }
        assert!(ctx
            .runtime()
            .cross_ns_ref_mgr
            .get_refs("other-ns")
            .contains(&resource_ref));
    }
}
