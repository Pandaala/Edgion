//! Owning fixed-local ConfigMap descriptor resolution and readonly status.
use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::{
    bound_configmap_status_result, fetch_bound_configmap, BoundFetchOutcome,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResolvedRefsError, ResourceRef};
use edgion_resources::resources::refs::plugin_fields::{
    BoundPluginEntryMut, DescriptorReferenceError, PreparedPluginReferences,
};
use edgion_resources::{
    grpc_transcoding::MAX_DESCRIPTOR_BYTES, resources::edgion_plugins::EdgionPlugin, EdgionPlugins,
};

pub(crate) fn clear(ep: &mut EdgionPlugins) {
    macro_rules! stage {
        ($entries:expr) => {
            for entry in $entries.iter_mut().flatten() {
                if let EdgionPlugin::GrpcJsonTranscoder(config) = &mut entry.plugin {
                    config.resolved_descriptor = None;
                    config.descriptor_error = None;
                }
            }
        };
    }
    stage!(ep.spec.request_plugins);
    stage!(ep.spec.upstream_response_filter_plugins);
    stage!(ep.spec.upstream_response_body_filter_plugins);
    stage!(ep.spec.upstream_response_plugins);
}

pub(crate) fn resolve(
    ep: &mut EdgionPlugins,
    prepared: &PreparedPluginReferences,
    consumer: &ResourceRef,
    ctx: &HandlerContext,
) {
    clear(ep);
    let Ok(mut bindings) = prepared.bind_mut(ep) else {
        return;
    };
    for entry in bindings.request_entries() {
        resolve_entry(entry, consumer, ctx);
    }
    for entry in bindings.response_filter_entries() {
        resolve_entry(entry, consumer, ctx);
    }
    for entry in bindings.response_body_filter_entries() {
        resolve_entry(entry, consumer, ctx);
    }
    for entry in bindings.response_entries() {
        resolve_entry(entry, consumer, ctx);
    }
}

fn resolve_entry(mut entry: BoundPluginEntryMut<'_>, consumer: &ResourceRef, ctx: &HandlerContext) {
    if !matches!(entry.config(), EdgionPlugin::GrpcJsonTranscoder(_)) {
        return;
    }
    let bytes = {
        let Ok(references) = entry.references() else {
            return;
        };
        let Some(selection) = references.grpc_descriptor() else {
            return;
        };
        match selection {
            Ok(selection) => match fetch_bound_configmap(ctx, consumer, *selection.reference()) {
                BoundFetchOutcome::Ok(configmap) => configmap
                    .binary_data
                    .and_then(|mut data| data.remove(selection.key()))
                    .map(|bytes| bytes.0),
                BoundFetchOutcome::Pending(_) | BoundFetchOutcome::Invalid(_) => None,
            },
            Err(_) => None,
        }
    };
    if let EdgionPlugin::GrpcJsonTranscoder(config) = entry.config_mut() {
        match bytes {
            Some(bytes) if !bytes.is_empty() && bytes.len() <= MAX_DESCRIPTOR_BYTES => {
                config.resolved_descriptor = Some(bytes);
                config.descriptor_error = config.compile().err();
            }
            _ => {
                config.descriptor_error =
                    Some("descriptor ConfigMap reference or binaryData is invalid, absent or over limit".into())
            }
        }
    }
}

pub(crate) fn status_errors(
    ep: &EdgionPlugins,
    consumer: &ResourceRef,
    ctx: &HandlerContext,
) -> Vec<ResolvedRefsError> {
    let prepared = PreparedPluginReferences::from_plugins(ep);
    let bindings = match prepared.bind(ep) {
        Ok(bindings) => bindings,
        Err(error) => return vec![ResolvedRefsError::InvalidReference { error }],
    };
    let mut errors = Vec::new();
    for entry in bindings
        .request_entries()
        .chain(bindings.response_filter_entries())
        .chain(bindings.response_body_filter_entries())
        .chain(bindings.response_entries())
    {
        let references = match entry.references() {
            Ok(references) => references,
            Err(error) => {
                errors.push(ResolvedRefsError::InvalidReference { error });
                continue;
            }
        };
        match references.grpc_descriptor() {
            Some(Ok(selection)) => {
                if let Err(error) = bound_configmap_status_result(ctx, consumer, *selection.reference()) {
                    errors.push(error);
                }
            }
            Some(Err(DescriptorReferenceError::Reference(error))) => {
                errors.push(ResolvedRefsError::InvalidReference { error })
            }
            // Selector shape is an intrinsic Accepted diagnostic, not object absence.
            Some(Err(DescriptorReferenceError::InvalidSelector)) | None => {}
        }
    }
    errors
}
