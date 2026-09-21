//! Backend status projection without re-deserializing processed resource specs.

use std::any::Any;

use edgion_resources::{
    BackendTLSPolicy, EdgionBackend, EdgionBackendTrafficPolicy, EdgionPlugins, EdgionStreamPlugins, EdgionTls,
    GRPCRoute, GatewayClass, HTTPRoute, LinkSys, TCPRoute, TLSRoute, UDPRoute,
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
        EdgionBackendTrafficPolicy
    );
    Ok(())
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
            let local = serde_json::to_value(obj.status.as_ref().unwrap()).unwrap();
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
        assert!(obj.status.is_some());
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
                "spec": {}, "status": status}))
            .unwrap();
            let WorkItemResult::Processed {
                obj,
                status_changed,
                persisted_status,
            } = processor.on_init_apply(raw, None).await
            else {
                panic!("expected local publication")
            };
            let local = serde_json::to_value(obj.status.unwrap()).unwrap();
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
