//! Cache-only acquisition for the two approved cluster EGC TLS fields.
use super::super::{HandlerContext, ResourceRef, CONFIG_MAP_REF_KEY_PREFIX};
use super::ca_refs::{configmap_ca_material, ordinary_secret_material, BoundTlsMaterial};
use super::{BoundFetchOutcome, PendingReason};
use edgion_resources::resources::refs::egc_tls_fields::{
    EgcTlsMaterialSelection, EgcTlsReferences, PreparedEgcTlsReferences,
};
use edgion_resources::resources::refs::{ReferenceError, ResourceScope};
use edgion_resources::{EdgionGatewayConfig, ResourceKind};
use k8s_openapi::api::core::v1::Secret;

/// Intrinsic identity only: no material visibility or Grant state affects EGC status.
pub(crate) fn identity_errors(config: &EdgionGatewayConfig) -> Vec<String> {
    let prepared = PreparedEgcTlsReferences::from_config(config);
    let binding = match prepared.bind(config) {
        Ok(binding) => binding,
        Err(error) => return vec![format!("outboundTls owner: {error}")],
    };
    let references = match binding.references() {
        Ok(references) => references,
        Err(error) => return vec![format!("outboundTls: {error}")],
    };
    let mut errors = Vec::new();
    for (index, selection) in references.ca_certificates().enumerate() {
        if let Err(error) = selection {
            errors.push(format!("outboundTls.validation.caCertificateRefs[{index}]: {error}"));
        }
    }
    if let Some(Err(error)) = references.client_certificate() {
        errors.push(format!("outboundTls.clientCertificateRef: {error}"));
    }
    errors
}

pub(crate) fn resolve(ctx: &HandlerContext, owner: &ResourceRef, references: EgcTlsReferences<'_>) -> BoundTlsMaterial {
    let mut ca_certificates = Vec::new();
    for selection in references.ca_certificates().flatten() {
        if let BoundFetchOutcome::Ok(Some(material)) = fetch(ctx, owner, selection) {
            ca_certificates.push(material);
        }
    }
    let client_certificate = references
        .client_certificate()
        .and_then(Result::ok)
        .and_then(|selection| match fetch(ctx, owner, selection) {
            BoundFetchOutcome::Ok(material) => material,
            _ => None,
        });
    BoundTlsMaterial {
        ca_certificates: (!ca_certificates.is_empty()).then_some(ca_certificates),
        client_certificate,
    }
}

/// Deployment-local availability of the declared TLS material, for the
/// consuming GatewayClass only. Shared EGC status must never use this: another
/// deployment may see different namespaces. Read-only: it registers no
/// dependency, because the EGC already owns the material recovery edges.
/// Intrinsic reference errors are skipped; shared EGC Accepted reports them.
pub(crate) fn unavailable_material(ctx: &HandlerContext, config: &EdgionGatewayConfig) -> Vec<String> {
    let prepared = PreparedEgcTlsReferences::from_config(config);
    let Ok(binding) = prepared.bind(config) else {
        return Vec::new();
    };
    let Ok(references) = binding.references() else {
        return Vec::new();
    };
    let owner = ResourceRef::new(
        ResourceKind::EdgionGatewayConfig,
        None,
        config.metadata.name.clone().unwrap_or_default(),
    );
    let mut unavailable = Vec::new();
    for (index, selection) in references.ca_certificates().enumerate() {
        if let Some(problem) = selection
            .ok()
            .and_then(|selection| availability(ctx, &owner, selection))
        {
            unavailable.push(format!("outboundTls.validation.caCertificateRefs[{index}]: {problem}"));
        }
    }
    if let Some(problem) = references
        .client_certificate()
        .and_then(Result::ok)
        .and_then(|selection| availability(ctx, &owner, selection))
    {
        unavailable.push(format!("outboundTls.clientCertificateRef: {problem}"));
    }
    unavailable
}

fn availability(ctx: &HandlerContext, owner: &ResourceRef, selection: EgcTlsMaterialSelection<'_>) -> Option<String> {
    let target = locate(owner, selection).ok()?;
    match read(ctx, &target) {
        BoundFetchOutcome::Ok(Some(_)) => None,
        BoundFetchOutcome::Ok(None) => Some(format!(
            "ConfigMap '{}/{}' has no CA certificate data",
            target.namespace, target.name
        )),
        _ => Some(format!(
            "{} '{}/{}' not found",
            if target.configmap { "ConfigMap" } else { "Secret" },
            target.namespace,
            target.name
        )),
    }
}

/// A validated EGC TLS material target.
struct MaterialTarget {
    configmap: bool,
    namespace: String,
    name: String,
}

fn locate(owner: &ResourceRef, selection: EgcTlsMaterialSelection<'_>) -> Result<MaterialTarget, ReferenceError> {
    let reference = selection.reference().validated()?;
    if owner.kind != ResourceKind::EdgionGatewayConfig
        || owner.namespace.is_some()
        || owner.name != reference.owner().name()
        || reference.source().group_kind().group() != "edgion.io"
        || reference.source().group_kind().kind() != "EdgionGatewayConfig"
        || reference.source().scope() != ResourceScope::Cluster
        || reference.owner().scope() != ResourceScope::Cluster
    {
        return Err(ReferenceError::StaleBinding);
    }
    let target = reference.target();
    let kind = target.group_kind().kind();
    if target.group_kind().group() != ""
        || !matches!(kind, "Secret" | "ConfigMap")
        || (selection.is_client_certificate() && kind != "Secret")
    {
        return Err(ReferenceError::UnsupportedTarget);
    }
    let Some(namespace) = target.scope().namespace() else {
        return Err(ReferenceError::MissingNamespace);
    };
    Ok(MaterialTarget {
        configmap: kind == "ConfigMap",
        namespace: namespace.to_string(),
        name: target.name().to_string(),
    })
}

fn read(ctx: &HandlerContext, target: &MaterialTarget) -> BoundFetchOutcome<Option<Secret>> {
    // This capability is the exact cluster administrative selection, not a
    // namespaced Grant exception. There is no fictitious source namespace.
    if target.configmap {
        if let Some(configmap) = ctx.runtime().configmap_store.get(Some(&target.namespace), &target.name) {
            return BoundFetchOutcome::Ok(configmap_ca_material(&configmap));
        }
    } else if let Some(secret) = ctx.runtime().secret_store.get(Some(&target.namespace), &target.name) {
        return BoundFetchOutcome::Ok(Some(ordinary_secret_material(secret)));
    }
    BoundFetchOutcome::Pending(PendingReason::SecretNotFound {
        namespace: target.namespace.clone(),
        name: target.name.clone(),
    })
}

fn fetch(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    selection: EgcTlsMaterialSelection<'_>,
) -> BoundFetchOutcome<Option<Secret>> {
    let target = match locate(owner, selection) {
        Ok(target) => target,
        Err(error) => return BoundFetchOutcome::Invalid(error),
    };
    let key = if target.configmap {
        format!("{CONFIG_MAP_REF_KEY_PREFIX}{}/{}", target.namespace, target.name)
    } else {
        format!("{}/{}", target.namespace, target.name)
    };
    ctx.secret_ref_manager().add_ref(key, owner.clone());
    read(ctx, &target)
}
