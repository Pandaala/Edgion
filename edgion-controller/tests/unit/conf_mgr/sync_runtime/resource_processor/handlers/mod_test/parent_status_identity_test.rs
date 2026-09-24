//! Cross-handler ownership and processed transport regression tests.

use super::{EdgionTlsHandler, GrpcRouteHandler, HttpRouteHandler, TcpRouteHandler, TlsRouteHandler, UdpRouteHandler};
use crate::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, LocalRawStore, ProcessorHandler, RawStore, ResourceProcessor, WorkItemResult,
};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::{EdgionTls, GRPCRoute, HTTPRoute, TCPRoute, TLSRoute, UDPRoute};
use serde_json::{json, Value};
use std::sync::Arc;

const OWNER: &str = "example.com/custom-controller";
const FOREIGN: &str = "example.com/foreign-controller";
const KINDS: [&str; 6] = [
    "HTTPRoute",
    "GRPCRoute",
    "TCPRoute",
    "TLSRoute",
    "UDPRoute",
    "EdgionTls",
];

fn context() -> HandlerContext {
    HandlerContext::new(
        None,
        None,
        Default::default(),
        5,
        20,
        Arc::new(ControllerRuntime::default()),
    )
}

fn resource(kind: &str, parents: Value) -> Value {
    let mut value = json!({
        "metadata": {"namespace": "apps", "name": "route", "generation": 1},
        "spec": {"parentRefs": parents}
    });
    if kind == "EdgionTls" {
        value["spec"]["hosts"] = json!(["example.com"]);
        value["spec"]["secretRef"] = json!({"name": "certificate"});
    } else if matches!(kind, "HTTPRoute" | "GRPCRoute" | "TLSRoute") {
        value["spec"]["hostnames"] = json!(["example.com"]);
    }
    value
}

fn parent(port: i32, explicit: bool) -> Value {
    let mut value = json!({"name": "gateway", "sectionName": "listener", "port": port});
    if explicit {
        value["group"] = json!("gateway.networking.k8s.io");
        value["kind"] = json!("Gateway");
        value["namespace"] = json!("apps");
    }
    value
}

fn status(parent: Value, owner: &str) -> Value {
    json!({
        "parentRef": parent, "controllerName": owner,
        "conditions": [{"type": "Accepted", "status": "True", "reason": "Accepted",
            "message": "Foreign snapshot", "observedGeneration": 1,
            "lastTransitionTime": "2000-01-01T00:00:00Z"}]
    })
}

fn update(kind: &str, value: Value, ctx: &HandlerContext) -> Value {
    macro_rules! run {
        ($ty:ty, $handler:ty) => {{
            let mut route: $ty = serde_json::from_value(value).unwrap();
            <$handler>::new(OWNER.to_owned()).update_status(&mut route, ctx, &[]);
            serde_json::to_value(route).unwrap()
        }};
    }
    match kind {
        "HTTPRoute" => run!(HTTPRoute, HttpRouteHandler),
        "GRPCRoute" => run!(GRPCRoute, GrpcRouteHandler),
        "TCPRoute" => run!(TCPRoute, TcpRouteHandler),
        "TLSRoute" => run!(TLSRoute, TlsRouteHandler),
        "UDPRoute" => run!(UDPRoute, UdpRouteHandler),
        "EdgionTls" => run!(EdgionTls, EdgionTlsHandler),
        _ => panic!("unexpected fixture kind: {kind}"),
    }
}

fn owned(value: &Value) -> Vec<&Value> {
    value["status"]["parents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["controllerName"] == OWNER)
        .collect()
}

#[test]
fn all_writers_isolate_owners_defaults_ports_and_duplicates() {
    for kind in KINDS {
        for foreign_first in [false, true] {
            let ctx = context();
            let foreign = status(parent(80, false), FOREIGN);
            let mut old = vec![status(parent(80, true), OWNER), status(parent(80, false), OWNER)];
            if foreign_first {
                old.insert(0, foreign.clone());
            } else {
                old.push(foreign.clone());
            }
            let mut input = resource(kind, json!([parent(80, false), parent(443, false)]));
            input["status"] = json!({"parents": old});
            let output = update(kind, input, &ctx);
            let entries = owned(&output);
            assert_eq!(
                entries.len(),
                2,
                "{kind}: owned defaults must deduplicate but ports remain distinct"
            );
            for port in [80, 443] {
                let entry = entries.iter().find(|entry| entry["parentRef"]["port"] == port).unwrap();
                assert!(
                    entry["conditions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|condition| condition["type"] == "Accepted" && condition["status"] == "False"),
                    "{kind}: missing Gateway must reject our attachment"
                );
            }
            let foreign_after = output["status"]["parents"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["controllerName"] == FOREIGN)
                .unwrap();
            assert_eq!(
                foreign_after, &foreign,
                "{kind}: foreign snapshot must survive unchanged"
            );

            // Seed an old timestamp after the first calculation so unchanged conditions
            // cannot pass merely because two reconciles happened in the same second.
            let mut seeded = output;
            for entry in seeded["status"]["parents"].as_array_mut().unwrap() {
                if entry["controllerName"] == OWNER {
                    for condition in entry["conditions"].as_array_mut().unwrap() {
                        condition["lastTransitionTime"] = json!("2000-01-01T00:00:00Z");
                    }
                }
            }
            let repeated = update(kind, seeded.clone(), &ctx);
            assert_eq!(
                repeated["status"], seeded["status"],
                "{kind}: unchanged reconcile must preserve conditions"
            );
        }
    }
}

#[test]
fn all_writers_remove_only_stale_owned_statuses() {
    for kind in KINDS {
        for parents in [Value::Null, json!([]), json!([parent(443, false)])] {
            let foreign = status(parent(80, false), FOREIGN);
            let mut input = resource(kind, parents.clone());
            input["status"] = json!({"parents": [foreign.clone(), status(parent(80, false), OWNER)]});
            let output = update(kind, input, &context());
            assert!(
                output["status"]["parents"].as_array().unwrap().contains(&foreign),
                "{kind}"
            );
            assert!(
                owned(&output).iter().all(|entry| entry["parentRef"]["port"] == 443),
                "{kind}"
            );
            assert_eq!(owned(&output).len(), parents.as_array().map_or(0, Vec::len), "{kind}");
        }
    }
}

#[test]
fn delegation_and_gateway_statuses_never_share_same_coordinate_entry() {
    for kind in ["HTTPRoute", "GRPCRoute"] {
        let gateway = json!({"name": "parent", "namespace": "apps"});
        let delegation = json!({"name": "parent", "namespace": "apps",
            "group": "gateway.networking.k8s.io", "kind": kind});
        let mut input = resource(kind, json!([gateway.clone(), delegation.clone()]));
        let foreign = status(delegation.clone(), FOREIGN);
        input["status"] = json!({"parents": [foreign.clone(), status(delegation, OWNER), status(gateway, OWNER)]});
        let output = update(kind, input, &context());
        let entries = owned(&output);
        assert_eq!(entries.len(), 2, "{kind}");
        let gateway = entries
            .iter()
            .find(|entry| entry["parentRef"]["kind"].is_null())
            .unwrap();
        let delegation = entries.iter().find(|entry| entry["parentRef"]["kind"] == kind).unwrap();
        let message = |entry: &Value| {
            entry["conditions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|condition| condition["type"] == "Accepted")
                .unwrap()["message"]
                .clone()
        };
        assert_ne!(
            message(gateway),
            message(delegation),
            "{kind}: independent validation must not overwrite either status"
        );
        assert!(output["status"]["parents"].as_array().unwrap().contains(&foreign));
    }
}

macro_rules! processor_owner_test {
    ($name:ident, $ty:ty, $handler:ty, $kind:literal) => {
        #[tokio::test]
        async fn $name() {
            let runtime = Arc::new(ControllerRuntime::default());
            let processor =
                ResourceProcessor::<$ty>::new($kind, 16, Arc::new(<$handler>::new(OWNER.to_owned())), runtime);
            let raw = Arc::new(LocalRawStore::new());
            processor.set_raw_store(raw.clone());
            let mut input = resource($kind, json!([parent(443, false)]));
            input["spec"]["resolvedStatusController"] = json!(FOREIGN);
            let route: $ty = serde_json::from_value(input).unwrap();
            raw.upsert("apps/route".into(), Arc::new(route.clone()));
            processor.on_init();
            assert!(matches!(
                processor.on_init_apply(route, None).await,
                WorkItemResult::Processed { .. }
            ));
            processor.on_init_done();
            let initial = serde_json::to_value(processor.get("apps/route").unwrap()).unwrap();
            assert_eq!(initial["spec"]["resolvedStatusController"], OWNER);
            assert_eq!(owned(&initial).len(), 1);

            // An owner-only forged update, followed by a status-only update, must
            // run the real queue/processor entry and replace the cached payload.
            for status_only in [false, true] {
                let mut update = initial.clone();
                if status_only {
                    update["status"]["parents"][0]["conditions"][0]["message"] = json!("Stale input condition");
                } else {
                    update["spec"]["resolvedStatusController"] = json!("example.com/forged");
                }
                let route: $ty = serde_json::from_value(update).unwrap();
                raw.upsert("apps/route".into(), Arc::new(route));
                processor.on_event_key("apps/route".into()).await;
                let queue = processor.workqueue();
                let item = queue.dequeue().await.unwrap();
                let result = processor
                    .process_work_item(
                        &item.key,
                        raw.get(&item.key).map(|route| (*route).clone()),
                        None,
                        item.trigger_chain,
                    )
                    .await;
                queue.done(&item.key);
                let WorkItemResult::Processed { status_changed, .. } = result else {
                    panic!("queued route must be processed");
                };
                assert_eq!(status_changed, status_only);
                let published = serde_json::to_value(processor.get("apps/route").unwrap()).unwrap();
                assert_eq!(published["spec"]["resolvedStatusController"], OWNER);
                assert_eq!(published["status"], initial["status"]);
            }
        }
    };
}

processor_owner_test!(
    http_processor_overwrites_inbound_owner_on_every_publication,
    HTTPRoute,
    HttpRouteHandler,
    "HTTPRoute"
);
processor_owner_test!(
    grpc_processor_overwrites_inbound_owner_on_every_publication,
    GRPCRoute,
    GrpcRouteHandler,
    "GRPCRoute"
);
processor_owner_test!(
    tcp_processor_overwrites_inbound_owner_on_every_publication,
    TCPRoute,
    TcpRouteHandler,
    "TCPRoute"
);
processor_owner_test!(
    tls_processor_overwrites_inbound_owner_on_every_publication,
    TLSRoute,
    TlsRouteHandler,
    "TLSRoute"
);
processor_owner_test!(
    udp_processor_overwrites_inbound_owner_on_every_publication,
    UDPRoute,
    UdpRouteHandler,
    "UDPRoute"
);
