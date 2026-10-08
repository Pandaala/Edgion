use super::*;
use serde_json::json;

#[test]
fn five_actual_owners_preserve_defaults_positions_and_terminal_authority() {
    macro_rules! check {
        ($route:ty, $facade:ident, $kind:expr) => {{
            let route: $route = serde_json::from_value(json!({"metadata":{"name":"route","uid":"uid-a","generation":7}, "spec":{"rules":[{"backendRefs":[{"name":"service","port":1},{"name":"service","port":65535}]}]}})).unwrap();
            let facade = $facade::from_route(&route).unwrap();
            let refs: Vec<_> = facade.authored().collect();
            assert_eq!(refs.len(), 2);
            for (index, reference) in refs.into_iter().enumerate() {
                let AuthoredRouteTarget::Terminal(terminal) = reference.validated().unwrap() else { panic!("expected terminal") };
                assert_eq!(terminal.source_kind(), $kind);
                assert_eq!(terminal.source_uid(), Some("uid-a"));
                assert_eq!(terminal.source_generation(), Some(7));
                assert_eq!(terminal.source().scope().namespace(), Some("default"));
                assert_eq!(terminal.slot().rule(), 0);
                assert_eq!(terminal.slot().backend(), index);
                assert_eq!(terminal.port(), Some(if index == 0 { 1 } else { 65535 }));
                let query = terminal.grant_query();
                assert_eq!(query.target().group_kind().group(), "");
                assert_eq!(query.target().group_kind().kind(), "Service");
                assert_eq!(query.source().group_kind().kind(), $kind.as_str());
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
fn five_owners_reject_invalid_source_and_target_without_losing_slots() {
    macro_rules! check {
        ($route:ty, $facade:ident) => {{
            let value = json!({"metadata":{"name":"route","namespace":"routes"}, "spec":{"rules":[{"backendRefs":[{"name":"service","port":80,"weight":7}]}]}});
            for (field, invalid) in [("name", ""), ("name", "UPPER"), ("name", "bad/name"), ("namespace", ""), ("namespace", "bad/ns")] {
                let mut bad = value.clone(); bad["metadata"][field] = json!(invalid);
                let route: $route = serde_json::from_value(bad).unwrap();
                assert!($facade::from_route(&route).is_err(), "{field}={invalid}");
            }
            for (field, invalid) in [("group",json!("core")),("group",json!("CORE")),("group",json!("example.io")),("kind",json!("")),("kind",json!("service")),("kind",json!("ServiceImport")),("kind",json!("ServiceExternalName")),("name",json!("*")),("name",json!("bad/name")),("namespace",json!("")),("port",json!(null)),("port",json!(0)),("port",json!(-1)),("port",json!(65536))] {
                let mut bad=value.clone(); bad["spec"]["rules"][0]["backendRefs"][0][field]=invalid;
                let route: $route=serde_json::from_value(bad).unwrap();
                let facade=$facade::from_route(&route).unwrap();
                let reference=facade.authored_at(0,0).unwrap();
                assert!(reference.validated().is_err(), "{field}");
                let effective=reference.target_view();
                assert_eq!(effective.weight(),Some(7));
                assert_eq!(effective.slot().backend(),0);
            }
            let mut explicit=value; explicit["spec"]["rules"][0]["backendRefs"][0]["group"]=json!(""); explicit["spec"]["rules"][0]["backendRefs"][0]["kind"]=json!("Service");
            let route: $route=serde_json::from_value(explicit).unwrap();
            assert!($facade::from_route(&route).unwrap().authored_at(0,0).unwrap().validated().is_ok());
        }};
    }
    check!(HTTPRoute, HttpRouteBackendRefs);
    check!(GRPCRoute, GrpcRouteBackendRefs);
    check!(TCPRoute, TcpRouteBackendRefs);
    check!(TLSRoute, TlsRouteBackendRefs);
    check!(UDPRoute, UdpRouteBackendRefs);
}

#[test]
fn delegation_selectors_and_ai_targets_keep_distinct_contracts() {
    for kind in ["HTTPRoute", "GRPCRoute"] {
        for name in ["child", "*"] {
            let value = json!({"metadata":{"name":"parent","namespace":"root"},"spec":{"rules":[{"backendRefs":[{"group":GATEWAY_API_GROUP,"kind":kind,"namespace":"children","name":name}]}]}});
            let classify = |target: AuthoredRouteTarget<'_>| match target {
                AuthoredRouteTarget::NamedRoute(reference) => {
                    assert_eq!(name, "child");
                    assert_eq!(reference.target().scope().namespace(), Some("children"));
                }
                AuthoredRouteTarget::NamespaceRoutes(reference) => {
                    assert_eq!(name, "*");
                    assert_eq!(reference.selector().namespace(), "children");
                }
                _ => panic!("delegation cannot be terminal"),
            };
            if kind == "HTTPRoute" {
                let route: HTTPRoute = serde_json::from_value(value).unwrap();
                classify(
                    HttpRouteBackendRefs::from_route(&route)
                        .unwrap()
                        .authored_at(0, 0)
                        .unwrap()
                        .validated()
                        .unwrap(),
                );
            } else {
                let route: GRPCRoute = serde_json::from_value(value).unwrap();
                classify(
                    GrpcRouteBackendRefs::from_route(&route)
                        .unwrap()
                        .authored_at(0, 0)
                        .unwrap()
                        .validated()
                        .unwrap(),
                );
            }
        }
    }
    let mut route:HTTPRoute=serde_json::from_value(json!({"metadata":{"name":"route"},"spec":{"rules":[{"backendRefs":[{"group":"edgion.io","kind":"EdgionBackend","name":"ai"}]}],"resolvedRules":[{"backendRefs":[{"name":"effective","port":80}]}]}})).unwrap();
    assert!(matches!(
        HttpRouteBackendRefs::from_route(&route)
            .unwrap()
            .authored_at(0, 0)
            .unwrap()
            .validated(),
        Ok(AuthoredRouteTarget::Terminal(_))
    ));
    assert_eq!(
        HttpRouteBackendRefs::from_route(&route)
            .unwrap()
            .effective()
            .next()
            .unwrap()
            .name(),
        "effective"
    );
    route.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].port = Some(443);
    assert!(HttpRouteBackendRefs::from_route(&route)
        .unwrap()
        .authored_at(0, 0)
        .unwrap()
        .validated()
        .is_err());
}

#[test]
fn indexed_effective_loans_keep_actual_owner_rule_and_backend_positions() {
    macro_rules! check {
        ($route:ty, $facade:ident) => {{
            let route: $route = serde_json::from_value(json!({"metadata":{"name":"owner"},"spec":{
                "rules":[{"backendRefs":[{"name":"authored","port":80}]}],
                "resolvedRules":[{}, {"backendRefs":[{"name":"child","namespace":"children","port":65535,"weight":0}]}]
            }})).unwrap();
            let refs = $facade::from_route(&route).unwrap();
            assert!(refs.effective_at(0,0).is_none());
            assert!(refs.effective_at(2,0).is_none());
            assert!(refs.effective_at(1,1).is_none());
            let loan = refs.effective_at(1,0).unwrap();
            assert_eq!(loan.slot().rule(), 1);
            assert_eq!(loan.slot().backend(), 0);
            assert_eq!(loan.weight(), Some(0));
            assert_eq!(loan.port(), Some(65535));
            let BackendTarget::Service(target) = loan.target().unwrap() else { panic!("expected Service") };
            assert_eq!(target.scope().namespace(), Some("children"));
            assert_eq!(target.name(), "child");
            assert_eq!(refs.authored_at(0,0).unwrap().target_view().name(), "authored");
        }};
    }
    check!(HTTPRoute, HttpRouteBackendRefs);
    check!(GRPCRoute, GrpcRouteBackendRefs);
}

#[test]
fn extension_loans_bind_actual_owners_positions_and_authored_vs_effective_locality() {
    macro_rules! check {
        ($route:ty, $facade:ident, $kind:expr) => {{
            let filter = json!({"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"bundle","resolvedNamespace":"forged"}});
            let rule = json!({"filters":[filter.clone()],"backendRefs":[{"name":"service","port":80,"filters":[filter.clone(),filter.clone()]}]});
            let mut route: $route = serde_json::from_value(json!({"metadata":{"name":"root","uid":"root-uid","generation":7},"spec":{"rules":[rule]}})).unwrap();
            let refs = $facade::from_route(&route).unwrap();
            for (index, field) in refs.authored().enumerate() {
                let lookup = field.validated().unwrap().unwrap();
                assert_eq!(lookup.source_kind(), $kind);
                assert_eq!(lookup.source_uid(), Some("root-uid"));
                assert_eq!(lookup.source_generation(), Some(7));
                assert_eq!(lookup.target().scope().namespace(), Some("default"));
                assert_eq!(lookup.source().scope().namespace(), Some("default"));
                assert_eq!(lookup.slot().rule(), 0);
                assert_eq!(lookup.slot().backend(), if index == 0 { None } else { Some(0) });
                assert_eq!(lookup.slot().filter(), index.saturating_sub(1));
            }
            assert_eq!(refs.authored().count(), 3);
            assert_eq!(refs.effective_at(0, None, 0).unwrap().validated().unwrap().unwrap().target().scope().namespace(), Some("default"));
            route.spec.resolved_rules = route.spec.rules.clone();
            let refs = $facade::from_route(&route).unwrap();
            let actual = refs.effective_at(0, Some(0), 1).unwrap().validated().unwrap().unwrap();
            assert_eq!(actual.target().scope().namespace(), Some("forged"));
            assert_eq!(actual.source_uid(), Some("root-uid"), "effective transport describes root, not invented child provenance");
            assert!(std::ptr::eq(actual.target().name().as_ptr(), route.spec.resolved_rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0].filters.as_ref().unwrap()[1].extension_ref.as_ref().unwrap().name.as_ptr()));
            for bad in ["", "Bad", "bad/ns"] {
                route.spec.resolved_rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0].extension_ref.as_mut().unwrap().resolved_namespace = Some(bad.into());
                let refs = $facade::from_route(&route).unwrap();
                assert!(refs.effective_at(0, None, 0).unwrap().validated().is_err());
                assert!(refs.authored_at(0, None, 0).unwrap().validated().unwrap().is_some());
            }
            let mut missing_name = route.clone();
            missing_name.metadata.name = None;
            assert!($facade::from_route(&missing_name).is_err());
            for (field, value) in [("name", ""), ("name", "bad/name"), ("namespace", ""), ("namespace", "Bad")] {
                let mut invalid = serde_json::to_value(&route).unwrap(); invalid["metadata"][field] = json!(value);
                let invalid: $route = serde_json::from_value(invalid).unwrap();
                assert!($facade::from_route(&invalid).is_err(), "{field}={value}");
            }
        }};
    }
    check!(HTTPRoute, HttpRouteExtensionRefs, ResourceKind::HTTPRoute);
    check!(GRPCRoute, GrpcRouteExtensionRefs, ResourceKind::GRPCRoute);
}

#[test]
fn extension_field_validation_rejects_bad_union_and_target_without_compacting_positions() {
    for patch in [
        json!({"group":""}),
        json!({"group":"core"}),
        json!({"kind":"Secret"}),
        json!({"name":""}),
        json!({"name":"bad/name"}),
    ] {
        let mut reference = json!({"group":"edgion.io","kind":"EdgionPlugins","name":"bundle"});
        reference
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let route:HTTPRoute=serde_json::from_value(json!({"metadata":{"name":"route","namespace":"apps"},"spec":{"rules":[{"filters":[{"type":"ExtensionRef","extensionRef":reference},{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"valid"}}]}]}})).unwrap();
        let refs = HttpRouteExtensionRefs::from_route(&route).unwrap();
        assert!(refs.authored_at(0, None, 0).unwrap().validated().is_err());
        let valid = refs.authored_at(0, None, 1).unwrap().validated().unwrap().unwrap();
        assert_eq!(valid.slot().filter(), 1);
        assert_eq!(valid.target().name(), "valid");
    }
}
