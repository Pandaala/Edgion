use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::{
    format_configmap_ref_key,
    frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind},
    HandlerContext, ProcessorHandler, ResourceProcessor, ResourceRef,
};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::ResourceKind;
use kube::api::ObjectMeta;
use std::sync::Arc;

struct GatewayNoop;

#[async_trait::async_trait]
impl ProcessorHandler<Gateway> for GatewayNoop {}

#[tokio::test]
async fn configmap_change_requeues_typed_material_dependent() {
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        100,
        Arc::new(GatewayNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let dependent = ResourceRef::new(
        ResourceKind::Gateway,
        Some("default".to_string()),
        "gateway".to_string(),
    );
    let namespace = "default".to_string();
    runtime
        .secret_ref_manager
        .add_ref(format_configmap_ref_key(Some(&namespace), "ca-bundle"), dependent);
    let configmap = ConfigMap {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("ca-bundle".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    ConfigMapHandler::new().on_change(&configmap, &ctx).await;

    let item = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.key, "default/gateway");
    processor.workqueue().done(&item.key);
}

#[tokio::test]
async fn configmap_delete_then_recreate_preserves_typed_edge_and_requeues_twice() {
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        100,
        Arc::new(GatewayNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let dependent = ResourceRef::new(
        ResourceKind::Gateway,
        Some("default".to_string()),
        "gateway".to_string(),
    );
    let source = FrontendCaSourceKey::new(FrontendCaSourceKind::ConfigMap, "default", "same-name");
    runtime.frontend_ca_ref_manager.add_ref(source.index_key(), dependent);
    let configmap = ConfigMap {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("same-name".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    ConfigMapHandler::new().on_delete(&configmap, &ctx).await;
    let first = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.key, "default/gateway");
    processor.workqueue().done(&first.key);
    assert_eq!(runtime.frontend_ca_ref_manager.get_refs(&source.index_key()).len(), 1);

    ConfigMapHandler::new().on_change(&configmap, &ctx).await;
    let second = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.key, "default/gateway");
    processor.workqueue().done(&second.key);
}

#[tokio::test]
async fn descriptor_configmap_events_recover_exact_raw_owner_and_notify_route_gateway_workers() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::{
        EdgionPluginsHandler, GatewayHandler, HttpRouteHandler,
    };
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, RawStore};
    use edgion_resources::resources::edgion_plugins::EdgionPlugin;
    use edgion_resources::{EdgionPlugins, HTTPRoute};
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;
    macro_rules! work {
        ($processor:expr, $raw:expr, $key:expr) => {{
            let queue = $processor.workqueue();
            let item = tokio::time::timeout(std::time::Duration::from_secs(3), queue.dequeue())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item.key, $key);
            let source = $raw.get(&item.key).map(|source| (*source).clone());
            $processor
                .process_work_item(&item.key, source, None, item.trigger_chain)
                .await;
            queue.done(&item.key);
        }};
    }
    for namespace in [None, Some("default")] {
        let runtime = Arc::new(ControllerRuntime::default());
        let plugins = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            32,
            Arc::new(EdgionPluginsHandler),
            runtime.clone(),
        ));
        let plugin_raw = Arc::new(LocalRawStore::new());
        plugins.set_raw_store(plugin_raw.clone());
        runtime.processor_registry.register(plugins.clone());
        let configmaps = Arc::new(ResourceProcessor::<ConfigMap>::new(
            "ConfigMap",
            32,
            Arc::new(ConfigMapHandler::new()),
            runtime.clone(),
        ));
        let configmap_raw = Arc::new(LocalRawStore::new());
        configmaps.set_raw_store(configmap_raw.clone());
        runtime.processor_registry.register(configmaps.clone());
        let routes = Arc::new(ResourceProcessor::<HTTPRoute>::new(
            "HTTPRoute",
            32,
            Arc::new(HttpRouteHandler::default()),
            runtime.clone(),
        ));
        let route_raw = Arc::new(LocalRawStore::new());
        routes.set_raw_store(route_raw.clone());
        runtime.processor_registry.register(routes.clone());
        let gateways = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            32,
            Arc::new(GatewayHandler::new(None, None)),
            runtime.clone(),
        ));
        let gateway_raw = Arc::new(LocalRawStore::new());
        gateways.set_raw_store(gateway_raw.clone());
        runtime.processor_registry.register(gateways.clone());
        let gateway: Gateway = serde_json::from_value(serde_json::json!({"metadata":{"name":"descriptor-gateway","namespace":"default"},"spec":{"gatewayClassName":"edgion","listeners":[{"name":"http","protocol":"HTTP","port":80}]}})).unwrap();
        gateway_raw.upsert("default/descriptor-gateway".into(), Arc::new(gateway));
        gateways.on_event_key("default/descriptor-gateway".into()).await;
        work!(gateways, gateway_raw, "default/descriptor-gateway");
        let mut plugin: EdgionPlugins = serde_json::from_value(serde_json::json!({"metadata":{"name":"descriptor-plugin","namespace":namespace,"uid":"first","generation":1},"spec":{"requestPlugins":[{"type":"GrpcJsonTranscoder","config":{"descriptor":{"type":"configMapRef","name":"schema","key":"test.pb"},"services":["test.TestService"]}}]}})).unwrap();
        let owner_key = if namespace.is_some() {
            "default/descriptor-plugin"
        } else {
            "descriptor-plugin"
        };
        plugin_raw.upsert(owner_key.into(), Arc::new(plugin.clone()));
        plugins.on_event_key(owner_key.into()).await;
        work!(plugins, plugin_raw, owner_key);
        let owner = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            namespace.map(str::to_string),
            "descriptor-plugin".into(),
        );
        assert_eq!(
            runtime.secret_ref_manager.get_refs("ConfigMap/default/schema"),
            vec![owner.clone()]
        );
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({"metadata":{"name":"descriptor-route","namespace":"default"},"spec":{"parentRefs":[{"name":"descriptor-gateway"}],"rules":[{"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"descriptor-plugin"}}]}]}})).unwrap();
        route_raw.upsert("default/descriptor-route".into(), Arc::new(route));
        routes.on_event_key("default/descriptor-route".into()).await;
        work!(routes, route_raw, "default/descriptor-route");
        work!(gateways, gateway_raw, "default/descriptor-gateway");
        let descriptor = include_bytes!("../../../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb");
        for (generation, present) in [(2, true), (3, true), (4, false), (5, true)] {
            plugin.metadata.uid = Some(format!("plugin-{generation}"));
            plugin.metadata.generation = Some(generation);
            // Coalesced replacement: the dependency event must fetch the current raw UID,
            // never the processed generation or a fabricated default/name alias.
            plugin_raw.remove(owner_key);
            plugin_raw.upsert(owner_key.into(), Arc::new(plugin.clone()));
            let updated_bytes = if generation == 3 {
                vec![0xff]
            } else {
                descriptor.to_vec()
            };
            if present {
                let cm = ConfigMap {
                    metadata: ObjectMeta {
                        name: Some("schema".into()),
                        namespace: Some("default".into()),
                        uid: Some(format!("schema-{generation}")),
                        resource_version: Some(generation.to_string()),
                        ..Default::default()
                    },
                    binary_data: Some(BTreeMap::from([("test.pb".into(), ByteString(updated_bytes.clone()))])),
                    ..Default::default()
                };
                configmap_raw.upsert("default/schema".into(), Arc::new(cm));
            } else {
                configmap_raw.remove("default/schema");
            }
            configmaps.on_event_key("default/schema".into()).await;
            work!(configmaps, configmap_raw, "default/schema");
            work!(plugins, plugin_raw, owner_key);
            let published = plugins.get(owner_key).unwrap();
            assert_eq!(published.metadata.uid, plugin.metadata.uid);
            assert_eq!(published.metadata.generation, Some(generation));
            let EdgionPlugin::GrpcJsonTranscoder(config) = &published.spec.request_plugins.as_ref().unwrap()[0].plugin
            else {
                panic!()
            };
            assert_eq!(
                config.resolved_descriptor.as_deref(),
                present.then_some(updated_bytes.as_slice())
            );
            assert_eq!(config.compile().is_ok(), present && generation != 3);
            let accepted = published
                .status
                .as_ref()
                .unwrap()
                .as_local()
                .unwrap()
                .conditions
                .iter()
                .find(|condition| condition.type_ == "Accepted")
                .unwrap();
            assert_eq!(accepted.status == "True", present && generation != 3);
            assert_eq!(
                runtime.secret_ref_manager.get_refs("ConfigMap/default/schema"),
                vec![owner.clone()]
            );
            work!(routes, route_raw, "default/descriptor-route");
            work!(gateways, gateway_raw, "default/descriptor-gateway");
            let published_route = routes.get("default/descriptor-route").unwrap();
            assert!(runtime
                .plugin_ref_mgr
                .get_refs("default/descriptor-plugin")
                .contains(&ResourceRef::new(
                    ResourceKind::HTTPRoute,
                    Some("default".into()),
                    "descriptor-route".into()
                )));
            if namespace.is_some() {
                let references = edgion_resources::resources::refs::route_fields::HttpRouteExtensionRefs::from_route(
                    &published_route,
                )
                .unwrap();
                let reference = references
                    .effective_at(0, None, 0)
                    .unwrap()
                    .validated()
                    .unwrap()
                    .unwrap();
                let target = reference.target();
                let target_key = format!("{}/{}", target.scope().namespace().unwrap(), target.name());
                assert_eq!(target_key, owner_key);
                assert_eq!(plugin_raw.get(&target_key).unwrap().metadata.uid, plugin.metadata.uid);
                // HTTP rule discovery registers this edge without reading plugin material.
                // Route conditions remain independent of EP Accepted; Gateway compilation
                // owns descriptor fail-closed behavior, tested in the publication tests.
                let conditions = &published_route.status.as_ref().unwrap().parents[0].conditions;
                for kind in ["Accepted", "ResolvedRefs"] {
                    assert_eq!(
                        conditions
                            .iter()
                            .find(|condition| condition.type_ == kind)
                            .unwrap()
                            .status,
                        "True"
                    );
                }
            }
            let published_gateway = gateways.get("default/descriptor-gateway").unwrap();
            if namespace.is_some() {
                assert_eq!(
                    published_gateway.status.as_ref().unwrap().listeners.as_ref().unwrap()[0].attached_routes,
                    1
                );
            }
            if namespace.is_none() {
                assert!(plugin_raw.get("default/descriptor-plugin").is_none());
                // Notification is proved; semantic ExtensionRef lookup of raw None remains R4.
            }
        }
        let EdgionPlugin::GrpcJsonTranscoder(config) = &mut plugin.spec.request_plugins.as_mut().unwrap()[0].plugin
        else {
            panic!()
        };
        config.descriptor = edgion_resources::grpc_transcoding::GrpcDescriptorSource::ConfigMapRef {
            name: "replacement-schema".into(),
            key: "next.pb".into(),
        };
        plugin_raw.upsert(owner_key.into(), Arc::new(plugin.clone()));
        plugins.on_event_key(owner_key.into()).await;
        work!(plugins, plugin_raw, owner_key);
        assert!(runtime
            .secret_ref_manager
            .get_refs("ConfigMap/default/schema")
            .is_empty());
        assert_eq!(
            runtime
                .secret_ref_manager
                .get_refs("ConfigMap/default/replacement-schema"),
            vec![owner.clone()]
        );
        // Disabled and dropped entries replace dependency side effects through the worker.
        plugin.spec.request_plugins.as_mut().unwrap()[0].enable = false;
        plugin_raw.upsert(owner_key.into(), Arc::new(plugin.clone()));
        plugins.on_event_key(owner_key.into()).await;
        work!(plugins, plugin_raw, owner_key);
        assert!(runtime
            .secret_ref_manager
            .get_refs("ConfigMap/default/schema")
            .is_empty());
        assert!(runtime
            .secret_ref_manager
            .get_refs("ConfigMap/default/replacement-schema")
            .is_empty());
        let published = plugins.get(owner_key).unwrap();
        let EdgionPlugin::GrpcJsonTranscoder(config) = &published.spec.request_plugins.as_ref().unwrap()[0].plugin
        else {
            panic!()
        };
        assert!(config.resolved_descriptor.is_none() && config.descriptor_error.is_none());
        plugin.spec.request_plugins = None;
        plugin_raw.upsert(owner_key.into(), Arc::new(plugin));
        plugins.on_event_key(owner_key.into()).await;
        work!(plugins, plugin_raw, owner_key);
        assert!(runtime.secret_ref_manager.get_dependencies(&owner.key()).is_empty());
    }
}

#[tokio::test]
async fn native_gateway_configmap_events_recover_missing_ambiguous_and_deleted_material() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, RawStore};
    use edgion_resources::resources::gateway::FrontendCAResolutionError;
    use edgion_resources::{EdgionPlugins, Gateway};
    use std::collections::{BTreeMap, HashMap, HashSet};
    struct RawDependent;
    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionPlugins> for RawDependent {}
    macro_rules! work {
        ($processor:expr, $raw:expr, $key:expr) => {{
            let queue = $processor.workqueue();
            let item = tokio::time::timeout(std::time::Duration::from_secs(3), queue.dequeue())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item.key, $key);
            let source = $raw.get(&item.key).map(|value| (*value).clone());
            $processor
                .process_work_item(&item.key, source, None, item.trigger_chain)
                .await;
            queue.done(&item.key);
        }};
    }
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["server.example.com".to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let pem = params.self_signed(&key).unwrap().pem();
    for owner_namespace in [None, Some("default")] {
        let runtime = Arc::new(ControllerRuntime::default());
        let sources = Arc::new(ResourceProcessor::<ConfigMap>::new(
            "ConfigMap",
            32,
            Arc::new(ConfigMapHandler::new()),
            runtime.clone(),
        ));
        let source_raw = Arc::new(LocalRawStore::new());
        sources.set_raw_store(source_raw.clone());
        runtime.processor_registry.register(sources.clone());
        let gateways = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            32,
            Arc::new(GatewayHandler::new(None, None)),
            runtime.clone(),
        ));
        let gateway_raw = Arc::new(LocalRawStore::new());
        gateways.set_raw_store(gateway_raw.clone());
        runtime.processor_registry.register(gateways.clone());
        let plugins = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            32,
            Arc::new(RawDependent),
            runtime.clone(),
        ));
        runtime.processor_registry.register(plugins.clone());
        let owner_key = if owner_namespace.is_some() {
            "default/edge"
        } else {
            "edge"
        };
        let value = serde_json::json!({"metadata":{"name":"edge","namespace":owner_namespace},"spec":{"gatewayClassName":"edge",
            "listeners":[{"name":"https","protocol":"HTTPS","port":443,"tls":{"mode":"Terminate"}}],
            "tls":{"frontend":{"default":{"validation":{"caCertificateRefs":[{"group":"","kind":"ConfigMap","name":"ca"}]}}}}}});

        let gateway: Gateway = serde_json::from_value(value).unwrap();
        gateway_raw.upsert(owner_key.into(), Arc::new(gateway));
        gateways.on_event_key(owner_key.into()).await;
        work!(gateways, gateway_raw, owner_key);
        let first = gateways.get(owner_key).unwrap();
        assert_eq!(
            first.spec.listeners.as_ref().unwrap()[0]
                .tls
                .as_ref()
                .unwrap()
                .resolved_frontend_ca_refs
                .as_ref()
                .unwrap()[0]
                .error,
            Some(FrontendCAResolutionError::NotFound)
        );
        let dependent = ResourceRef::new(ResourceKind::Gateway, owner_namespace.map(str::to_owned), "edge".into());
        let typed = FrontendCaSourceKey::new(FrontendCaSourceKind::ConfigMap, "default", "ca").index_key();
        assert_eq!(
            runtime.frontend_ca_ref_manager.get_refs(&typed),
            std::slice::from_ref(&dependent)
        );
        let wrong = FrontendCaSourceKey::new(FrontendCaSourceKind::Secret, "default", "ca").index_key();
        assert!(runtime.frontend_ca_ref_manager.get_refs(&wrong).is_empty());
        // A different material kind with the same namespace/name is not a candidate.
        runtime.secret_store.update(
            HashMap::from([("default/ca".into(), k8s_openapi::api::core::v1::Secret::default())]),
            &HashSet::new(),
        );
        for (step, raw_key, present, error) in [
            (1, "ca", true, None),
            (2, "default/ca", true, Some(FrontendCAResolutionError::InvalidReference)),
            (3, "ca", false, None),
            (4, "default/ca", false, Some(FrontendCAResolutionError::NotFound)),
            (5, "ca", true, None),
        ] {
            if present {
                let material = ConfigMap {
                    metadata: ObjectMeta {
                        name: Some("ca".into()),
                        namespace: raw_key.contains('/').then(|| "default".into()),
                        uid: Some(format!("material-{step}")),
                        resource_version: Some(step.to_string()),
                        ..Default::default()
                    },
                    data: Some(BTreeMap::from([("ca.crt".into(), pem.clone())])),
                    ..Default::default()
                };
                source_raw.upsert(raw_key.into(), Arc::new(material));
            } else {
                source_raw.remove(raw_key);
            }
            // Original raw-key observers remain notified; semantic-only non-Gateway observers do not gain a bare alias.
            let raw_dependent = ResourceRef::new(ResourceKind::EdgionPlugins, Some("default".into()), "raw".into());
            let semantic_dependent =
                ResourceRef::new(ResourceKind::EdgionPlugins, Some("default".into()), "semantic".into());
            runtime
                .secret_ref_manager
                .add_ref(format!("ConfigMap/{raw_key}"), raw_dependent.clone());
            if raw_key == "ca" {
                runtime
                    .secret_ref_manager
                    .add_ref("ConfigMap/default/ca".into(), semantic_dependent.clone());
            }
            sources.on_event_key(raw_key.into()).await;
            work!(sources, source_raw, raw_key);
            work!(gateways, gateway_raw, owner_key);
            let published = gateways.get(owner_key).unwrap();
            assert_eq!(published.metadata.namespace.as_deref(), owner_namespace);
            let tls = published.spec.listeners.as_ref().unwrap()[0].tls.as_ref().unwrap();
            let outcome = &tls.resolved_frontend_ca_refs.as_ref().unwrap()[0];
            assert_eq!(outcome.error, error, "step {step}");
            assert_eq!(outcome.ca_certificate.is_some(), error.is_none());
            assert_eq!(outcome.source.namespace, "default");
            assert!(runtime.secret_ref_manager.get_dependencies(&dependent.key()).is_empty());
            let item = tokio::time::timeout(std::time::Duration::from_secs(3), plugins.workqueue().dequeue())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item.key, "default/raw");
            plugins.workqueue().done(&item.key);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), plugins.workqueue().dequeue())
                    .await
                    .is_err()
            );
            runtime.secret_ref_manager.clear_value_refs(&raw_dependent);
            runtime.secret_ref_manager.clear_value_refs(&semantic_dependent);
        }
        assert!(runtime.frontend_ca_ref_manager.get_refs(&typed).contains(&dependent));
    }
}
