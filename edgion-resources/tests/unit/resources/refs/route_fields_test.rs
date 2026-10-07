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
