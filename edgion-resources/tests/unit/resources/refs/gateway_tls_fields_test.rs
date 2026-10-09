use super::*;
use serde_json::json;

fn wire() -> serde_json::Value {
    json!({"metadata":{"name":"edge","namespace":"apps"},"spec":{"gatewayClassName":"edge",
        "listeners":[{"name":"https","port":443,"protocol":"HTTPS","tls":{"certificateRefs":[{"name":"cert"}]}}],
        "tls":{"backend":{"clientCertificateRef":{"name":"client"}},"frontend":{"default":{"validation":{"caCertificateRefs":[{"group":"","kind":"ConfigMap","name":"ca"}]}}}}}})
}

#[test]
fn native_gateway_tls_fields_bind_actual_owner_and_never_freeze() {
    let gateway: Gateway = serde_json::from_value(wire()).unwrap();
    let refs = GatewayTlsRefs::from_gateway(&gateway);
    for (field, name, kind) in [
        (refs.backend_client().unwrap(), "client", "Secret"),
        (refs.serving(0, 0).unwrap(), "cert", "Secret"),
        (refs.frontend_ca(0, 0).unwrap(), "ca", "ConfigMap"),
    ] {
        let token = field.token();
        let bound = token.validated().unwrap();
        assert_eq!(bound.owner().name(), "edge");
        assert_eq!(bound.source().group_kind().kind(), "Gateway");
        assert_eq!(bound.source().scope().namespace(), Some("apps"));
        assert_eq!(bound.target().name(), name);
        assert_eq!(bound.target().group_kind().kind(), kind);
        assert_eq!(bound.target().scope().namespace(), Some("apps"));
        assert!(bound.is_gateway_tls());
        assert_eq!(bound.is_gateway_frontend_ca(), kind == "ConfigMap");
        assert!(token.freeze().is_err());
    }
    assert!(refs.serving(0, 1).is_none());
    assert!(refs.frontend_ca(1, 0).is_none());
}

#[test]
fn native_gateway_tls_defaults_and_object_name_character_boundaries() {
    for name in ["Capital_Name".to_string(), "é".repeat(253)] {
        for group in [None, Some("")] {
            let mut value = wire();
            value["metadata"].as_object_mut().unwrap().remove("namespace");
            value["spec"]["listeners"][0]["tls"]["certificateRefs"][0]["name"] = json!(name);
            if let Some(group) = group {
                value["spec"]["listeners"][0]["tls"]["certificateRefs"][0]["group"] = json!(group);
            }
            value["spec"]["tls"]["frontend"]["default"]["validation"]["caCertificateRefs"][0]["name"] = json!(name);
            let gateway: Gateway = serde_json::from_value(value).unwrap();
            let refs = GatewayTlsRefs::from_gateway(&gateway);
            for field in [refs.serving(0, 0).unwrap(), refs.frontend_ca(0, 0).unwrap()] {
                let token = field.token();
                let bound = token.validated().unwrap();
                assert_eq!(bound.target().name(), name);
                assert_eq!(bound.target().scope().namespace(), Some("default"));
                assert_eq!(bound.source().scope().namespace(), Some("default"));
            }
        }
    }
}

#[test]
fn native_gateway_tls_rejects_invalid_source_target_and_core_alias() {
    for (key, value) in [
        ("group", json!("core")),
        ("group", json!("foreign.example")),
        ("kind", json!("")),
        ("kind", json!("Service")),
        ("name", json!("")),
        ("name", json!("é".repeat(254))),
        ("namespace", json!("")),
        ("namespace", json!("Bad_Name")),
    ] {
        let mut raw = wire();
        raw["spec"]["listeners"][0]["tls"]["certificateRefs"][0][key] = value.clone();
        raw["spec"]["tls"]["backend"]["clientCertificateRef"][key] = value.clone();
        let mut gateway: Gateway = serde_json::from_value(raw).unwrap();
        let field = &mut gateway
            .spec
            .tls
            .as_mut()
            .unwrap()
            .frontend
            .as_mut()
            .unwrap()
            .default
            .validation
            .as_mut()
            .unwrap()
            .ca_certificate_refs[0];
        let invalid = value.as_str().unwrap().to_owned();
        match key {
            "group" => field.group = invalid,
            "kind" => field.kind = invalid,
            "name" => field.name = invalid,
            "namespace" => field.namespace = Some(invalid),
            _ => unreachable!(),
        }
        if matches!(key, "kind" | "name" | "namespace") && value == json!("") {
            let mut decoded = wire();
            decoded["spec"]["tls"]["frontend"]["default"]["validation"]["caCertificateRefs"][0][key] = value;
            assert!(serde_json::from_value::<Gateway>(decoded).is_err());
        }
        let refs = GatewayTlsRefs::from_gateway(&gateway);
        for field in [
            refs.serving(0, 0).unwrap(),
            refs.backend_client().unwrap(),
            refs.frontend_ca(0, 0).unwrap(),
        ] {
            assert!(field.token().validated().is_err(), "{key}");
        }
    }
    for (key, value) in [
        ("name", json!(null)),
        ("name", json!("bad/name")),
        ("namespace", json!("")),
    ] {
        let mut raw = wire();
        raw["metadata"][key] = value;
        let gateway: Gateway = serde_json::from_value(raw).unwrap();
        let refs = GatewayTlsRefs::from_gateway(&gateway);
        for field in [
            refs.serving(0, 0).unwrap(),
            refs.backend_client().unwrap(),
            refs.frontend_ca(0, 0).unwrap(),
        ] {
            assert!(field.token().validated().is_err());
        }
    }
    for key in ["group", "kind"] {
        let mut raw = wire();
        raw["spec"]["tls"]["frontend"]["default"]["validation"]["caCertificateRefs"][0]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(
            serde_json::from_value::<Gateway>(raw).is_err(),
            "native {key} remains required"
        );
    }
}

#[test]
fn native_frontend_selection_uses_declaration_and_authoritative_port() {
    let mut raw = wire();
    raw["spec"]["tls"]["frontend"]["perPort"] = json!([{"port":443,"tls":{}}]);
    raw["spec"]["listeners"][0]["tls"]["frontendValidation"] =
        json!({"caCertificateRefs":[{"group":"","kind":"Secret","name":"injected"}]});
    let gateway: Gateway = serde_json::from_value(raw.clone()).unwrap();
    assert!(GatewayTlsRefs::from_gateway(&gateway).frontend_ca(0, 0).is_none());
    raw["spec"]["tls"]["frontend"]["perPort"][0]["tls"]["validation"] =
        json!({"caCertificateRefs":[{"group":"","kind":"Secret","name":"port-ca"}]});
    let gateway: Gateway = serde_json::from_value(raw.clone()).unwrap();
    let refs = GatewayTlsRefs::from_gateway(&gateway);
    let field = refs.frontend_ca(0, 0).unwrap();
    assert_eq!(field.token().validated().unwrap().target().name(), "port-ca");
    raw["spec"]["listeners"][0]["protocol"] = json!("TLS");
    let gateway: Gateway = serde_json::from_value(raw).unwrap();
    assert!(GatewayTlsRefs::from_gateway(&gateway).frontend_validation(0).is_none());
}
