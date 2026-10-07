//! Store-only descriptor resolution using the existing ConfigMap dependency index.
use crate::conf_mgr::sync_runtime::resource_processor::{format_configmap_ref_key, HandlerContext, ResourceRef};
use edgion_resources::{
    grpc_transcoding::{GrpcDescriptorSource, MAX_DESCRIPTOR_BYTES},
    resources::edgion_plugins::EdgionPlugin,
    EdgionPlugins,
};

pub(crate) fn resolve(ep: &mut EdgionPlugins, consumer: &ResourceRef, ctx: &HandlerContext) {
    let namespace = ep.metadata.namespace.as_deref().unwrap_or("default");
    for entry in ep.spec.request_plugins.iter_mut().flatten() {
        let enabled = entry.is_enabled();
        let EdgionPlugin::GrpcJsonTranscoder(config) = &mut entry.plugin else {
            continue;
        };
        config.resolved_descriptor = None;
        config.descriptor_error = None;
        if !enabled {
            continue;
        }
        let GrpcDescriptorSource::ConfigMapRef { name, key } = &config.descriptor else {
            continue;
        };
        // Register even missing sources, so later creation repairs the generation.
        ctx.secret_ref_manager().add_ref(
            format_configmap_ref_key(Some(namespace.to_owned()).as_ref(), name),
            consumer.clone(),
        );
        let bytes = ctx
            .runtime()
            .configmap_store
            .get(Some(namespace), name)
            .and_then(|cm| cm.binary_data.and_then(|mut data| data.remove(key)))
            .map(|bytes| bytes.0);
        match bytes {
            Some(bytes) if !bytes.is_empty() && bytes.len() <= MAX_DESCRIPTOR_BYTES => {
                config.resolved_descriptor = Some(bytes);
                config.descriptor_error = config.compile().err();
            }
            _ => config.descriptor_error = Some("descriptor ConfigMap binaryData is absent or over limit".into()),
        }
    }
}
