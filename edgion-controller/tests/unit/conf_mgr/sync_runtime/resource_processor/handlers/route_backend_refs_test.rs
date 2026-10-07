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
    let dropped = super::super::route_utils::register_service_backend_refs(
        ResourceKind::HTTPRoute,
        "default",
        "owner",
        &receipts,
        &runtime,
    );
    assert!(dropped.is_empty());
    assert_eq!(
        runtime.service_ref_mgr.get_refs("default/service"),
        vec![ResourceRef::new(
            ResourceKind::HTTPRoute,
            Some("default".into()),
            "owner".into()
        )]
    );
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
                assert!(runtime.service_ref_mgr.get_refs("default/service").contains(&ResourceRef::new($kind,Some("default".into()),"owner".into())));
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
            assert_eq!(runtime.service_ref_mgr.get_refs("old/service"),vec![ResourceRef::new($kind,Some("default".into()),"owner".into())]);
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
