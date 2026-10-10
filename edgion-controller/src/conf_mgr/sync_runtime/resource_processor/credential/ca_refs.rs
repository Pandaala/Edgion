//! Owning plugin TLS material acquisition and CA transport projection.
//! Field tokens own target defaults, authorization and recovery semantics.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::ByteString;

use edgion_resources::constants::annotations::edgion::{CONTROLLER_CA_SOURCE, CONTROLLER_CA_SOURCE_CONFIG_MAP};
use edgion_resources::constants::secret_keys::tls::CA_CERT;

use super::super::{HandlerContext, ResourceRef};

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
