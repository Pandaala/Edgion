use super::*;
use edgion_resources::resources::refs::route_fields::*;
use edgion_resources::{GRPCRoute, HTTPRoute, TCPRoute, TLSRoute, UDPRoute};
use serde_json::json;
use std::cell::RefCell;

#[test]
fn all_five_terminal_sources_register_before_flag_and_exact_grant_reads() {
    macro_rules! check {
        ($route:ty,$facade:ident,$kind:expr) => {{
            let route:$route=serde_json::from_value(json!({"metadata":{"name":"owner"},"spec":{"rules":[{"backendRefs":[{"name":"service","namespace":"target","port":80}]}]}})).unwrap();
            let refs=$facade::from_route(&route).unwrap();
            let AuthoredRouteTarget::Terminal(terminal)=refs.authored_at(0,0).unwrap().validated().unwrap() else { panic!("terminal") };
            let owner=ResourceRef::new($kind,None,"owner".into());
            for enabled in [false,true] {
                let runtime=ControllerRuntime::default(); let order=RefCell::new(Vec::new());
                let denial=authorize_terminal_with(terminal,&owner,&runtime,|| {
                    assert_eq!(runtime.cross_ns_ref_mgr.get_refs("target"),vec![owner.clone()]); order.borrow_mut().push("flag"); enabled
                },|query| {
                    assert_eq!(runtime.cross_ns_ref_mgr.get_refs("target"),vec![owner.clone()]); order.borrow_mut().push("grant");
                    assert_eq!(query.source().group_kind().kind(),$kind.as_str());
                    assert_eq!(query.source().scope().namespace(),Some("default"));
                    assert_eq!(query.target().group_kind().group(),""); false
                });
                assert_eq!(denial.is_some(),enabled);
                assert_eq!(*order.borrow(),if enabled {vec!["flag","grant"]}else{vec!["flag"]});
                assert_eq!(owner.key(),format!("{}//owner",$kind.as_str()));
            }
        }};
    }
    check!(HTTPRoute, HttpRouteBackendRefs, ResourceKind::HTTPRoute);
    check!(GRPCRoute, GrpcRouteBackendRefs, ResourceKind::GRPCRoute);
    check!(TCPRoute, TcpRouteBackendRefs, ResourceKind::TCPRoute);
    check!(TLSRoute, TlsRouteBackendRefs, ResourceKind::TLSRoute);
    check!(UDPRoute, UdpRouteBackendRefs, ResourceKind::UDPRoute);
}

#[test]
fn same_namespace_skips_authority_reads_and_receipts_preserve_duplicate_ports() {
    let route:HTTPRoute=serde_json::from_value(json!({"metadata":{"name":"owner"},"spec":{"rules":[{"backendRefs":[{"name":"service","port":80},{"name":"service","port":443}]}]}})).unwrap();
    let refs = HttpRouteBackendRefs::from_route(&route).unwrap();
    let runtime = ControllerRuntime::default();
    let owner = ResourceRef::new(ResourceKind::HTTPRoute, None, "owner".into());
    for reference in refs.authored() {
        let AuthoredRouteTarget::Terminal(terminal) = reference.validated().unwrap() else {
            panic!("terminal")
        };
        assert!(authorize_terminal_with(
            terminal,
            &owner,
            &runtime,
            || panic!("same namespace flag read"),
            |_| panic!("same namespace grant read")
        )
        .is_none());
    }
    let receipts = collect_dependencies(refs.authored());
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0].port, Some(80));
    assert_eq!(receipts[1].port, Some(443));
    assert_eq!(receipts[0].slot.backend(), 0);
    assert_eq!(receipts[1].slot.backend(), 1);
    assert_eq!(receipts[0].source_kind, ResourceKind::HTTPRoute);
    assert_eq!(receipts[0].source_name, "owner");
    let dropped = super::super::route_utils::register_service_backend_refs(&owner, &receipts, &runtime);
    assert!(dropped.is_empty());
    assert_eq!(runtime.service_ref_mgr.get_refs("default/service"), vec![owner.clone()]);
    assert!(runtime.cross_ns_ref_mgr.get_refs("default").is_empty());
}

#[test]
fn invalid_and_denied_targets_never_reach_existence_reads() {
    macro_rules! check {
        ($route:ty,$facade:ident) => {{
            for backend in [json!({"name":"service","namespace":"target","port":0}),json!({"name":"service","namespace":"target","group":"core","port":80}),json!({"name":"bad/name","namespace":"target","port":80}),json!({"name":"service","namespace":"target","port":80,"refDenied":{"targetNamespace":"target","targetName":"service"}})] {
                let route:$route=serde_json::from_value(json!({"metadata":{"name":"owner","namespace":"source"},"spec":{"rules":[{"backendRefs":[backend]}]}})).unwrap();
                let refs=$facade::from_route(&route).unwrap();
                let denied=refs.effective().next().unwrap().ref_denied().is_some();
                let errors=validate_targets_with(Ok(refs.effective()),|_| panic!("invalid/denied target read"));
                assert_eq!(errors.is_empty(),denied);
                let receipts=collect_dependencies(refs.authored()); assert_eq!(receipts.len(),usize::from(denied));
            }
            let route:$route=serde_json::from_value(json!({"metadata":{"name":"bad/name","namespace":"source"},"spec":{"rules":[{"backendRefs":[{"name":"service","namespace":"target","port":80}]}]}})).unwrap();
            let errors=validate_targets_with($facade::from_route(&route).map(|refs|refs.effective()),|_| panic!("invalid source read"));
            assert_eq!(errors.len(),1); assert_eq!(errors[0].reason(),"InvalidKind");
        }};
    }
    check!(HTTPRoute, HttpRouteBackendRefs);
    check!(GRPCRoute, GrpcRouteBackendRefs);
    check!(TCPRoute, TcpRouteBackendRefs);
    check!(TLSRoute, TlsRouteBackendRefs);
    check!(UDPRoute, UdpRouteBackendRefs);
}

#[tokio::test]
async fn all_five_invalid_owners_fail_closed_in_direct_status_and_recover_after_parse() {
    use crate::conf_mgr::sync_runtime::resource_processor::{
        handlers::{
            GatewayHandler, GrpcRouteHandler, HttpRouteHandler, TcpRouteHandler, TlsRouteHandler, UdpRouteHandler,
        },
        HandlerContext, LocalRawStore, ProcessResult, ProcessorHandler, ResourceProcessor,
    };
    use edgion_resources::Gateway;
    use std::sync::Arc;
    let runtime = Arc::new(ControllerRuntime::default());
    let gateways = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    raw.upsert(
        "default/gateway".into(),
        Arc::new(
            serde_json::from_value(json!({
                "metadata":{"namespace":"default","name":"gateway"},
                "spec":{"gatewayClassName":"test","listeners":[
                    {"name":"http","port":80,"protocol":"HTTP"},
                    {"name":"tcp","port":9000,"protocol":"TCP"},
                    {"name":"tls","port":443,"protocol":"TLS","tls":{"mode":"Passthrough"}},
                    {"name":"udp","port":9001,"protocol":"UDP"}
                ]}
            }))
            .unwrap(),
        ),
    );
    gateways.set_raw_store(raw);
    runtime.processor_registry.register(gateways);
    let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime.clone());
    macro_rules! check {
        ($route:ty,$handler:expr,$facade:ident,$kind:expr,$listener:expr) => {{
            let handler=$handler;
            for (name, namespace) in [(json!("bad/name"),json!("default")),(json!("owner"),json!("")),(json!(null),json!(null))] {
                let mut route:$route=serde_json::from_value(json!({
                    "metadata":{"name":name,"namespace":namespace,"generation":3},
                    "spec":{"hostnames":["api.example.com"],"parentRefs":[{"name":"gateway","namespace":"default","sectionName":$listener}],
                        "rules":[{"backendRefs":[{"name":"service","namespace":"target","port":80}]}]}
                })).unwrap();
                // Seed genuine prior derived attachments and Accepted=True, then replace
                // only the source metadata and target descriptor under test.
                let mut prior=route.clone(); prior.metadata.name=Some("owner".into()); prior.metadata.namespace=None;
                prior.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].namespace=None;
                let ProcessResult::Continue(mut prior)=handler.parse(prior,&ctx).await else {panic!("prior valid source")};
                handler.update_status(&mut prior,&ctx,&[]);
                assert!(prior.spec.resolved_listeners.is_some());
                route.spec.resolved_listeners=prior.spec.resolved_listeners;
                route.status=prior.status;
                assert!(route.status.as_ref().unwrap().parents.iter().any(|parent|parent.conditions.iter().any(|condition|condition.type_=="Accepted" && condition.status=="True")));
                assert!(!handler.validate(&route,&ctx).is_empty());
                // Direct status callers must not depend on validate having run.
                handler.update_status(&mut route,&ctx,&[]);
                assert!(route.spec.resolved_listeners.is_none());
                assert!(route.status.as_ref().unwrap().parents.iter().all(|parent|parent.conditions.iter().any(|condition|condition.type_=="Accepted" && condition.status=="False" && condition.observed_generation==Some(3))));
                let ProcessResult::Continue(mut parsed)=handler.parse(route,&ctx).await else {panic!("lenient invalid source status")};
                let errors=handler.validate(&parsed,&ctx); handler.update_status(&mut parsed,&ctx,&errors);
                assert!(parsed.spec.resolved_listeners.is_none());
                if matches!($kind, ResourceKind::GRPCRoute | ResourceKind::TCPRoute | ResourceKind::UDPRoute) {
                    let mut invalid = parsed.clone();
                    let expected = source_error_message($facade::from_route(&invalid).err().unwrap());
                    for parent in ["missing", "gateway"] {
                        invalid.spec.parent_refs.as_mut().unwrap()[0].name = parent.into();
                        handler.update_status(&mut invalid, &ctx, &errors);
                        let parents = &invalid.status.as_ref().unwrap().parents;
                        assert_eq!(parents.len(), 1);
                        let accepted = parents[0].conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
                        assert_eq!((&*accepted.status, &*accepted.reason), ("False", "Invalid"));
                        assert_eq!(accepted.message, expected);
                    }
                }
                assert!(runtime.cross_ns_ref_mgr.get_refs("target").is_empty());
                assert!(runtime.service_ref_mgr.get_refs("target/service").is_empty());
                // Repair keeps the original backend slot and restarts ordinary publication.
                parsed.metadata.name=Some("owner".into()); parsed.metadata.namespace=None;
                parsed.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].namespace=None;
                let ProcessResult::Continue(mut repaired)=handler.parse(parsed,&ctx).await else {panic!("repaired source")};
                handler.update_status(&mut repaired,&ctx,&[]);
                assert!(repaired.spec.resolved_listeners.is_some(),"{} repaired attachment",$kind.as_str());
                assert!(repaired.status.as_ref().unwrap().parents.iter().any(|parent|parent.conditions.iter().any(|condition|condition.type_=="Accepted" && condition.status=="True")));
                assert_eq!($facade::from_route(&repaired).unwrap().effective().count(),1);
                assert!(runtime.service_ref_mgr.get_refs("default/service").contains(&ResourceRef::new($kind,None,"owner".into())));
            }
        }};
    }
    check!(
        HTTPRoute,
        HttpRouteHandler::default(),
        HttpRouteBackendRefs,
        ResourceKind::HTTPRoute,
        "http"
    );
    check!(
        GRPCRoute,
        GrpcRouteHandler::default(),
        GrpcRouteBackendRefs,
        ResourceKind::GRPCRoute,
        "http"
    );
    check!(
        TCPRoute,
        TcpRouteHandler::default(),
        TcpRouteBackendRefs,
        ResourceKind::TCPRoute,
        "tcp"
    );
    check!(
        TLSRoute,
        TlsRouteHandler::default(),
        TlsRouteBackendRefs,
        ResourceKind::TLSRoute,
        "tls"
    );
    check!(
        UDPRoute,
        UdpRouteHandler::default(),
        UdpRouteBackendRefs,
        ResourceKind::UDPRoute,
        "udp"
    );
}

#[tokio::test]
async fn all_five_apply_replace_dependencies_and_clear_stale_denial_for_invalid_targets() {
    use crate::conf_mgr::sync_runtime::resource_processor::{
        handlers::{GrpcRouteHandler, HttpRouteHandler, TcpRouteHandler, TlsRouteHandler, UdpRouteHandler},
        HandlerContext, ProcessResult, ProcessorHandler,
    };
    use std::sync::Arc;
    macro_rules! check {
        ($route:ty,$handler:expr,$facade:ident,$kind:expr) => {{
            let runtime=Arc::new(ControllerRuntime::default());
            let ctx=HandlerContext::new(None,None,Default::default(),3,20,runtime.clone());
            let handler=$handler;
            let owner=ResourceRef::new($kind,None,"owner".into());
            let route:$route=serde_json::from_value(json!({"metadata":{"name":"owner"},"spec":{"rules":[{"backendRefs":[
                {"name":"service","namespace":"old","port":80},{"name":"service","namespace":"second","port":443}
            ]}]}})).unwrap();
            let ProcessResult::Continue(mut route)=handler.parse(route,&ctx).await else {panic!("first parse")};
            assert_eq!(runtime.cross_ns_ref_mgr.get_refs("old"),vec![owner.clone()]);
            assert_eq!(runtime.cross_ns_ref_mgr.get_refs("second"),vec![owner.clone()]);
            assert_eq!(runtime.service_ref_mgr.get_refs("old/service"),vec![ResourceRef::new($kind,None,"owner".into())]);
            for backend in route.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap() {
                backend.port=Some(0);
                backend.ref_denied=Some(RefDenied{target_namespace:"old".into(),target_name:"service".into(),reason:Some("stale".into())});
            }
            let ProcessResult::Continue(route)=handler.parse(route,&ctx).await else {panic!("replacement parse")};
            assert!(runtime.cross_ns_ref_mgr.get_refs("old").is_empty());
            assert!(runtime.cross_ns_ref_mgr.get_refs("second").is_empty());
            assert!(runtime.service_ref_mgr.get_refs("old/service").is_empty());
            assert!(runtime.service_ref_mgr.get_refs("second/service").is_empty());
            assert!($facade::from_route(&route).unwrap().authored().all(|reference|reference.target_view().ref_denied().is_none()));
        }};
    }
    check!(
        HTTPRoute,
        HttpRouteHandler::default(),
        HttpRouteBackendRefs,
        ResourceKind::HTTPRoute
    );
    check!(
        GRPCRoute,
        GrpcRouteHandler::default(),
        GrpcRouteBackendRefs,
        ResourceKind::GRPCRoute
    );
    check!(
        TCPRoute,
        TcpRouteHandler::default(),
        TcpRouteBackendRefs,
        ResourceKind::TCPRoute
    );
    check!(
        TLSRoute,
        TlsRouteHandler::default(),
        TlsRouteBackendRefs,
        ResourceKind::TLSRoute
    );
    check!(
        UDPRoute,
        UdpRouteHandler::default(),
        UdpRouteBackendRefs,
        ResourceKind::UDPRoute
    );
}

#[test]
fn l4_complete_dependency_sets_are_available_before_first_authority_read() {
    macro_rules! check {
        ($route:ty,$facade:ident,$kind:expr) => {{
            let route:$route=serde_json::from_value(json!({"metadata":{"name":"owner","namespace":"source"},"spec":{"rules":[{"backendRefs":[
                {"name":"service-a","namespace":"first","port":80},
                {"name":"invalid","namespace":"invalid","port":0},
                {"name":"service-b","namespace":"last","port":443}
            ]}]}})).unwrap();
            let refs=$facade::from_route(&route).unwrap();
            for enabled in [false,true] {
                let runtime=ControllerRuntime::default();
                let owner=ResourceRef::new($kind,Some("source".into()),"owner".into());
                let receipts=collect_dependencies(refs.authored());
                register_cross_namespace(&receipts,&owner,&runtime);
                let AuthoredRouteTarget::Terminal(terminal)=refs.authored_at(0,0).unwrap().validated().unwrap() else {panic!("terminal")};
                let denial=authorize_terminal_with(terminal,&owner,&runtime,|| {
                    assert_eq!(runtime.cross_ns_ref_mgr.get_refs("first"),vec![owner.clone()]);
                    assert_eq!(runtime.cross_ns_ref_mgr.get_refs("last"),vec![owner.clone()]);
                    assert!(runtime.cross_ns_ref_mgr.get_refs("invalid").is_empty());
                    enabled
                }, |_| false);
                assert_eq!(denial.is_some(),enabled);
            }
        }};
    }
    check!(TCPRoute, TcpRouteBackendRefs, ResourceKind::TCPRoute);
    check!(TLSRoute, TlsRouteBackendRefs, ResourceKind::TLSRoute);
    check!(UDPRoute, UdpRouteBackendRefs, ResourceKind::UDPRoute);
}

#[tokio::test]
async fn remaining_route_workers_preserve_raw_owner_through_service_lifecycle() {
    use crate::conf_mgr::sync_runtime::resource_processor::{
        handlers::{
            EndpointSliceHandler, GrpcRouteHandler, ServiceHandler, TcpRouteHandler, TlsRouteHandler, UdpRouteHandler,
        },
        LocalRawStore, ProcessorHandler, RawStore, ResourceProcessor,
    };
    use edgion_resources::{BackendTLSPolicy, EdgionBackendTrafficPolicy};
    use k8s_openapi::api::{core::v1::Service, discovery::v1::EndpointSlice};
    use std::sync::Arc;
    struct PolicyObserver;
    #[async_trait::async_trait]
    impl ProcessorHandler<BackendTLSPolicy> for PolicyObserver {}
    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionBackendTrafficPolicy> for PolicyObserver {}
    macro_rules! step {
        ($processor:expr,$raw:expr,$key:expr) => {{
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
    macro_rules! check {
        ($route:ty,$handler:expr,$kind:expr,$listener:expr) => {{
            for namespace in [None, Some("default")] {
                let runtime = Arc::new(ControllerRuntime::default());
                let gateways=Arc::new(ResourceProcessor::<edgion_resources::Gateway>::new("Gateway",32,Arc::new(crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler::new(None,None)),runtime.clone()));
                let gateway_raw=Arc::new(LocalRawStore::new());
                gateway_raw.upsert("default/gateway".into(),Arc::new(serde_json::from_value(json!({"metadata":{"name":"gateway","namespace":"default"},"spec":{"gatewayClassName":"test","listeners":[{"name":"http","port":80,"protocol":"HTTP"},{"name":"tcp","port":9000,"protocol":"TCP"},{"name":"tls","port":443,"protocol":"TLS","tls":{"mode":"Passthrough"}},{"name":"udp","port":9001,"protocol":"UDP"}]}})).unwrap()));
                gateways.set_raw_store(gateway_raw); runtime.processor_registry.register(gateways);
                let routes = Arc::new(ResourceProcessor::<$route>::new($kind.as_str(),32,Arc::new($handler),runtime.clone()));
                let raw = Arc::new(LocalRawStore::new()); routes.set_raw_store(raw.clone());
                runtime.processor_registry.register(routes.clone());
                let services = Arc::new(ResourceProcessor::<Service>::new("Service",32,Arc::new(ServiceHandler::new()),runtime.clone()));
                let service_raw=Arc::new(LocalRawStore::new()); services.set_raw_store(service_raw.clone()); runtime.processor_registry.register(services.clone());
                let slices=Arc::new(ResourceProcessor::<EndpointSlice>::new("EndpointSlice",32,Arc::new(EndpointSliceHandler::new()),runtime.clone()));
                let slice_raw=Arc::new(LocalRawStore::new()); slices.set_raw_store(slice_raw.clone()); runtime.processor_registry.register(slices.clone());
                let btp=Arc::new(ResourceProcessor::<BackendTLSPolicy>::new("BackendTLSPolicy",32,Arc::new(PolicyObserver),runtime.clone()));
                let ebtp=Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new("EdgionBackendTrafficPolicy",32,Arc::new(PolicyObserver),runtime.clone()));
                runtime.processor_registry.register(btp.clone()); runtime.processor_registry.register(ebtp.clone());
                let owner=ResourceRef::new($kind,namespace.map(str::to_string),"route".into());
                let key=owner.resource_key();
                let mut route:$route=serde_json::from_value(json!({"metadata":{"name":"route","namespace":namespace,"uid":"initial","generation":1},"spec":{"hostnames":["api.example.com"],"parentRefs":[{"name":"gateway","sectionName":$listener}],"rules":[{"backendRefs":[{"name":"old","port":80}]}]}})).unwrap();
                raw.upsert(key.clone(),Arc::new(route.clone())); routes.on_event_key(key.clone()).await; step!(routes,raw,key);
                assert_eq!(runtime.service_ref_mgr.get_refs("default/old"),vec![owner.clone()]);
                assert_eq!(runtime.service_ref_mgr.get_dependencies(&owner.key()),vec!["default/old"]);
                for (generation,present) in [(2,true),(3,false),(4,true)] {
                    route.metadata.uid=Some(format!("replacement-{generation}")); route.metadata.generation=Some(generation);
                    raw.remove(&key); raw.upsert(key.clone(),Arc::new(route.clone()));
                    if present { service_raw.upsert("default/old".into(),Arc::new(serde_json::from_value(json!({"metadata":{"name":"old","namespace":"default","uid":format!("service-{generation}"),"resourceVersion":"1"},"spec":{"ports":[{"port":80}]}})).unwrap())); }
                    else { service_raw.remove("default/old"); }
                    services.on_event_key("default/old".into()).await; step!(services,service_raw,"default/old"); step!(routes,raw,key);
                    let current=routes.get(&key).unwrap(); assert_eq!(current.metadata.uid,route.metadata.uid); assert_eq!(current.metadata.generation,Some(generation));
                    let conditions=&current.status.as_ref().unwrap().parents[0].conditions;
                    assert!(conditions.iter().any(|condition|condition.type_=="Accepted" && condition.status=="True"));
                    assert!(conditions.iter().any(|condition|condition.type_=="ResolvedRefs" && condition.status==if present {"True"} else {"False"}));
                    assert_eq!(runtime.service_ref_mgr.get_refs("default/old"),vec![owner.clone()]);
                    assert_eq!(services.get("default/old").is_some(),present);
                }
                // Ordinary Service update preserves UID while changing its published port mapping.
                let mut updated_service=(*service_raw.get("default/old").unwrap()).clone();
                let service_uid=updated_service.metadata.uid.clone();
                assert_eq!(updated_service.metadata.resource_version.as_deref(),Some("1"));
                updated_service.metadata.resource_version=Some("2".into());
                updated_service.spec.as_mut().unwrap().ports.as_mut().unwrap()[0].target_port=Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(8081));
                service_raw.upsert("default/old".into(),Arc::new(updated_service));
                route.metadata.resource_version=Some("service-update".into());
                raw.upsert(key.clone(),Arc::new(route.clone()));
                services.on_event_key("default/old".into()).await;
                step!(services,service_raw,"default/old"); step!(routes,raw,key);
                let published_service=services.get("default/old").unwrap();
                assert_eq!(published_service.metadata.uid,service_uid);
                assert_eq!(published_service.metadata.resource_version.as_deref(),Some("2"));
                assert_eq!(published_service.spec.as_ref().unwrap().ports.as_ref().unwrap()[0].target_port,Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(8081)));
                let current=routes.get(&key).unwrap();
                assert_eq!(current.metadata.uid,route.metadata.uid);
                assert_eq!(current.metadata.generation,route.metadata.generation);
                assert_eq!(current.metadata.resource_version.as_deref(),Some("service-update"));
                assert_eq!(runtime.service_ref_mgr.get_refs("default/old"),vec![owner.clone()]);
                assert_eq!(runtime.service_ref_mgr.get_dependencies(&owner.key()),vec!["default/old"]);
                for present in [true,false] {
                    if present { slice_raw.upsert("default/slice".into(),Arc::new(serde_json::from_value(json!({"metadata":{"name":"slice","namespace":"default","labels":{"kubernetes.io/service-name":"old"}},"addressType":"IPv4","endpoints":[]})).unwrap())); }
                    else { slice_raw.remove("default/slice"); }
                    slices.on_event_key("default/slice".into()).await; step!(slices,slice_raw,"default/slice"); step!(routes,raw,key);
                    assert_eq!(routes.get(&key).unwrap().metadata.uid,route.metadata.uid);
                }
                for target in ["old","new","mirror-only"] {
                    runtime.service_ref_mgr.add_ref(format!("default/{target}"),ResourceRef::new(ResourceKind::BackendTLSPolicy,Some("default".into()),format!("btp-{target}")));
                    runtime.service_ref_mgr.add_ref(format!("default/{target}"),ResourceRef::new(ResourceKind::EdgionBackendTrafficPolicy,Some("default".into()),format!("ebtp-{target}")));
                }
                runtime.service_ref_mgr.add_ref(super::super::super::service_ref::mirror_service_key("default","mirror-only"),owner.clone());
                // Two source events coalesce; only the latest recreated UID is loaded.
                raw.remove(&key); routes.on_event_key(key.clone()).await;
                route.metadata.uid=Some("coalesced".into()); route.metadata.generation=Some(5);
                route.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].name="new".into();
                raw.upsert(key.clone(),Arc::new(route.clone())); routes.on_event_key(key.clone()).await;
                assert_eq!(routes.workqueue().len(),1); step!(routes,raw,key);
                assert_eq!(routes.get(&key).unwrap().metadata.uid.as_deref(),Some("coalesced"));
                assert_eq!(runtime.service_ref_mgr.get_dependencies(&owner.key()),vec!["default/new"]);
                assert!(!runtime.service_ref_mgr.get_refs("default/old").contains(&owner));
                for (processor,prefix) in [(btp.workqueue(),"btp"),(ebtp.workqueue(),"ebtp")] {
                    let mut seen=std::collections::BTreeSet::new();
                    for _ in 0..2 { let item=tokio::time::timeout(std::time::Duration::from_secs(3),processor.dequeue()).await.unwrap().unwrap(); seen.insert(item.key.clone()); processor.done(&item.key); }
                    assert_eq!(seen,std::collections::BTreeSet::from([format!("default/{prefix}-old"),format!("default/{prefix}-new")]));
                    assert!(tokio::time::timeout(processor.config().default_requeue_delay+std::time::Duration::from_millis(20),processor.dequeue()).await.is_err());
                }
                raw.remove(&key); routes.on_event_key(key.clone()).await; step!(routes,raw,key);
                assert!(routes.get(&key).is_none()); assert!(runtime.service_ref_mgr.get_dependencies(&owner.key()).is_empty());
                assert!(!runtime.service_ref_mgr.get_refs("default/new").contains(&owner));
                for queue in [btp.workqueue(),ebtp.workqueue()] { let item=tokio::time::timeout(std::time::Duration::from_secs(3),queue.dequeue()).await.unwrap().unwrap(); assert!(item.key.ends_with("-new")); queue.done(&item.key); }
                route.metadata.uid=Some("after-delete".into()); route.metadata.generation=Some(6);
                raw.upsert(key.clone(),Arc::new(route)); routes.on_event_key(key.clone()).await; step!(routes,raw,key);
                assert_eq!(routes.get(&key).unwrap().metadata.uid.as_deref(),Some("after-delete"));
                assert_eq!(runtime.service_ref_mgr.get_dependencies(&owner.key()),vec!["default/new"]);
            }
        }};
    }
    check!(GRPCRoute, GrpcRouteHandler::default(), ResourceKind::GRPCRoute, "http");
    check!(TCPRoute, TcpRouteHandler::default(), ResourceKind::TCPRoute, "tcp");
    check!(TLSRoute, TlsRouteHandler::default(), ResourceKind::TLSRoute, "tls");
    check!(UDPRoute, UdpRouteHandler::default(), ResourceKind::UDPRoute, "udp");
}
