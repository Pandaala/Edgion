use super::*;
use serde_json::json;

fn wire() -> serde_json::Value {
    json!({"metadata":{"name":"route","namespace":"apps","uid":"owner-a","generation":1},"spec":{"rules":[{
        "filters":[{"type":"ExternalAuth","externalAuth":{"target":{"name":"auth","group":"core","port":8080},"decision":{},
            "tls":{"enabled":true,"validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},"clientCertificateRef":{"name":"client"}},
            "request":{"path":{"template":"/check/${secretRef:secrets/token:key}"}}}}],
        "backendRefs":[{"name":"backend","port":80,"filters":[{"type":"ExternalAuth","externalAuth":{"target":{"url":"https://auth.example"},"decision":{}}}]}]
    }]}})
}

#[test]
fn authored_auth_positions_and_permission_belong_to_the_declaring_route() {
    let route: HTTPRoute = serde_json::from_value(wire()).unwrap();
    let refs = HttpRouteAuthRefs::from_route(&route).unwrap();
    let slots = refs.authored_slots();
    assert_eq!(slots.len(), 2);
    assert_eq!(slots[0].backend(), None);
    assert_eq!(slots[1].backend(), Some(0));
    let prepared = PreparedRouteAuthField::from_route(&route, slots[0]).unwrap();
    let field = prepared.bind(&route).unwrap();
    assert!(std::ptr::eq(
        field.config(),
        route.spec.rules.as_ref().unwrap()[0].filters.as_ref().unwrap()[0]
            .external_auth
            .as_ref()
            .unwrap()
            .edgion_config()
            .unwrap()
    ));
    let ca = field.ca_certificates().next().unwrap();
    let bound = ca.validated().unwrap();
    assert_eq!(bound.owner().name(), "route");
    assert_eq!(bound.source().scope().namespace(), Some("apps"));
    assert_eq!(bound.target().scope().namespace(), Some("apps"));
    assert!(ca.freeze().is_err());
    let template = field.request_secrets().next().unwrap();
    assert_eq!(
        template.reference().validated().unwrap().target().scope().namespace(),
        Some("secrets")
    );
    assert_eq!(
        template
            .reference()
            .validated()
            .unwrap()
            .grant_query()
            .source()
            .group_kind()
            .kind(),
        "HTTPRoute"
    );
}

#[test]
fn authored_auth_rebind_checks_reference_topology_but_not_literal_template_text() {
    let route: HTTPRoute = serde_json::from_value(wire()).unwrap();
    let slot = HttpRouteAuthRefs::from_route(&route).unwrap().authored_slots()[0];
    let prepared = PreparedRouteAuthField::from_route(&route, slot).unwrap();
    for pointer in [
        "/metadata/uid",
        "/metadata/namespace",
        "/spec/rules/0/filters/0/externalAuth/target/name",
        "/spec/rules/0/filters/0/externalAuth/tls/clientCertificateRef/name",
    ] {
        let mut changed = wire();
        *changed.pointer_mut(pointer).unwrap() = json!("different");
        let changed: HTTPRoute = serde_json::from_value(changed).unwrap();
        assert!(prepared.bind(&changed).is_err(), "{pointer}");
    }
    let mut changed = wire();
    changed["spec"]["rules"][0]["filters"][0]["externalAuth"]["request"]["path"]["template"] =
        json!("/other/${secretRef:secrets/token:key}");
    let changed: HTTPRoute = serde_json::from_value(changed).unwrap();
    assert!(prepared.bind(&changed).is_ok());
    let mut moved = wire();
    moved["spec"]["rules"][0]["filters"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!({"type":"RequestHeaderModifier","requestHeaderModifier":{}}));
    assert!(prepared.bind(&serde_json::from_value(moved).unwrap()).is_err());
}

#[test]
fn auth_source_target_and_union_fail_before_authored_tokens() {
    for (pointer, invalid) in [
        ("/metadata/name", json!("bad/name")),
        ("/metadata/namespace", json!("")),
        ("/spec/rules/0/filters/0/externalAuth/target/group", json!("example.io")),
        ("/spec/rules/0/filters/0/externalAuth/target/name", json!("bad/name")),
        ("/spec/rules/0/filters/0/externalAuth/target/port", json!(0)),
        ("/spec/rules/0/filters/0/type", json!("CORS")),
    ] {
        let mut value = wire();
        *value.pointer_mut(pointer).unwrap() = invalid;
        let route: HTTPRoute = serde_json::from_value(value).unwrap();
        let slot = RouteFilterSlot::new(0, None, 0);
        assert!(PreparedRouteAuthField::from_route(&route, slot).is_err(), "{pointer}");
    }
    for payload in [
        json!({"backendRef":{"name":"auth","port":80},"protocol":"HTTP"}),
        json!({"target":{"url":"https://auth.example"},"decision":{},"backendRef":{"name":"native","port":80}}),
    ] {
        let mut value = wire();
        value["spec"]["rules"][0]["filters"][0]["externalAuth"] = payload;
        let route: HTTPRoute = serde_json::from_value(value).unwrap();
        assert!(PreparedRouteAuthField::from_route(&route, RouteFilterSlot::new(0, None, 0)).is_err());
    }
}

#[test]
fn route_auth_transport_is_schema_hidden_and_disabled_tls_has_no_active_material_loan() {
    let mut value = wire();
    value["spec"]["rules"][0]["filters"][0]["resolvedExternalAuth"] = json!({"namespace":"apps","error":"pending"});
    value["spec"]["rules"][0]["filters"][0]["externalAuth"]["tls"]["enabled"] = json!(false);
    let route: HTTPRoute = serde_json::from_value(value).unwrap();
    let refs = HttpRouteAuthRefs::from_route(&route).unwrap();
    let slot = refs.authored_slots()[0];
    let prepared = PreparedRouteAuthField::from_route(&route, slot).unwrap();
    let authored = prepared.bind(&route).unwrap();
    assert_eq!(authored.ca_certificates().count(), 0);
    assert!(authored.client_certificate().is_none());
    assert_eq!(authored.request_secrets().count(), 1);
    let effective = refs.effective_at(0, None, 0).unwrap();
    assert!(effective.outbound_tls_selection().unwrap().active_field().is_none());
    assert_eq!(effective.resolution_error(), Some("pending"));
    let transported = serde_json::to_value(&route).unwrap();
    assert_eq!(
        transported["spec"]["rules"][0]["filters"][0]["resolvedExternalAuth"]["namespace"],
        json!("apps")
    );
    let schema = schemars::schema_for!(HTTPRouteFilter);
    assert!(!serde_json::to_string(&schema).unwrap().contains("resolvedExternalAuth"));
}
