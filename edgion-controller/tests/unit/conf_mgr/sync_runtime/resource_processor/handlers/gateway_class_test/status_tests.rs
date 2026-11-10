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
    runtime: Arc<ControllerRuntime>,
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
            runtime,
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

    /// `resolved` is false when this deployment cannot see the global plugin.
    fn assert_class(&self, accepted: bool, resolved: bool) {
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
        let resolved_refs = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
        if resolved {
            assert_eq!(resolved_refs.status, "True");
        } else {
            assert_eq!(
                (&*resolved_refs.status, &*resolved_refs.reason),
                ("False", "InvalidParameters")
            );
            assert!(resolved_refs.message.contains("globalPluginsRef team-a/ids not found"));
        }
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
        deployment.assert_class(pass % 2 != 0, pass % 2 == 0);
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
        deployment.assert_class(source != Some("ctx"), source.is_some());
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

#[tokio::test]
async fn parameters_recover_after_target_removal_and_recreation() {
    let deployment = Deployment::new("parameters-recovery", Some("traceId")).await;
    let (source, _) = deployment.process_egc(config()).await;
    deployment.drain_class().await;
    deployment.assert_class(true, true);
    deployment.egc_raw.remove("shared");
    deployment.egc.on_event_key("shared".into()).await;
    let item = tokio::time::timeout(Duration::from_secs(5), deployment.egc.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.key, "shared");
    deployment
        .egc
        .process_work_item(&item.key, None, None, item.trigger_chain)
        .await;
    deployment.egc.workqueue().done(&item.key);
    deployment.drain_class().await;
    let class = deployment.gc.get(&deployment.class).unwrap();
    for kind in ["Accepted", "ResolvedRefs"] {
        let condition = class
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|c| c.type_ == kind)
            .unwrap();
        assert_eq!((&*condition.status, &*condition.reason), ("False", "InvalidParameters"));
    }
    deployment.process_egc(source).await;
    deployment.drain_class().await;
    deployment.assert_class(true, true);
}

/// ctrl-proc2-02: the shared EGC keeps intrinsic-only status, while the
/// deployment-owned class reports every EGC reference this deployment cannot
/// resolve (and therefore cannot ship to its Gateways), then recovers.
#[tokio::test]
async fn class_reports_locally_unresolved_global_references_and_recovers() {
    use k8s_openapi::api::core::v1::{ConfigMap, Secret};
    use std::collections::{HashMap, HashSet};
    let deployment = Deployment::new("references", Some("traceId")).await;
    let shared: EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "shared", "generation": 1},
        "spec": {
            "globalPluginsRef": [{"name": "ids", "namespace": "team-a"}, {"name": "absent", "namespace": "team-a"}],
            "realIp": {"trustedIpRefs": [{"name": "proxies", "namespace": "ops"}]},
            "outboundTls": {
                "validation": {"caCertificateRefs": [
                    {"kind": "Secret", "name": "ca", "namespace": "certs"},
                    {"kind": "ConfigMap", "name": "bundle", "namespace": "certs"}
                ]},
                "clientCertificateRef": {"name": "client", "namespace": "certs"}
            }
        }
    }))
    .unwrap();
    let (shared, _) = deployment.process_egc(shared).await;
    assert!(shared
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .all(|condition| condition.type_ != "ResolvedRefs"));
    assert_eq!(shared.status.as_ref().unwrap().conditions[0].status, "True");
    deployment.drain_class().await;
    let class = deployment.gc.get(&deployment.class).unwrap();
    let conditions = &class.status.as_ref().unwrap().conditions;
    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(accepted.status, "True", "missing references do not reject the class");
    let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
    assert_eq!((&*resolved.status, &*resolved.reason), ("False", "InvalidParameters"));
    for expected in [
        "outboundTls.validation.caCertificateRefs[0]: Secret 'certs/ca' not found",
        "outboundTls.validation.caCertificateRefs[1]: ConfigMap 'certs/bundle' not found",
        "outboundTls.clientCertificateRef: Secret 'certs/client' not found",
        "globalPluginsRef team-a/absent not found",
        "realIp.trustedIpRefs[0]: EdgionConfigData 'ops/proxies' not found",
    ] {
        assert!(
            resolved.message.contains(expected),
            "{expected} in {}",
            resolved.message
        );
    }
    assert!(!resolved.message.contains("team-a/ids"));

    // A ConfigMap without CA data is reported distinctly from a missing one.
    let empty: ConfigMap = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "bundle", "namespace": "certs"}, "data": {}
    }))
    .unwrap();
    deployment
        .runtime
        .configmap_store
        .replace_all(HashMap::from([("certs/bundle".into(), empty)]));
    deployment.process_egc(shared.clone()).await;
    deployment.drain_class().await;
    let class = deployment.gc.get(&deployment.class).unwrap();
    let resolved = class
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .find(|c| c.type_ == "ResolvedRefs")
        .unwrap()
        .clone();
    assert!(resolved
        .message
        .contains("ConfigMap 'certs/bundle' has no CA certificate data"));

    let secret = |name: &str| Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(name.into()),
            namespace: Some("certs".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    deployment.runtime.secret_store.update(
        HashMap::from([
            ("certs/ca".into(), secret("ca")),
            ("certs/client".into(), secret("client")),
        ]),
        &HashSet::new(),
    );
    let bundle: ConfigMap = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "bundle", "namespace": "certs"}, "data": {"ca.crt": "pem"}
    }))
    .unwrap();
    deployment
        .runtime
        .configmap_store
        .replace_all(HashMap::from([("certs/bundle".into(), bundle)]));
    let proxies = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "proxies", "namespace": "ops"},
        "spec": {"visibility": "Cluster", "data": {"type": "IpList", "config": {"items": [{"name": "private", "cidrs": ["10.0.0.0/8"]}]}}}
    }))
    .unwrap();
    deployment
        .runtime
        .edgion_config_data_store
        .update(HashMap::from([("ops/proxies".into(), proxies)]), &HashSet::new());
    let mut absent = plugins("traceId");
    absent.metadata.name = Some("absent".into());
    deployment.plugins_raw.upsert("team-a/absent".into(), Arc::new(absent));
    let (_, changed) = deployment.process_egc(shared.clone()).await;
    assert!(!changed, "local availability never changes shared EGC status");
    deployment.drain_class().await;
    deployment.assert_class(true, true);

    // Regression direction: material that disappears flips the class back, and a
    // present list of the wrong type is named as such rather than as missing.
    deployment
        .runtime
        .secret_store
        .update(HashMap::new(), &HashSet::from(["certs/client".into()]));
    let wrong_type = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "proxies", "namespace": "ops"},
        "spec": {"visibility": "Cluster", "data": {"type": "Misc", "config": {}}}
    }))
    .unwrap();
    deployment
        .runtime
        .edgion_config_data_store
        .update(HashMap::from([("ops/proxies".into(), wrong_type)]), &HashSet::new());
    deployment.process_egc(shared).await;
    deployment.drain_class().await;
    let class = deployment.gc.get(&deployment.class).unwrap();
    let resolved = class
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .find(|c| c.type_ == "ResolvedRefs")
        .unwrap()
        .clone();
    assert_eq!(resolved.status, "False");
    assert!(resolved
        .message
        .contains("outboundTls.clientCertificateRef: Secret 'certs/client' not found"));
    assert!(resolved
        .message
        .contains("realIp.trustedIpRefs[0]: EdgionConfigData 'ops/proxies' is not an enabled IpList"));
    assert!(!resolved.message.contains("certs/ca'"));
}

#[tokio::test]
async fn global_real_ip_shared_acceptance_is_independent_of_local_data_and_validation_policy() {
    const CHILD: &str = "EDGION_B3_REAL_IP_STATUS_CHILD";
    const TEST: &str = "conf_mgr::sync_runtime::resource_processor::handlers::gateway_class::status_tests::global_real_ip_shared_acceptance_is_independent_of_local_data_and_validation_policy";
    if std::env::var_os(CHILD).is_none() {
        let mut reports = Vec::new();
        for enabled in ["true", "false"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env(CHILD, enabled)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            let report = stdout
                .split("B3_STATUS_REPORT:")
                .nth(1)
                .expect("child executed exact status test")
                .lines()
                .next()
                .unwrap();
            reports.push(serde_json::from_str::<serde_json::Value>(report).unwrap());
        }
        assert_eq!(reports[0]["accepted"], reports[1]["accepted"]);
        assert_ne!(reports[0]["scopes"], reports[1]["scopes"]);
        return;
    }
    use edgion_resources::resources::edgion_plugins::ip_reference_set::IpReadScope;
    use std::collections::{HashMap, HashSet};
    let enabled = std::env::var(CHILD).unwrap() == "true";
    let mut process_config = crate::config::EdgionControllerConfig::default();
    process_config.validation.enable_reference_grant_validation = enabled;
    crate::config::init_controller_config(process_config);
    assert_eq!(crate::config::is_reference_grant_validation_enabled(), enabled);
    let first = Arc::new(ControllerRuntime::default());
    let second = Arc::new(ControllerRuntime::default());
    let mut accepted = None;
    let mut scopes = Vec::new();
    for (runtime, visibility) in [(&first, Some("Cluster")), (&second, Some("Namespace")), (&second, None)] {
        let incoming = visibility.map(|visibility| {
            let data = serde_json::from_value(serde_json::json!({
                "metadata": {"name": "proxies", "namespace": "ops"},
                "spec": {"visibility": visibility, "data": {"type": "IpList", "config": {"items": [{"name": "private", "cidrs": ["10.0.0.0/8"]}]}}}
            })).unwrap();
            HashMap::from([("ops/proxies".into(), data)])
        }).unwrap_or_default();
        runtime
            .edgion_config_data_store
            .update(HashMap::new(), &HashSet::from(["ops/proxies".into()]));
        runtime.edgion_config_data_store.update(incoming, &HashSet::new());
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        let mut owner: EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "shared-trust", "generation": 1}, "spec": {"realIp": {"trustedIpRefs": [{"name": "proxies", "namespace": "ops"}]}}
        })).unwrap();
        let handler = EdgionGatewayConfigHandler::new();
        let errors = handler.preparse(&mut owner, &ctx);
        assert!(errors.is_empty());
        let crate::conf_mgr::sync_runtime::resource_processor::ProcessResult::Continue(mut owner) =
            handler.parse(owner, &ctx).await
        else {
            panic!("intrinsically valid EGC must publish");
        };
        handler.update_status(&mut owner, &ctx, &errors);
        let conditions = &owner.status.as_ref().unwrap().conditions;
        assert!(conditions.iter().all(|condition| condition.type_ != "ResolvedRefs"));
        let condition = conditions
            .iter()
            .find(|condition| condition.type_ == "Accepted")
            .unwrap();
        let current = serde_json::json!([condition.status, condition.reason, condition.message]);
        assert_eq!(condition.status, "True");
        if let Some(previous) = &accepted {
            assert_eq!(previous, &current);
        } else {
            accepted = Some(current);
        }
        let scope = owner
            .spec
            .real_ip
            .as_ref()
            .unwrap()
            .trusted
            .resolved_refs
            .as_ref()
            .unwrap()
            .slots[0]
            .scope;
        assert_eq!(
            scope,
            if !enabled {
                IpReadScope::ValidationDisabled
            } else if visibility == Some("Namespace") {
                IpReadScope::Denied
            } else {
                IpReadScope::ClusterOnly
            }
        );
        scopes.push(scope);
    }
    println!(
        "B3_STATUS_REPORT:{}",
        serde_json::json!({"accepted": accepted.unwrap(), "scopes": scopes})
    );
}
