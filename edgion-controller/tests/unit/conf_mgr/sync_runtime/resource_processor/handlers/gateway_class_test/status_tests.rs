//! Deployment-local parameter validation and shared EGC convergence regressions.
use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::{EdgionGatewayConfigHandler, EdgionPluginsHandler};
use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, RawStore, ResourceProcessor, WorkItemResult};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::edgion_gateway_config::{EdgionGatewayConfigSpec, PluginReference};
use edgion_resources::resources::gateway_class::{GatewayClassSpec, ParametersReference};
use std::sync::Arc;
use std::time::Duration;

struct Deployment {
    egc: Arc<ResourceProcessor<EdgionGatewayConfig>>,
    gc: Arc<ResourceProcessor<GatewayClass>>,
    plugins: Arc<ResourceProcessor<EdgionPlugins>>,
    egc_raw: Arc<LocalRawStore<EdgionGatewayConfig>>,
    gc_raw: Arc<LocalRawStore<GatewayClass>>,
    plugins_raw: Arc<LocalRawStore<EdgionPlugins>>,
    class: String,
}

fn config() -> EdgionGatewayConfig {
    EdgionGatewayConfig::new(
        "shared",
        EdgionGatewayConfigSpec {
            current_status: None,
            global_plugins_ref: Some(vec![PluginReference {
                name: "ids".into(),
                namespace: Some("team-a".into()),
            }]),
            ..Default::default()
        },
    )
}

fn plugins(source: &str) -> EdgionPlugins {
    let source = if source == "ctx" {
        serde_json::json!({"type": "ctx", "name": "business"})
    } else {
        serde_json::json!({"type": source})
    };
    serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"namespace": "team-a", "name": "ids"},
        "spec": {"requestPlugins": [{"type": "RequestId", "config": {
            "ids": [{"header": "x-business-id", "source": source}]
        }}]}
    }))
    .unwrap()
}

impl Deployment {
    async fn new(class: &str, source: Option<&str>) -> Self {
        let runtime = Arc::new(ControllerRuntime::default());
        let egc = Arc::new(ResourceProcessor::new(
            "EdgionGatewayConfig",
            16,
            Arc::new(EdgionGatewayConfigHandler::new()),
            runtime.clone(),
        ));
        let gc = Arc::new(ResourceProcessor::new(
            "GatewayClass",
            16,
            Arc::new(GatewayClassHandler::new(GatewayClassOwnership::new(
                DEFAULT_CONTROLLER_NAME.into(),
            ))),
            runtime.clone(),
        ));
        let plugins_processor = Arc::new(ResourceProcessor::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler::new()),
            runtime.clone(),
        ));
        let egc_raw = Arc::new(LocalRawStore::new());
        let gc_raw = Arc::new(LocalRawStore::new());
        let plugins_raw = Arc::new(LocalRawStore::new());
        egc.set_raw_store(egc_raw.clone());
        gc.set_raw_store(gc_raw.clone());
        plugins_processor.set_raw_store(plugins_raw.clone());
        runtime.processor_registry.register(egc.clone());
        runtime.processor_registry.register(gc.clone());
        runtime.processor_registry.register(plugins_processor.clone());
        if let Some(source) = source {
            plugins_raw.upsert("team-a/ids".into(), Arc::new(plugins(source)));
        }
        egc_raw.upsert("shared".into(), Arc::new(config()));
        let mut class_obj = GatewayClass::new(
            class,
            GatewayClassSpec {
                current_status: None,
                controller_name: DEFAULT_CONTROLLER_NAME.into(),
                description: None,
                parameters_ref: Some(ParametersReference {
                    group: "edgion.io".into(),
                    kind: "EdgionGatewayConfig".into(),
                    name: "shared".into(),
                    namespace: None,
                }),
            },
        );
        class_obj.metadata.uid = Some(format!("uid-{class}"));
        gc_raw.upsert(class.into(), Arc::new(class_obj.clone()));
        let result = gc.on_init_apply(class_obj, None).await;
        if let WorkItemResult::Processed { obj, .. } = result {
            gc_raw.upsert(class.into(), Arc::new(obj));
        }
        Self {
            egc,
            gc,
            plugins: plugins_processor,
            egc_raw,
            gc_raw,
            plugins_raw,
            class: class.into(),
        }
    }

    async fn process_egc(&self, obj: EdgionGatewayConfig) -> (EdgionGatewayConfig, bool) {
        self.egc_raw.upsert("shared".into(), Arc::new(obj.clone()));
        self.egc.on_event_key("shared".into()).await;
        let item = tokio::time::timeout(Duration::from_secs(5), self.egc.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.key, "shared");
        let result = self
            .egc
            .process_work_item(&item.key, Some(obj), None, item.trigger_chain)
            .await;
        self.egc.workqueue().done(&item.key);
        match result {
            WorkItemResult::Processed {
                obj, status_changed, ..
            } => {
                self.egc_raw.upsert("shared".into(), Arc::new(obj.clone()));
                (obj, status_changed)
            }
            _ => panic!("EGC must be processed"),
        }
    }

    async fn drain_class(&self) -> bool {
        let item = tokio::time::timeout(Duration::from_secs(5), self.gc.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.key, self.class);
        let raw = (*self.gc_raw.get(&item.key).unwrap()).clone();
        let result = self
            .gc
            .process_work_item(&item.key, Some(raw), None, item.trigger_chain)
            .await;
        self.gc.workqueue().done(&item.key);
        match result {
            WorkItemResult::Processed {
                obj, status_changed, ..
            } => {
                self.gc_raw.upsert(item.key, Arc::new(obj));
                status_changed
            }
            _ => panic!("owned class must be processed"),
        }
    }

    fn assert_class(&self, accepted: bool) {
        let class = self.gc.get(&self.class).unwrap();
        let conditions = &class.status.unwrap().conditions;
        let accepted_condition = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted_condition.status, if accepted { "True" } else { "False" });
        assert_eq!(
            accepted_condition.reason,
            if accepted { "Accepted" } else { "InvalidParameters" }
        );
        if !accepted {
            assert!(accepted_condition.message.contains("globalPluginsRef team-a/ids"));
        }
        assert_eq!(
            conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap().status,
            "True"
        );
    }
}

#[tokio::test]
async fn shared_egc_status_converges_with_different_namespace_visibility() {
    let visible = Deployment::new("visible", Some("ctx")).await;
    let hidden = Deployment::new("hidden", None).await;
    let mut shared = config();
    for pass in 0..8 {
        let deployment = if pass % 2 == 0 { &visible } else { &hidden };
        let (next, changed) = deployment.process_egc(shared).await;
        assert_eq!(changed, pass == 0, "shared EGC diverged on pass {pass}");
        shared = next;
        deployment.drain_class().await;
        deployment.assert_class(pass % 2 != 0);
    }
    assert_eq!(shared.status.unwrap().conditions[0].status, "True");
}

/// Replacement for the former EGC test: the same raw-generation transitions
/// are now checked on the consuming class, plus the real dependency queue chain.
#[tokio::test]
async fn global_request_id_validation_observes_replaced_raw_generation() {
    let deployment = Deployment::new("edgion", None).await;
    let (mut shared, _) = deployment.process_egc(config()).await;
    deployment.drain_class().await;
    for source in [Some("traceId"), Some("ctx"), Some("traceId"), None, Some("ctx")] {
        let plugin = source.map(plugins);
        if let Some(plugin) = &plugin {
            deployment
                .plugins_raw
                .upsert("team-a/ids".into(), Arc::new(plugin.clone()));
        } else {
            deployment.plugins_raw.remove("team-a/ids");
        }
        deployment.plugins.on_event_key("team-a/ids".into()).await;
        let item = deployment.plugins.workqueue().dequeue().await.unwrap();
        deployment
            .plugins
            .process_work_item(&item.key, plugin, None, item.trigger_chain)
            .await;
        deployment.plugins.workqueue().done(&item.key);

        // Do not manually enqueue EGC: this verifies the plugin reference edge.
        let item = tokio::time::timeout(Duration::from_secs(5), deployment.egc.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.key, "shared");
        let result = deployment
            .egc
            .process_work_item(&item.key, Some(shared.clone()), None, item.trigger_chain)
            .await;
        deployment.egc.workqueue().done(&item.key);
        match result {
            WorkItemResult::Processed {
                obj, status_changed, ..
            } => {
                assert!(!status_changed, "plugin changes cannot change shared EGC status");
                shared = obj;
                deployment.egc_raw.upsert("shared".into(), Arc::new(shared.clone()));
            }
            _ => panic!("plugin dependency must reconcile EGC"),
        }
        deployment.drain_class().await;
        deployment.assert_class(source != Some("ctx"));
        // A status-only class WATCH echo must not request another write.
        let raw = (*deployment.gc_raw.get(&deployment.class).unwrap()).clone();
        let echo = deployment
            .gc
            .process_work_item(&deployment.class, Some(raw), None, TriggerChain::default())
            .await;
        assert!(matches!(
            echo,
            WorkItemResult::Processed {
                status_changed: false,
                ..
            }
        ));
        let (next, changed) = deployment.process_egc(shared).await;
        assert!(!changed);
        shared = next;
        assert!(!deployment.drain_class().await, "EGC echo must not cause a class write");
    }
}

#[tokio::test]
async fn bare_plugin_event_requeues_global_owner_and_class_reports_collision() {
    let deployment = Deployment::new("edgion", None).await;
    let mut shared = config();
    shared.spec.global_plugins_ref.as_mut().unwrap()[0].namespace = None;
    let (shared, _) = deployment.process_egc(shared).await;
    deployment.drain_class().await;
    for (raw_key, namespace) in [("ids", None), ("default/ids", Some("default"))] {
        let mut plugin = EdgionPlugins::new("ids", Default::default());
        plugin.metadata.namespace = namespace.map(str::to_owned);
        deployment.plugins_raw.upsert(raw_key.into(), Arc::new(plugin.clone()));
        deployment.plugins.on_event_key(raw_key.into()).await;
        let item = deployment.plugins.workqueue().dequeue().await.unwrap();
        assert_eq!(item.key, raw_key, "queue retains exact raw source identity");
        deployment
            .plugins
            .process_work_item(&item.key, Some(plugin), None, item.trigger_chain)
            .await;
        deployment.plugins.workqueue().done(&item.key);
        let item = tokio::time::timeout(Duration::from_secs(5), deployment.egc.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.key, "shared");
        deployment
            .egc
            .process_work_item(&item.key, Some(shared.clone()), None, item.trigger_chain)
            .await;
        deployment.egc.workqueue().done(&item.key);
        deployment.drain_class().await;
        let class = deployment.gc_raw.get("edgion").unwrap();
        let status = serde_json::to_string(&class.status).unwrap();
        assert_eq!(status.contains("ambiguous"), namespace.is_some());
    }
}
