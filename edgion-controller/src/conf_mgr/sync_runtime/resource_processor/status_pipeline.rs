//! Backend status projection without re-deserializing processed resource specs.

use std::any::Any;

use edgion_resources::{
    BackendTLSPolicy, EdgionAcme, EdgionBackend, EdgionBackendTrafficPolicy, EdgionConfigData, EdgionGatewayConfig,
    EdgionPlugins, EdgionStreamPlugins, EdgionTls, GRPCRoute, Gateway, GatewayClass, HTTPRoute, LinkSys, TCPRoute,
    TLSRoute, UDPRoute,
};
use serde_json::Value;

use super::status_observation::{ClassObservation, ObserverIdentity};
use crate::conf_mgr::ControllerRuntime;

pub(super) fn is_partitioned(kind: &str) -> bool {
    super::status_observation::is_partitioned_kind(kind)
}

pub(super) fn has_non_native_status<T: 'static>(obj: &T) -> bool {
    let obj = obj as &dyn Any;
    macro_rules! check {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_ref::<$ty>() {
                return resource.status.as_ref().is_some_and(|status| status.as_local().is_none());
            })+
        };
    }
    check!(EdgionPlugins, EdgionStreamPlugins, EdgionBackend, LinkSys);
    false
}

pub(super) fn observer_identity(runtime: &ControllerRuntime) -> Option<ObserverIdentity> {
    let identity = runtime.status_identity()?;
    let processor = runtime.processor_registry.get("GatewayClass")?;
    let raw = processor.get_raw_any(&identity.gateway_class_name)?;
    let class = raw.downcast_ref::<GatewayClass>()?;
    let uid = class.metadata.uid.as_ref().filter(|uid| !uid.is_empty())?;
    if class.spec.controller_name != identity.controller_name {
        return None;
    }
    Some(ObserverIdentity {
        controller_name: identity.controller_name,
        gateway_class_name: identity.gateway_class_name,
        gateway_class_uid: uid.clone(),
    })
}

pub(super) fn class_observations(runtime: &ControllerRuntime, status: Option<&Value>) -> Vec<ClassObservation> {
    let Some(processor) = runtime.processor_registry.get("GatewayClass") else {
        return Vec::new();
    };
    let Some(entries) = status.and_then(|s| s.get("controllers")).and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut facts = Vec::new();
    for entry in entries {
        let Some(name) = entry.get("gatewayClassName").and_then(Value::as_str) else {
            continue;
        };
        match processor.get_raw_any(name) {
            Some(raw) => {
                if let Some(class) = raw.downcast_ref::<GatewayClass>() {
                    if let Some(uid) = class.metadata.uid.as_ref().filter(|uid| !uid.is_empty()) {
                        facts.push(ClassObservation::Existing {
                            name: name.to_owned(),
                            uid: uid.clone(),
                            controller_name: class.spec.controller_name.clone(),
                        });
                    }
                }
            }
            None if runtime.gateway_classes_complete() => {
                facts.push(ClassObservation::Deleted { name: name.to_owned() });
            }
            None => {}
        }
    }
    facts
}

/// Only replace status: compiled regexes, resolved fields and unknown-field sidecars survive.
pub(super) fn replace_status<T: 'static>(obj: &mut T, status: Value) -> Result<(), serde_json::Error> {
    let obj = obj as &mut dyn Any;
    macro_rules! replace {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                resource.status = serde_json::from_value(status)?;
                return Ok(());
            })+
        };
    }
    replace!(
        EdgionPlugins,
        EdgionStreamPlugins,
        EdgionBackend,
        LinkSys,
        HTTPRoute,
        GRPCRoute,
        TCPRoute,
        TLSRoute,
        UDPRoute,
        EdgionTls,
        BackendTLSPolicy,
        EdgionBackendTrafficPolicy,
        Gateway,
        GatewayClass,
        EdgionGatewayConfig,
        EdgionConfigData,
        EdgionAcme
    );
    Ok(())
}

/// Incoming transport projections are never evidence of local ownership.
pub(super) fn clear_current_status<T: 'static>(obj: &mut T) {
    let obj = obj as &mut dyn Any;
    macro_rules! clear {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                resource.spec.current_status = None;
                return;
            })+
        };
    }
    clear!(
        HTTPRoute,
        GRPCRoute,
        TCPRoute,
        TLSRoute,
        UDPRoute,
        Gateway,
        GatewayClass,
        EdgionTls,
        BackendTLSPolicy,
        EdgionBackendTrafficPolicy,
        EdgionPlugins,
        EdgionStreamPlugins,
        EdgionBackend,
        LinkSys,
        EdgionGatewayConfig,
        EdgionConfigData,
        EdgionAcme
    );
}

/// Snapshot the working native status without round-tripping a compiled spec.
/// This runs before transport finalization so Gateway proofs use current status.
pub(super) fn capture_current_status<T: 'static>(obj: &mut T) {
    let obj = obj as &mut dyn Any;
    macro_rules! native {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                resource.spec.current_status = resource.status.clone();
                return;
            })+
        };
    }
    native!(
        HTTPRoute,
        GRPCRoute,
        TCPRoute,
        TLSRoute,
        UDPRoute,
        Gateway,
        GatewayClass,
        EdgionTls,
        BackendTLSPolicy,
        EdgionBackendTrafficPolicy,
        EdgionGatewayConfig,
        EdgionConfigData,
        EdgionAcme
    );
    macro_rules! partitioned {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                resource.spec.current_status = resource.status.as_ref().and_then(|status| status.as_local()).cloned();
                return;
            })+
        };
    }
    partitioned!(EdgionPlugins, EdgionStreamPlugins, EdgionBackend, LinkSys);
}

/// Even native FS/etcd status may retain foreign parents for diagnostics. Only
/// the identity overwritten by finalize_transport can select serving results.
pub(super) fn filter_current_route_status<T: 'static>(obj: &mut T) {
    let obj = obj as &mut dyn Any;
    macro_rules! filter {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                if let Some(status) = resource.spec.current_status.as_mut() {
                    let owner = resource.spec.resolved_status_controller.as_deref().filter(|owner| !owner.is_empty());
                    status.parents.retain(|parent| Some(parent.controller_name.as_str()) == owner);
                }
                return;
            })+
        };
    }
    filter!(HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute);
}

/// Native attachment handlers supply their configured owner during finalization.
/// API history stays complete while current status excludes foreign observations.
pub(super) fn filter_current_native_attachment_status<T: 'static>(obj: &mut T, configured_owner: &str) {
    let obj = obj as &mut dyn Any;
    let configured_owner = Some(configured_owner);
    let owner = configured_owner.filter(|owner| !owner.is_empty());
    if let Some(resource) = obj.downcast_mut::<EdgionTls>() {
        if let Some(status) = resource.spec.current_status.as_mut() {
            status
                .parents
                .retain(|parent| Some(parent.controller_name.as_str()) == owner);
        }
        return;
    }
    macro_rules! policies {
        ($($ty:ty),+ $(,)?) => {
            $(if let Some(resource) = obj.downcast_mut::<$ty>() {
                if let Some(status) = resource.spec.current_status.as_mut() {
                    status.ancestors.retain(|ancestor| Some(ancestor.controller_name.as_str()) == owner);
                }
                return;
            })+
        };
    }
    policies!(BackendTLSPolicy, EdgionBackendTrafficPolicy);
}

#[cfg(test)]
mod tests {
    use super::super::status_ownership::KubernetesStatusIdentity;
    use super::super::{HandlerContext, LocalRawStore, ProcessorHandler, ResourceProcessor, WorkItemResult};
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    struct LocalObservation(bool);

    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionPlugins> for LocalObservation {
        fn update_status(&self, obj: &mut EdgionPlugins, _ctx: &HandlerContext, _errors: &[String]) {
            assert!(obj.spec.current_status.is_none(), "incoming projection must be cleared");
            assert!(obj.status.as_ref().is_none_or(|status| status.as_local().is_some()));
            obj.status = Some(
                serde_json::from_value(json!({"conditions": [{
                    "type": "ResolvedRefs", "status": if self.0 { "True" } else { "False" },
                    "reason": "LocalView", "message": "Local reference visibility",
                    "observedGeneration": 1, "lastTransitionTime": "2026-09-21T00:00:00Z"
                }]}))
                .unwrap(),
            );
        }
    }

    struct ClassHandler;
    #[async_trait::async_trait]
    impl ProcessorHandler<GatewayClass> for ClassHandler {}

    struct NativeRouteHandler;
    macro_rules! native_route_handler {
        ($($ty:ty),+ $(,)?) => {$(
            #[async_trait::async_trait]
            impl ProcessorHandler<$ty> for NativeRouteHandler {
                fn filter(&self, obj: &$ty, _ctx: &HandlerContext) -> bool {
                    assert!(obj.spec.current_status.is_none());
                    true
                }

                fn finalize_transport(&self, obj: &mut $ty, _ctx: &HandlerContext) {
                    obj.spec.resolved_status_controller = Some("example/local".into());
                }
            }
        )+};
    }
    native_route_handler!(HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute);

    #[test]
    fn native_tls_and_policy_current_status_excludes_foreign_history() {
        use super::super::handlers::{BackendTlsPolicyHandler, EdgionBackendTrafficPolicyHandler, EdgionTlsHandler};
        use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
        let ctx = HandlerContext::new(
            None,
            None,
            TriggerChain::default(),
            3,
            20,
            Arc::new(ControllerRuntime::new()),
        );
        macro_rules! check {
            ($ty:ty, $handler:expr, $spec:expr, $list:literal, $reference:literal) => {{
                let mut source = json!({"metadata":{"name":"resource","namespace":"apps"}, "spec":$spec});
                source["status"] = json!({$list:[
                    {$reference:{"name":"local"},"controllerName":"example/local","conditions":[]},
                    {$reference:{"name":"foreign"},"controllerName":"example/foreign","conditions":[]}
                ]});
                let mut resource: $ty = serde_json::from_value(source.clone()).unwrap();
                capture_current_status(&mut resource);
                $handler.finalize_transport(&mut resource, &ctx);
                let output = serde_json::to_value(resource).unwrap();
                assert_eq!(output["status"], source["status"]);
                assert_eq!(output["spec"]["currentStatus"][$list].as_array().unwrap().len(), 1);
                assert_eq!(output["spec"]["currentStatus"][$list][0]["controllerName"], "example/local");
            }};
        }
        check!(
            EdgionTls,
            EdgionTlsHandler::new("example/local".into()),
            json!({"hosts":[],"secretRef":{"name":"certificate"}}),
            "parents",
            "parentRef"
        );
        check!(
            BackendTLSPolicy,
            BackendTlsPolicyHandler::new("example/local".into()),
            json!({"targetRefs":[],"validation":{"hostname":"backend.example.com"}}),
            "ancestors",
            "ancestorRef"
        );
        check!(
            EdgionBackendTrafficPolicy,
            EdgionBackendTrafficPolicyHandler::new("example/local".into()),
            json!({"targetRefs":[]}),
            "ancestors",
            "ancestorRef"
        );
    }

    #[tokio::test]
    async fn native_routes_keep_complete_history_but_only_current_owner_decisions() {
        let source = json!({
            "metadata":{"name":"route","namespace":"apps"},
            "spec":{"resolvedStatusController":"example/forged", "currentStatus":{"parents":[]}},
            "status":{"parents":[
                {"parentRef":{"name":"local"},"controllerName":"example/local","conditions":[]},
                {"parentRef":{"name":"foreign"},"controllerName":"example/foreign","conditions":[]}
            ]}
        });
        macro_rules! check {
            ($ty:ty, $kind:literal) => {{
                let processor = ResourceProcessor::new(
                    $kind,
                    8,
                    Arc::new(NativeRouteHandler),
                    Arc::new(ControllerRuntime::new()),
                );
                let raw: $ty = serde_json::from_value(source.clone()).unwrap();
                let WorkItemResult::Processed {
                    obj,
                    persisted_status,
                    ..
                } = processor.on_init_apply(raw, None).await
                else {
                    panic!("expected native route publication");
                };
                assert_eq!(
                    obj.spec.resolved_status_controller.as_deref(),
                    Some("example/local")
                );
                let current = obj.spec.current_status.as_ref().unwrap();
                assert_eq!(current.parents.len(), 1);
                assert_eq!(current.parents[0].parent_ref.name, "local");
                assert_eq!(obj.status.as_ref().unwrap().parents.len(), 2);
                assert_eq!(
                    serde_json::to_value(&obj.status).unwrap(),
                    persisted_status.unwrap()
                );
            }};
        }
        check!(HTTPRoute, "HTTPRoute");
        check!(GRPCRoute, "GRPCRoute");
        check!(TCPRoute, "TCPRoute");
        check!(TLSRoute, "TLSRoute");
        check!(UDPRoute, "UDPRoute");
    }

    fn deployment(class: &str, resolved: bool) -> Arc<ResourceProcessor<EdgionPlugins>> {
        let runtime = Arc::new(ControllerRuntime::new());
        runtime.set_status_identity(KubernetesStatusIdentity {
            controller_name: "example/controller".into(),
            gateway_class_name: class.into(),
        });
        let classes = Arc::new(ResourceProcessor::new(
            "GatewayClass",
            100,
            Arc::new(ClassHandler),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        for name in ["a", "b"] {
            let obj = serde_json::from_value(json!({
                "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass",
                "metadata": {"name": name, "uid": format!("uid-{name}")},
                "spec": {"controllerName": "example/controller"}
            }))
            .unwrap();
            raw.upsert(name.into(), Arc::new(obj));
        }
        classes.set_raw_store(raw);
        runtime.processor_registry.register(classes);
        Arc::new(ResourceProcessor::new(
            "EdgionPlugins",
            100,
            Arc::new(LocalObservation(resolved)),
            runtime,
        ))
    }

    #[tokio::test]
    async fn alternating_deployments_keep_opposite_local_status_and_converge() {
        let deployments = [deployment("a", true), deployment("b", false)];
        let mut source = json!({"apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
            "metadata": {"namespace": "apps", "name": "shared", "generation": 1}, "spec": {}});
        for turn in 0..8 {
            let index = turn % 2;
            let mut raw: EdgionPlugins = serde_json::from_value(source.clone()).unwrap();
            raw.spec.unknown_fields = vec!["spec.example".into()];
            let WorkItemResult::Processed {
                obj,
                status_changed,
                persisted_status,
            } = deployments[index].on_init_apply(raw, None).await
            else {
                panic!("expected publication")
            };
            let local = serde_json::to_value(obj.spec.current_status.as_ref().unwrap()).unwrap();
            assert!(local.get("controllers").is_none());
            assert_eq!(
                local["conditions"][0]["status"],
                if index == 0 { "True" } else { "False" }
            );
            assert_eq!(obj.spec.unknown_fields, ["spec.example"]);
            assert_eq!(
                status_changed,
                turn < 2,
                "steady alternating views must not write forever"
            );
            source["status"] = persisted_status.unwrap();
            assert_eq!(serde_json::to_value(&obj.status).unwrap(), source["status"]);
            assert_eq!(
                source["status"]["controllers"].as_array().unwrap().len(),
                (turn + 1).min(2)
            );
        }
    }

    #[tokio::test]
    async fn missing_owner_preserves_local_processing_without_flat_persistence() {
        let processor = deployment("missing", true);
        let raw: EdgionPlugins = serde_json::from_value(json!({"apiVersion": "edgion.io/v1",
            "kind": "EdgionPlugins", "metadata": {"name": "shared", "namespace": "apps"}, "spec": {}}))
        .unwrap();
        let WorkItemResult::Processed {
            obj,
            status_changed,
            persisted_status,
        } = processor.on_init_apply(raw, None).await
        else {
            panic!("expected local publication")
        };
        assert!(obj.status.is_none(), "no persisted status was observed");
        assert!(obj.spec.current_status.is_some());
        assert!(!status_changed);
        assert!(persisted_status.is_none());
    }

    #[tokio::test]
    async fn invalid_observations_do_not_discard_local_configuration() {
        for status in [
            json!({"controllers": "invalid"}),
            json!({"controllers": [], "conditions": []}),
            json!({"controllers": [{"controllerName": "example/controller"}]}),
        ] {
            let processor = deployment("a", true);
            let raw: EdgionPlugins = serde_json::from_value(json!({"apiVersion": "edgion.io/v1",
                "kind": "EdgionPlugins", "metadata": {"name": "shared", "namespace": "apps"},
                "spec": {"currentStatus": {"conditions": []}}, "status": status}))
            .unwrap();
            let WorkItemResult::Processed {
                obj,
                status_changed,
                persisted_status,
            } = processor.on_init_apply(raw, None).await
            else {
                panic!("expected local publication")
            };
            assert_eq!(serde_json::to_value(&obj.status).unwrap(), status);
            let local = serde_json::to_value(obj.spec.current_status.unwrap()).unwrap();
            assert_eq!(local["conditions"][0]["status"], "True");
            assert!(!status_changed);
            assert!(persisted_status.is_none());
            assert!(processor.get("apps/shared").is_some());
        }
    }

    #[tokio::test]
    async fn native_backend_recomputes_imported_envelope_without_adopting_foreign_status() {
        let processor = ResourceProcessor::new(
            "EdgionPlugins",
            100,
            Arc::new(LocalObservation(true)),
            Arc::new(ControllerRuntime::new()),
        );
        let raw: EdgionPlugins = serde_json::from_value(json!({"apiVersion": "edgion.io/v1",
            "kind": "EdgionPlugins", "metadata": {"name": "shared", "namespace": "apps"},
            "spec": {}, "status": {"controllers": []}}))
        .unwrap();
        let WorkItemResult::Processed {
            obj, persisted_status, ..
        } = processor.on_init_apply(raw, None).await
        else {
            panic!("expected publication")
        };
        assert!(obj.status.as_ref().unwrap().as_local().is_some());
        assert!(persisted_status.unwrap().get("controllers").is_none());
    }
}
