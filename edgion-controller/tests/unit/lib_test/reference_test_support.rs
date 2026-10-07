//! Actual owning resources for frozen-reference and async publication fixtures.
use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ProcessorHandler, ResourceProcessor};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::edgion_plugins::EdgionPlugins;
use edgion_resources::resources::link_sys::LinkSys;
use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
use edgion_resources::resources::refs::FrozenFieldReference;
use std::sync::Arc;

pub(crate) fn injector_owner(
    namespace: &str,
    name: &str,
    uid: &str,
    generation: i64,
    source_key: &str,
) -> EdgionPlugins {
    let (source_namespace, source_name) = source_key.split_once('/').expect("namespaced source key");
    serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": { "namespace": namespace, "name": name, "uid": uid, "generation": generation },
        "spec": { "requestPlugins": [{ "type": "CredentialInjector", "config": {
            "credentialRef": { "group": "edgion.io", "kind": "LinkSys", "namespace": source_namespace, "name": source_name }
        }}] }
    })).expect("typed injector owner")
}

pub(crate) fn injector_reference(owner: &EdgionPlugins) -> FrozenFieldReference {
    let prepared = PreparedPluginReferences::from_plugins(owner);
    let bound = prepared.bind(owner).expect("actual owner");
    let entry = bound.request_entries().next().expect("injector entry");
    let references = entry.references().expect("current entry");
    references
        .credential()
        .expect("credential field")
        .freeze()
        .expect("valid reference")
}

struct SourceHandler;
#[async_trait::async_trait]
impl ProcessorHandler<LinkSys> for SourceHandler {}
struct ConsumerHandler;
#[async_trait::async_trait]
impl ProcessorHandler<EdgionPlugins> for ConsumerHandler {}

pub(crate) struct OwnerStores {
    pub sources: Arc<LocalRawStore<LinkSys>>,
    pub consumers: Arc<LocalRawStore<EdgionPlugins>>,
}

pub(crate) fn install_owner_stores(runtime: &Arc<ControllerRuntime>) -> OwnerStores {
    let sources = Arc::new(LocalRawStore::new());
    let source = Arc::new(ResourceProcessor::<LinkSys>::new(
        "LinkSys",
        16,
        Arc::new(SourceHandler),
        runtime.clone(),
    ));
    source.set_raw_store(sources.clone());
    runtime.processor_registry.register(source);
    let consumers = Arc::new(LocalRawStore::new());
    let consumer = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(ConsumerHandler),
        runtime.clone(),
    ));
    consumer.set_raw_store(consumers.clone());
    runtime.processor_registry.register(consumer);
    OwnerStores { sources, consumers }
}
