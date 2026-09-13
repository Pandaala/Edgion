//! Drive one serial worker per kind through the real Gateway publication and
//! TLSRoute processing hooks. No network, status writer, or extra worker runs.

use super::GatewayHandler;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::tls_route::TlsRouteHandler;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessorHandler, ResourceProcessor};
use crate::conf_mgr::sync_runtime::workqueue::{WorkItem, Workqueue};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::{Gateway, TLSRoute};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const GATEWAY: &str = "apps/gateway";
const ROUTE: &str = "apps/route";

async fn dequeue(queue: &Workqueue) -> WorkItem {
    tokio::time::timeout(Duration::from_secs(1), queue.dequeue())
        .await
        .expect("dependent resource was not queued")
        .expect("queue closed")
}

struct Fixture {
    runtime: Arc<ControllerRuntime>,
    handler: Arc<GatewayHandler>,
    gateways: Arc<ResourceProcessor<Gateway>>,
    routes: Arc<ResourceProcessor<TLSRoute>>,
    route: TLSRoute,
}

impl Fixture {
    async fn new(gateway: &Gateway, hostnames: &[&str]) -> Self {
        let runtime = Arc::new(ControllerRuntime::default());
        let handler = Arc::new(GatewayHandler::new(None, None));
        let gateways = Arc::new(ResourceProcessor::new("Gateway", 16, handler.clone(), runtime.clone()));
        let routes = Arc::new(ResourceProcessor::new(
            "TLSRoute",
            16,
            Arc::new(TlsRouteHandler::default()),
            runtime.clone(),
        ));
        runtime.processor_registry.register(gateways.clone());
        runtime.processor_registry.register(routes.clone());
        let route: TLSRoute = serde_json::from_value(serde_json::json!({
            "metadata": {"namespace": "apps", "name": "route"},
            "spec": {"parentRefs": [{"name": "gateway"}], "hostnames": hostnames}
        }))
        .unwrap();
        gateways.on_init_apply(gateway.clone(), None).await;
        routes.on_init_apply(route.clone(), None).await;
        gateways.on_init_done();
        routes.on_init_done();
        let fixture = Self {
            runtime,
            handler,
            gateways,
            routes,
            route,
        };
        fixture.process_gateway(Some(gateway.clone()), false).await;
        assert_eq!(
            fixture.route_requeues(),
            0,
            "status feedback must not requeue the route"
        );
        fixture
    }

    fn route_requeues(&self) -> u64 {
        self.routes.workqueue().metrics().delayed_total.load(Ordering::Relaxed)
    }

    async fn process_gateway(&self, source: Option<Gateway>, event: bool) {
        if event {
            self.gateways.on_event_key(GATEWAY.into()).await;
        }
        let queue = self.gateways.workqueue();
        let item = dequeue(&queue).await;
        assert_eq!(item.key, GATEWAY);
        self.gateways
            .process_work_item(&item.key, source, None, item.trigger_chain)
            .await;
        queue.done(&item.key);
    }

    async fn update(&self, gateway: Option<Gateway>) {
        let before = self.route_requeues();
        if let Some(gateway) = &gateway {
            // The pre-publication hook must not expose old cached attachments
            // to an independently scheduled Route worker.
            let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, self.runtime.clone());
            self.handler.on_change(gateway, &ctx).await;
            assert_eq!(self.route_requeues(), before);
        }
        self.process_gateway(gateway.clone(), true).await;
        assert_eq!(self.route_requeues(), before + 1);
        assert_eq!(self.gateways.get(GATEWAY).is_some(), gateway.is_some());
        let queue = self.routes.workqueue();
        let item = dequeue(&queue).await;
        assert_eq!(item.key, ROUTE);
        self.routes
            .process_work_item(&item.key, Some(self.route.clone()), None, item.trigger_chain)
            .await;
        queue.done(&item.key);
        // TLSRoute feeds attachedRoutes back to Gateway; this publication gets
        // a new proof epoch but must not start another TLSRoute processing loop.
        self.process_gateway(gateway, false).await;
        assert_eq!(self.route_requeues(), before + 1);
    }

    fn attachments(&self) -> Vec<(String, String, Vec<String>)> {
        self.routes
            .get(ROUTE)
            .unwrap()
            .spec
            .resolved_listeners
            .unwrap_or_default()
            .into_iter()
            .map(|a| (a.listener_name, a.listener_key.0, a.hostnames))
            .collect()
    }
}

fn gateway(listeners: serde_json::Value) -> Gateway {
    serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "apps", "name": "gateway", "generation": 1},
        "spec": {"gatewayClassName": "test", "listeners": listeners}
    }))
    .unwrap()
}

fn listener(name: &str, hostname: Option<&str>, port: i32) -> serde_json::Value {
    serde_json::json!({"name": name, "hostname": hostname, "port": port, "protocol": "TLS", "tls": {"mode": "Passthrough"}})
}

#[tokio::test]
async fn listener_identity_refreshes_after_publish_and_unpublish() {
    let mut gateway = gateway(serde_json::json!([
        listener("named", Some("a.example.com"), 8443),
        listener("open", None, 8443)
    ]));
    let fixture = Fixture::new(&gateway, &["b.example.com"]).await;
    assert_eq!(
        fixture.attachments(),
        [("open".into(), "0.0.0.0:8443/TLS".into(), vec!["b.example.com".into()])]
    );

    gateway.spec.listeners.as_mut().unwrap()[1].name = "renamed".into();
    fixture.update(Some(gateway.clone())).await;
    assert_eq!(fixture.attachments()[0].0, "renamed");

    let listeners = gateway.spec.listeners.as_mut().unwrap();
    listeners[1].hostname = listeners[0].hostname.take();
    fixture.update(Some(gateway.clone())).await;
    assert_eq!(
        fixture.attachments(),
        [("named".into(), "0.0.0.0:8443/TLS".into(), vec!["b.example.com".into()])]
    );

    let epoch = fixture
        .gateways
        .get(GATEWAY)
        .unwrap()
        .spec
        .resolved_attachment_proof
        .unwrap()
        .epoch
        .clone();
    let count = fixture.route_requeues();
    gateway.metadata.generation = Some(2);
    gateway.metadata.resource_version = Some("99".into());
    fixture.process_gateway(Some(gateway.clone()), true).await;
    assert_ne!(
        fixture
            .gateways
            .get(GATEWAY)
            .unwrap()
            .spec
            .resolved_attachment_proof
            .unwrap()
            .epoch,
        epoch
    );
    assert_eq!(
        fixture.route_requeues(),
        count,
        "metadata/status/proof must not schedule TLSRoute"
    );

    fixture.update(None).await;
    assert!(
        fixture.attachments().is_empty(),
        "deleted Gateway must lose attachments before recreation"
    );
    fixture.update(Some(gateway)).await;
    assert_eq!(fixture.attachments()[0].0, "named");
}

#[tokio::test]
async fn hostname_exchange_across_existing_ports_refreshes_each_attachment() {
    let mut gateway = gateway(serde_json::json!([
        listener("a", Some("a.example.com"), 8443),
        listener("b", Some("b.example.com"), 9443)
    ]));
    let fixture = Fixture::new(&gateway, &["a.example.com", "b.example.com"]).await;
    let listeners = gateway.spec.listeners.as_mut().unwrap();
    listeners[0].hostname = Some("b.example.com".into());
    listeners[1].hostname = Some("a.example.com".into());
    fixture.update(Some(gateway)).await;
    assert_eq!(
        fixture.attachments(),
        [
            ("a".into(), "0.0.0.0:8443/TLS".into(), vec!["b.example.com".into()]),
            ("b".into(), "0.0.0.0:9443/TLS".into(), vec!["a.example.com".into()]),
        ]
    );
}

#[tokio::test]
async fn parent_edges_precede_gateway_reads_and_old_counts_clear_before_requeue() {
    use crate::conf_mgr::sync_runtime::resource_processor::RawStore;
    use edgion_resources::ResourceKind;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;

    struct ReadBarrierStore {
        runtime: std::sync::Weak<ControllerRuntime>,
        values: HashMap<String, Arc<Gateway>>,
        queue: Arc<Workqueue>,
        checked_new_parent: AtomicBool,
    }
    impl RawStore<Gateway> for ReadBarrierStore {
        fn get(&self, key: &str) -> Option<Arc<Gateway>> {
            let gateway = self.values.get(key)?;
            let runtime = self.runtime.upgrade().unwrap();
            assert!(
                runtime
                    .gateway_route_index
                    .get_routes_for_gateway(key)
                    .contains(&(ResourceKind::TLSRoute, ROUTE.into())),
                "dependency must precede Gateway lookup"
            );
            if key == "apps/new" && !self.checked_new_parent.swap(true, Ordering::Relaxed) {
                assert_eq!(runtime.attached_route_tracker.count_for_listener("apps/old", "tls"), 1);
                assert_eq!(runtime.gateway_route_index.get_routes_for_gateway("apps/old").len(), 1);
                assert_eq!(
                    self.queue.metrics().delayed_total.load(Ordering::Relaxed),
                    1,
                    "dropped parent must not be scheduled before status replaces its count"
                );
            }
            Some(gateway.clone())
        }
        fn snapshot(&self) -> Vec<Arc<Gateway>> {
            self.values.values().cloned().collect()
        }
    }

    let runtime = Arc::new(ControllerRuntime::default());
    let gateways = Arc::new(ResourceProcessor::new(
        "Gateway",
        16,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    let routes = Arc::new(ResourceProcessor::new(
        "TLSRoute",
        16,
        Arc::new(TlsRouteHandler::default()),
        runtime.clone(),
    ));
    runtime.processor_registry.register(gateways.clone());
    runtime.processor_registry.register(routes.clone());
    let values = ["old", "new"]
        .into_iter()
        .map(|name| {
            let mut gateway = gateway(serde_json::json!([listener("tls", None, 8443)]));
            gateway.metadata.name = Some(name.into());
            (format!("apps/{name}"), Arc::new(gateway))
        })
        .collect();
    let raw = Arc::new(ReadBarrierStore {
        runtime: Arc::downgrade(&runtime),
        values,
        queue: gateways.workqueue(),
        checked_new_parent: AtomicBool::new(false),
    });
    gateways.set_raw_store(raw.clone());
    let mut route: TLSRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "apps", "name": "route", "uid": "first"},
        "spec": {"parentRefs": [{"name": "old"}], "hostnames": ["a.example.com"]}
    }))
    .unwrap();
    routes.on_init_apply(route.clone(), None).await;
    routes.on_init_done();
    let queued = dequeue(&gateways.workqueue()).await;
    assert_eq!(queued.key, "apps/old");
    gateways.workqueue().done(&queued.key);

    // Model delete/create coalescing: the apply path sees a new UID and parent,
    // without an intervening on_delete to clean the prior generation's edges.
    route.metadata.uid = Some("second".into());
    route.spec.parent_refs.as_mut().unwrap()[0].name = "new".into();
    routes.on_event_key(ROUTE.into()).await;
    let queued = dequeue(&routes.workqueue()).await;
    routes
        .process_work_item(&queued.key, Some(route), None, queued.trigger_chain)
        .await;
    routes.workqueue().done(&queued.key);
    assert!(raw.checked_new_parent.load(Ordering::Relaxed));
    assert!(runtime
        .gateway_route_index
        .get_routes_for_gateway("apps/old")
        .is_empty());
    assert_eq!(runtime.gateway_route_index.get_routes_for_gateway("apps/new").len(), 1);
    assert_eq!(runtime.attached_route_tracker.count_for_listener("apps/old", "tls"), 0);
    assert_eq!(runtime.attached_route_tracker.count_for_listener("apps/new", "tls"), 1);
    let mut requeued = Vec::new();
    for _ in 0..2 {
        let queued = dequeue(&gateways.workqueue()).await;
        requeued.push(queued.key.clone());
        gateways.workqueue().done(&queued.key);
    }
    requeued.sort();
    assert_eq!(requeued, ["apps/new", "apps/old"]);
}
