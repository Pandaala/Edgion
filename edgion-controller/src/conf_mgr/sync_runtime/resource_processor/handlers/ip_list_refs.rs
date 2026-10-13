//! Shared fixed-IpList lookup policy for actual EP/ESP and cluster RealIp fields.
use super::config_data_ref_mark::ExpectedEntry;
use crate::conf_mgr::sync_runtime::resource_processor::ref_grant::decide_edgion_config_data_ref;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResolvedRefsError};
use crate::config::is_reference_grant_validation_enabled;
use edgion_resources::refs::ip_list_fields::{
    BoundGeoIpAccess, BoundGeoIpAccessMut, BoundIpListRefs, BoundIpListRefsMut, IpListSide, PreparedIpListReferences,
};
use edgion_resources::refs::{BoundFieldReference, GrantQuery, ReferenceError, ReferenceToken};
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::edgion_config_data::ConfigDataVisibility;
use edgion_resources::resources::edgion_plugins::ip_reference_set::IpReadScope;
use edgion_resources::{EdgionConfigData, EdgionPlugins, EdgionStreamPlugins};

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn validated_ip_reference<'a>(
    token: &'a ReferenceToken<'_>,
) -> Result<BoundFieldReference<'a>, ReferenceError> {
    let bound = token.validated()?;
    let source = bound.source().group_kind();
    let target = bound.target().group_kind();
    if source.group() != "edgion.io"
        || !matches!(
            source.kind(),
            "EdgionPlugins" | "EdgionStreamPlugins" | "EdgionGatewayConfig"
        )
        || (source.kind() == "EdgionGatewayConfig"
            && bound.source().scope() != edgion_resources::refs::ResourceScope::Cluster)
        || target.group() != "edgion.io"
        || target.kind() != "EdgionConfigData"
    {
        return Err(ReferenceError::UnsupportedTarget);
    }
    Ok(bound)
}
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) struct IpResolution {
    pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) read_scope: IpReadScope,
    pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) denied: Option<RefDenied>,
    pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) error: Option<ResolvedRefsError>,
    pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) data: Option<EdgionConfigData>,
    pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) warning: Option<String>,
}

// The actual owning token is validated before either injected read boundary.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve_ip_reference(
    token: ReferenceToken<'_>,
    validation_enabled: bool,
    read: impl FnMut(&str, &str) -> Option<EdgionConfigData>,
    grant: impl FnMut(GrantQuery<'_>) -> bool,
) -> IpResolution {
    resolve_ip_reference_inner(token, validation_enabled, read, grant, true)
}
fn resolve_ip_reference_inner(
    token: ReferenceToken<'_>,
    validation_enabled: bool,
    mut read: impl FnMut(&str, &str) -> Option<EdgionConfigData>,
    mut grant: impl FnMut(GrantQuery<'_>) -> bool,
    warnings: bool,
) -> IpResolution {
    let bound = match validated_ip_reference(&token) {
        Ok(bound) => bound,
        Err(error) => {
            return IpResolution {
                read_scope: IpReadScope::Denied,
                denied: None,
                data: None,
                warning: warnings.then(|| format!("RequestAccessPolicy IP data reference is invalid: {error}")),
                error: Some(ResolvedRefsError::InvalidReference { error }),
            }
        }
    };
    let target = bound.target();
    let namespace = target.scope().namespace().unwrap_or_default();
    let name = target.name();
    let intrinsic_scope = if bound.is_same_namespace() {
        IpReadScope::SameNamespace
    } else if !validation_enabled {
        IpReadScope::ValidationDisabled
    } else {
        IpReadScope::ClusterOnly
    };
    let unavailable = |suffix: &str| IpResolution {
        read_scope: intrinsic_scope,
        denied: None,
        data: None,
        error: Some(ResolvedRefsError::EdgionConfigDataNotFound {
            namespace: namespace.into(),
            name: name.into(),
            expected_type: "IpList".into(),
        }),
        warning: warnings.then(|| format!("RequestAccessPolicy IP data ref {namespace}/{name} {suffix}")),
    };
    let Some(data) = read(namespace, name) else {
        return unavailable("is unavailable");
    };
    if data.metadata.name.as_deref() != Some(name) || data.metadata.namespace.as_deref() != Some(namespace) {
        return IpResolution {
            read_scope: IpReadScope::Denied,
            denied: None,
            data: None,
            warning: warnings.then(|| "IP data reference target identity does not match".into()),
            error: Some(ResolvedRefsError::InvalidReference {
                error: ReferenceError::StaleBinding,
            }),
        };
    }
    if !ExpectedEntry::IpList.matches(&data.spec.data) {
        return unavailable("has the wrong type");
    }
    let same_namespace = bound.is_same_namespace();
    let cluster_owner = bound.source().scope() == edgion_resources::refs::ResourceScope::Cluster;
    let grant_allows = !cluster_owner
        && !same_namespace
        && validation_enabled
        && data.spec.visibility != ConfigDataVisibility::Cluster
        && grant(bound.grant_query());
    if !decide_edgion_config_data_ref(validation_enabled, same_namespace, data.spec.visibility, grant_allows) {
        return IpResolution {
            read_scope: IpReadScope::Denied,
            denied: Some(RefDenied {
                target_namespace: namespace.into(),
                target_name: name.into(),
                reason: Some(
                    if cluster_owner {
                        "ClusterScopedReferenceRequiresClusterVisibility"
                    } else {
                        "NoMatchingReferenceGrant"
                    }
                    .into(),
                ),
            }),
            error: Some(ResolvedRefsError::RefNotPermitted {
                from_namespace: bound.source().scope().namespace().unwrap_or_default().into(),
                from_kind: match bound.source().group_kind().kind() {
                    "EdgionPlugins" => "EdgionPlugins",
                    "EdgionGatewayConfig" => "EdgionGatewayConfig",
                    _ => "EdgionStreamPlugins",
                },
                target_namespace: namespace.into(),
                target_name: name.into(),
            }),
            data: None,
            warning: warnings.then(|| format!("RequestAccessPolicy IP data ref {namespace}/{name} is not permitted")),
        };
    }
    let read_scope = if same_namespace {
        IpReadScope::SameNamespace
    } else if !validation_enabled {
        IpReadScope::ValidationDisabled
    } else if data.spec.visibility == ConfigDataVisibility::Cluster {
        IpReadScope::ClusterOnly
    } else {
        IpReadScope::GrantAuthorized
    };
    if !data.spec.enable {
        let mut result = unavailable("is disabled");
        result.read_scope = read_scope;
        return result;
    }
    IpResolution {
        read_scope,
        denied: None,
        error: None,
        data: Some(data),
        warning: None,
    }
}

fn resolve(token: ReferenceToken<'_>, ctx: &HandlerContext) -> IpResolution {
    resolve_ip_reference_inner(
        token,
        is_reference_grant_validation_enabled(),
        |ns, name| ctx.runtime().edgion_config_data_store.get(Some(ns), name),
        |query| ctx.runtime().ref_grant_store.check_query(query),
        false,
    )
}

fn pairs<'a>(refs: impl Iterator<Item = BoundIpListRefs<'a>>) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for refs in refs {
        if !refs.is_controller_reference_site() {
            continue;
        }
        for (token, _) in refs.allow().chain(refs.deny()) {
            if let Ok(bound) = validated_ip_reference(&token) {
                result.push((
                    bound.target().scope().namespace().unwrap_or_default().into(),
                    bound.target().name().into(),
                ));
            }
        }
    }
    result
}
fn errors<'a>(refs: impl Iterator<Item = BoundIpListRefs<'a>>, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let mut result = Vec::new();
    for refs in refs {
        if !refs.is_controller_reference_site() {
            continue;
        }
        for (token, _) in refs.allow().chain(refs.deny()) {
            result.extend(resolve(token, ctx).error);
        }
    }
    result
}
fn mark_acl(mut leaf: BoundIpListRefsMut<'_>, ctx: &HandlerContext) {
    leaf.clear_read_metadata();
    if !leaf.is_supported() {
        return;
    }
    for side in [IpListSide::Allow, IpListSide::Deny] {
        let resolutions: Vec<_> = leaf
            .references(side)
            .map(|(token, _)| {
                let resolution = resolve(token, ctx);
                (resolution.read_scope, resolution.denied)
            })
            .collect();
        if resolutions.is_empty() {
            continue;
        }
        let scopes: Vec<_> = resolutions.iter().map(|entry| entry.0).collect();
        // Tokens are validated before lookup; only bounded metadata survives the turn.
        if leaf
            .set_denied(side, resolutions.into_iter().map(|entry| entry.1).collect())
            .is_ok()
        {
            let _ = leaf.set_read_scopes(side, &scopes);
        }
    }
}
fn geo_pairs(leaf: BoundGeoIpAccess<'_>, result: &mut Vec<(String, String)>) {
    if !leaf.is_supported() {
        return;
    }
    if let Ok(tokens) = leaf.references() {
        for token in tokens {
            if let Ok(bound) = validated_ip_reference(&token) {
                result.push((
                    bound.target().scope().namespace().unwrap_or_default().into(),
                    bound.target().name().into(),
                ));
            }
        }
    }
}
fn mark_geo(mut leaf: BoundGeoIpAccessMut<'_>, ctx: &HandlerContext) {
    // Reset every actual leaf, including inactive/invalid entries, before any lookup.
    leaf.config_mut().resolved_refs = None;
    for reference in &mut leaf.config_mut().ip_refs {
        reference.ref_denied = None;
    }
    if !leaf.is_supported() {
        return;
    }
    let resolutions = match leaf.references() {
        Ok(tokens) => tokens
            .map(|token| {
                let outcome = resolve(token, ctx);
                // Retain bounded read metadata only; drop the cold store payload immediately.
                (outcome.read_scope, outcome.denied)
            })
            .collect::<Vec<_>>(),
        Err(_) => return,
    };
    let scopes: Vec<_> = resolutions.iter().map(|value| value.0).collect();
    for (reference, (_, denied)) in leaf.config_mut().ip_refs.iter_mut().zip(resolutions) {
        reference.ref_denied = denied;
    }
    let _ = leaf.set_read_scopes(&scopes);
}
fn geo_errors(leaf: BoundGeoIpAccess<'_>, ctx: &HandlerContext, result: &mut Vec<ResolvedRefsError>) {
    if !leaf.is_supported() {
        return;
    }
    match leaf.references() {
        Ok(tokens) => {
            for token in tokens {
                result.extend(resolve(token, ctx).error);
            }
        }
        Err(error) => result.push(ResolvedRefsError::InvalidReference { error }),
    }
}
pub(super) fn http_pairs(owner: &EdgionPlugins) -> Vec<(String, String)> {
    let prepared = PreparedIpListReferences::from_plugins(owner);
    let mut result = prepared
        .bind_plugins(owner)
        .map(|binding| pairs(binding.references()))
        .unwrap_or_default();
    let _ = prepared.visit_geo_plugins(owner, |leaf| geo_pairs(leaf, &mut result));
    result.sort();
    result.dedup();
    result
}
pub(super) fn stream_pairs(owner: &EdgionStreamPlugins) -> Vec<(String, String)> {
    let prepared = PreparedIpListReferences::from_stream_plugins(owner);
    let mut result = prepared
        .bind_stream_plugins(owner)
        .map(|binding| pairs(binding.references()))
        .unwrap_or_default();
    let _ = prepared.visit_geo_stream_plugins(owner, |leaf| geo_pairs(leaf, &mut result));
    result.sort();
    result.dedup();
    result
}
pub(super) fn mark_http(owner: &mut EdgionPlugins, ctx: &HandlerContext) {
    let prepared = PreparedIpListReferences::from_plugins(owner);
    let _ = prepared.visit_acl_plugins_mut(owner, |leaf| mark_acl(leaf, ctx));
    let _ = prepared.visit_geo_plugins_mut(owner, |leaf| mark_geo(leaf, ctx));
}
pub(super) fn mark_stream(owner: &mut EdgionStreamPlugins, ctx: &HandlerContext) {
    let prepared = PreparedIpListReferences::from_stream_plugins(owner);
    let _ = prepared.visit_acl_stream_plugins_mut(owner, |leaf| mark_acl(leaf, ctx));
    let _ = prepared.visit_geo_stream_plugins_mut(owner, |leaf| mark_geo(leaf, ctx));
}
pub(super) fn http_errors(owner: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let prepared = PreparedIpListReferences::from_plugins(owner);
    let mut result = match prepared.bind_plugins(owner) {
        Ok(binding) => errors(binding.references(), ctx),
        Err(error) => vec![ResolvedRefsError::InvalidReference { error }],
    };
    if let Err(error) = prepared.visit_geo_plugins(owner, |leaf| geo_errors(leaf, ctx, &mut result)) {
        result.push(ResolvedRefsError::InvalidReference { error });
    }
    result
}
pub(super) fn stream_errors(owner: &EdgionStreamPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let prepared = PreparedIpListReferences::from_stream_plugins(owner);
    let mut result = match prepared.bind_stream_plugins(owner) {
        Ok(binding) => errors(binding.references(), ctx),
        Err(error) => vec![ResolvedRefsError::InvalidReference { error }],
    };
    if let Err(error) = prepared.visit_geo_stream_plugins(owner, |leaf| geo_errors(leaf, ctx, &mut result)) {
        result.push(ResolvedRefsError::InvalidReference { error });
    }
    result
}
#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/ip_list_refs_test.rs"]
mod tests;
