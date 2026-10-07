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
//!   EGC's two cluster-owned TLS fields instead use their dedicated owning
//!   administrative selections in `egc_tls`; no source namespace is fabricated.
//!
//! ConfigMap refs are converted into synthetic `Secret` objects carrying the
//! `ca.crt` PEM under the standard [`CA_CERT`] key, so downstream consumers
//! can treat all resolved CA bundles uniformly as `Secret` objects.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::ByteString;

use edgion_resources::constants::annotations::edgion::{CONTROLLER_CA_SOURCE, CONTROLLER_CA_SOURCE_CONFIG_MAP};
use edgion_resources::constants::secret_keys::tls::CA_CERT;
use edgion_resources::resources::common::CustomCACertificateRef;
use edgion_resources::resources::common::{is_core_api_group, SecretObjectReference};

use super::super::{format_configmap_ref_key, format_secret_key, HandlerContext, ResourceRef};
use super::pipeline::{fetch_configmap, fetch_secret, FetchOutcome};

/// Material rebuilt from the actual owning TLS fields; no configuration loan
/// survives publication back into the plugin.
pub(crate) struct BoundTlsMaterial {
    pub ca_certificates: Option<Vec<Secret>>,
    pub client_certificate: Option<Secret>,
}

pub(crate) fn resolve_bound_tls(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    references: edgion_resources::resources::refs::plugin_fields::TlsReferences<'_>,
) -> BoundTlsMaterial {
    use super::pipeline::{fetch_bound_configmap, fetch_bound_secret, BoundFetchOutcome};
    let mut ca_certificates = Vec::new();
    for token in references.ca_certificates() {
        let Ok(reference) = token.validated() else { continue };
        match reference.target().group_kind().kind() {
            "Secret" => {
                if let BoundFetchOutcome::Ok(authorized) = fetch_bound_secret(ctx, owner, token) {
                    ca_certificates.push(authorized.secret);
                }
            }
            "ConfigMap" => {
                if let BoundFetchOutcome::Ok(configmap) = fetch_bound_configmap(ctx, owner, token) {
                    if let Some(material) = configmap_ca_material(&configmap) {
                        ca_certificates.push(material);
                    }
                }
            }
            _ => {}
        }
    }
    let client_certificate =
        references
            .client_certificate()
            .and_then(|token| match fetch_bound_secret(ctx, owner, token) {
                BoundFetchOutcome::Ok(authorized) => Some(authorized.secret),
                BoundFetchOutcome::Pending(_) | BoundFetchOutcome::Invalid(_) => None,
            });
    BoundTlsMaterial {
        ca_certificates: (!ca_certificates.is_empty()).then_some(ca_certificates),
        client_certificate,
    }
}

pub(crate) fn bound_ca_status_error(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: edgion_resources::resources::refs::ReferenceToken<'_>,
) -> Option<super::super::ResolvedRefsError> {
    use super::super::ResolvedRefsError;
    use super::pipeline::{bound_configmap_status_result, bound_secret_status_error};
    let reference = match token.validated() {
        Ok(reference) => reference,
        Err(error) => return Some(ResolvedRefsError::InvalidReference { error }),
    };
    if reference.target().group_kind().kind() == "Secret" {
        return bound_secret_status_error(ctx, owner, token);
    }
    bound_configmap_status_result(ctx, owner, token).err()
}

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
    /// Remaining legacy callers may span namespaces with appropriate grants.
    /// EGC uses its exclusive typed administrative field selection instead.
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
/// - `owner_ns`: the owning namespace for this legacy helper. With no owner
///   namespace, refs need an explicit namespace to be resolvable. This helper
///   does not mint EGC administrative authority and is not used by that owner.
/// - `refs`: slice of CA certificate refs to resolve.
/// - `strategy`: fetch strategy controlling grant enforcement.
///
/// Returns a `Vec<Secret>` (may be empty). An empty vec means no refs resolved
/// yet; the SecretRefManager will trigger re-processing when material arrives.
pub fn resolve_ca_certificate_refs(
    ctx: &HandlerContext,
    resource_ref: &ResourceRef,
    owner_ns: Option<&str>,
    refs: &[CustomCACertificateRef],
    strategy: CaFetchStrategy,
) -> Vec<Secret> {
    let mut resolved = Vec::new();

    for ca_ref in refs {
        let is_secret = is_core_api_group(ca_ref.group.as_deref().unwrap_or_default()) && ca_ref.kind == "Secret";
        let is_configmap = is_core_api_group(ca_ref.group.as_deref().unwrap_or_default()) && ca_ref.kind == "ConfigMap";

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
                resolved.push(ordinary_secret_material(secret));
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
                if let Some(material) = configmap_ca_material(&cm) {
                    resolved.push(material);
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

/// Convert an actual fetched ConfigMap to the resolved CA transport shape.
/// Only identity metadata and the exact internal source marker are retained.
pub(crate) fn configmap_ca_material(configmap: &ConfigMap) -> Option<Secret> {
    let pem = configmap
        .data
        .as_ref()?
        .get(CA_CERT)
        .filter(|value| !value.trim().is_empty())?;
    let mut data = BTreeMap::new();
    data.insert(CA_CERT.to_string(), ByteString(pem.as_bytes().to_vec()));
    Some(Secret {
        metadata: ObjectMeta {
            name: configmap.metadata.name.clone(),
            namespace: configmap.metadata.namespace.clone(),
            uid: configmap.metadata.uid.clone(),
            resource_version: configmap.metadata.resource_version.clone(),
            annotations: Some(BTreeMap::from([(
                CONTROLLER_CA_SOURCE.to_string(),
                CONTROLLER_CA_SOURCE_CONFIG_MAP.to_string(),
            )])),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    })
}

/// Discard operator-supplied source markers from ordinary fetched Secrets.
pub(crate) fn ordinary_secret_material(mut secret: Secret) -> Secret {
    if let Some(annotations) = secret.metadata.annotations.as_mut() {
        annotations.remove(CONTROLLER_CA_SOURCE);
    }
    secret
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/credential/ca_refs_test.rs"]
mod tests;
