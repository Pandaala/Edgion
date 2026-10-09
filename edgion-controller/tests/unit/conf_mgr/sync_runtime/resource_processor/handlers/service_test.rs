use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
use crate::conf_mgr::sync_runtime::resource_processor::service_ref::mirror_service_key;
use crate::conf_mgr::sync_runtime::resource_processor::{ResourceProcessor, ResourceRef};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::EdgionPlugins;
use edgion_resources::ResourceKind;
use kube::api::ObjectMeta;
use std::sync::Arc;

#[tokio::test]
async fn dns_resolver_grant_dependencies_replace_and_clear_on_delete() {
    let runtime = Arc::new(ControllerRuntime::default());
    let grant: edgion_resources::resources::ReferenceGrant = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "ReferenceGrant",
        "metadata": {"namespace": "dns", "name": "allow-service"},
        "spec": {
            "from": [{"group": "", "kind": "Service", "namespace": "app"}],
            "to": [{"group": "edgion.io", "kind": "LinkSys", "name": "aliyun"}]
        }
    }))
    .unwrap();
    runtime
        .ref_grant_store
        .upsert("dns/allow-service".to_string(), Arc::new(grant));
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let handler = ServiceHandler::new();
    let mut service = Service {
        metadata: ObjectMeta {
            namespace: Some("app".to_string()),
            name: Some("backend".to_string()),
            annotations: Some(std::collections::BTreeMap::from([(
                edgion_resources::constants::annotations::edgion::DNS_RESOLVER_REF.to_string(),
                "dns/aliyun".to_string(),
            )])),
            ..Default::default()
        },
        ..Default::default()
    };
    let expected_owner = ResourceRef::new(ResourceKind::Service, Some("app".to_string()), "backend".to_string());

    let ProcessResult::Continue(parsed) = handler.parse(service.clone(), &ctx).await else {
        panic!("permitted Service must continue");
    };
    assert!(parsed.metadata.annotations.as_ref().is_some_and(
        |annotations| annotations.contains_key(edgion_resources::constants::annotations::edgion::DNS_RESOLVER_REF)
    ));
    assert_eq!(runtime.cross_ns_ref_mgr.get_refs("dns"), vec![expected_owner.clone()]);

    service.metadata.annotations = None;
    let ProcessResult::Continue(_) = handler.parse(service.clone(), &ctx).await else {
        panic!("replacement Service must continue");
    };
    assert!(runtime.cross_ns_ref_mgr.get_refs("dns").is_empty());

    service.metadata.annotations = Some(std::collections::BTreeMap::from([(
        edgion_resources::constants::annotations::edgion::DNS_RESOLVER_REF.to_string(),
        "dns/aliyun".to_string(),
    )]));
    let ProcessResult::Continue(restored) = handler.parse(service, &ctx).await else {
        panic!("restored Service must continue");
    };
    assert_eq!(runtime.cross_ns_ref_mgr.get_refs("dns"), vec![expected_owner]);
    handler.on_delete(&restored, &ctx).await;
    assert!(runtime.cross_ns_ref_mgr.get_refs("dns").is_empty());
}

#[tokio::test]
async fn mirror_dependents_requeue_only_after_service_visibility_changes() {
    let runtime = Arc::new(ControllerRuntime::default());
    let service_processor = Arc::new(ResourceProcessor::<Service>::new(
        "Service",
        16,
        Arc::new(ServiceHandler::new()),
        runtime.clone(),
    ));
    runtime.processor_registry.register(service_processor.clone());
    let plugin_processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    runtime.processor_registry.register(plugin_processor.clone());

    let owner = ResourceRef::new(
        ResourceKind::EdgionPlugins,
        Some("default".to_string()),
        "mirrors".to_string(),
    );
    runtime
        .service_ref_mgr
        .add_ref(mirror_service_key("default", "shadow"), owner);
    let service = Service {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("shadow".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let handler = ServiceHandler::new();

    handler.on_change(&service, &ctx).await;
    assert!(plugin_processor.workqueue().is_empty());
    service_processor.save(service.clone(), None);
    assert!(super::super::route_utils::service_exists("default", "shadow", &runtime));
    handler.after_publish(&service, &ctx).await;
    let published = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        plugin_processor.workqueue().dequeue(),
    )
    .await
    .expect("publish should requeue the mirror consumer")
    .expect("plugin workqueue should remain open");
    assert_eq!(published.key, "default/mirrors");

    handler.on_delete(&service, &ctx).await;
    assert!(plugin_processor.workqueue().is_empty());
    service_processor.remove("default/shadow", None);
    assert!(!super::super::route_utils::service_exists(
        "default", "shadow", &runtime
    ));
    handler.after_unpublish(&service, &ctx).await;
    let unpublished = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        plugin_processor.workqueue().dequeue(),
    )
    .await
    .expect("unpublish should requeue the mirror consumer")
    .expect("plugin workqueue should remain open");
    assert_eq!(unpublished.key, "default/mirrors");
}

#[tokio::test]
async fn dns_raw_owner_default_namespace_retargets_and_cleans_denied_dependencies() {
    use edgion_resources::constants::annotations::edgion::DNS_RESOLVER_REF;
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let handler = ServiceHandler::new();
    let mut source = Service {
        metadata: ObjectMeta {
            name: Some("bare-owner".into()),
            annotations: Some(std::collections::BTreeMap::from([(
                DNS_RESOLVER_REF.into(),
                "dns/resolver".into(),
            )])),
            ..Default::default()
        },
        ..Default::default()
    };
    let owner = ResourceRef::new(ResourceKind::Service, None, "bare-owner".into());
    for target in ["dns/resolver", "other/resolver"] {
        source
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(DNS_RESOLVER_REF.into(), target.into());
        let ProcessResult::Continue(parsed) = handler.parse(source.clone(), &ctx).await else {
            panic!("parse")
        };
        assert!(!parsed
            .metadata
            .annotations
            .as_ref()
            .unwrap()
            .contains_key(DNS_RESOLVER_REF));
        let ns = target.split_once('/').unwrap().0;
        assert_eq!(runtime.cross_ns_ref_mgr.get_refs(ns), vec![owner.clone()]);
        if ns == "other" {
            assert!(runtime.cross_ns_ref_mgr.get_refs("dns").is_empty());
        }
    }
    handler.on_delete(&source, &ctx).await;
    assert!(runtime.cross_ns_ref_mgr.get_refs("other").is_empty());
    for target in ["/resolver", "dns/a/b", "bare"] {
        source
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(DNS_RESOLVER_REF.into(), target.into());
        let _ = handler.parse(source.clone(), &ctx).await;
        assert!(runtime.cross_ns_ref_mgr.get_refs("dns").is_empty());
    }
    source.metadata.namespace = Some("".into());
    source
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(DNS_RESOLVER_REF.into(), "dns/resolver".into());
    let ProcessResult::Continue(parsed) = handler.parse(source, &ctx).await else {
        panic!("parse")
    };
    assert!(!parsed
        .metadata
        .annotations
        .as_ref()
        .unwrap()
        .contains_key(DNS_RESOLVER_REF));
    assert!(runtime.cross_ns_ref_mgr.get_refs("dns").is_empty());
}
