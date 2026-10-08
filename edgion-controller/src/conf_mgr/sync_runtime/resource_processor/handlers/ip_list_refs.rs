//! Shared fixed-IpList lookup policy for actual EP/ESP fields.
use super::config_data_ref_mark::ExpectedEntry;
use crate::conf_mgr::sync_runtime::resource_processor::ref_grant::decide_edgion_config_data_ref;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResolvedRefsError};
use crate::config::is_reference_grant_validation_enabled;
use edgion_resources::refs::ip_list_fields::{BoundIpListRefs, PreparedIpListReferences};
use edgion_resources::refs::{BoundFieldReference, GrantQuery, ReferenceError, ReferenceToken};
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::edgion_config_data::ConfigDataVisibility;
use edgion_resources::{EdgionConfigData, EdgionPlugins, EdgionStreamPlugins};

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn validated_ip_reference<'a>(
    token: &'a ReferenceToken<'_>,
) -> Result<BoundFieldReference<'a>, ReferenceError> {
    let bound = token.validated()?;
    let source = bound.source().group_kind();
    let target = bound.target().group_kind();
    if source.group() != "edgion.io"
        || !matches!(source.kind(), "EdgionPlugins" | "EdgionStreamPlugins")
        || target.group() != "edgion.io"
        || target.kind() != "EdgionConfigData"
    {
        return Err(ReferenceError::UnsupportedTarget);
    }
    Ok(bound)
}
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) struct IpResolution {
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
    let unavailable = |suffix: &str| IpResolution {
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
    if !ExpectedEntry::IpList.matches(&data.spec.data) {
        return unavailable("has the wrong type");
    }
    let same_namespace = bound.is_same_namespace();
    let grant_allows = !same_namespace
        && validation_enabled
        && data.spec.visibility != ConfigDataVisibility::Cluster
        && grant(bound.grant_query());
    if !decide_edgion_config_data_ref(validation_enabled, same_namespace, data.spec.visibility, grant_allows) {
        return IpResolution {
            denied: Some(RefDenied {
                target_namespace: namespace.into(),
                target_name: name.into(),
                reason: Some("NoMatchingReferenceGrant".into()),
            }),
            error: Some(ResolvedRefsError::RefNotPermitted {
                from_namespace: bound.source().scope().namespace().unwrap_or_default().into(),
                from_kind: if bound.source().group_kind().kind() == "EdgionPlugins" {
                    "EdgionPlugins"
                } else {
                    "EdgionStreamPlugins"
                },
                target_namespace: namespace.into(),
                target_name: name.into(),
            }),
            data: None,
            warning: warnings.then(|| format!("RequestAccessPolicy IP data ref {namespace}/{name} is not permitted")),
        };
    }
    if !data.spec.enable {
        return unavailable("is disabled");
    }
    IpResolution {
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
        for (token, _) in refs.allow().chain(refs.deny()) {
            result.extend(resolve(token, ctx).error);
        }
    }
    result
}
pub(super) fn http_pairs(owner: &EdgionPlugins) -> Vec<(String, String)> {
    let prepared = PreparedIpListReferences::from_plugins(owner);
    prepared
        .bind_plugins(owner)
        .map(|binding| pairs(binding.references()))
        .unwrap_or_default()
}
pub(super) fn stream_pairs(owner: &EdgionStreamPlugins) -> Vec<(String, String)> {
    let prepared = PreparedIpListReferences::from_stream_plugins(owner);
    let mut pairs = prepared
        .bind_stream_plugins(owner)
        .map(|binding| pairs(binding.references()))
        .unwrap_or_default();
    pairs.sort();
    pairs.dedup();
    pairs
}
pub(super) fn mark_http(owner: &mut EdgionPlugins, ctx: &HandlerContext) {
    let prepared = PreparedIpListReferences::from_plugins(owner);
    let _ = prepared.visit_plugins_mut(owner, |mut reference| {
        let denied = resolve(reference.token(), ctx).denied;
        reference.set_denied(denied);
    });
}
pub(super) fn mark_stream(owner: &mut EdgionStreamPlugins, ctx: &HandlerContext) {
    let prepared = PreparedIpListReferences::from_stream_plugins(owner);
    let _ = prepared.visit_stream_plugins_mut(owner, |mut reference| {
        let denied = resolve(reference.token(), ctx).denied;
        reference.set_denied(denied);
    });
}
pub(super) fn http_errors(owner: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let prepared = PreparedIpListReferences::from_plugins(owner);
    match prepared.bind_plugins(owner) {
        Ok(binding) => errors(binding.references(), ctx),
        Err(error) => vec![ResolvedRefsError::InvalidReference { error }],
    }
}
pub(super) fn stream_errors(owner: &EdgionStreamPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let prepared = PreparedIpListReferences::from_stream_plugins(owner);
    match prepared.bind_stream_plugins(owner) {
        Ok(binding) => errors(binding.references(), ctx),
        Err(error) => vec![ResolvedRefsError::InvalidReference { error }],
    }
}
#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/ip_list_refs_test.rs"]
mod tests;
