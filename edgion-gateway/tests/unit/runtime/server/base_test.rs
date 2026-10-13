use super::*;

#[test]
fn parse_enable_http2_annotation_preserves_frontend_policy() {
    let mut gateway: Gateway = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "http2-policy"},
        "spec": {"gatewayClassName": "public"}
    }))
    .unwrap();
    assert!(parse_enable_http2_annotation(&gateway));
    gateway.metadata.annotations = Some(Default::default());
    assert!(parse_enable_http2_annotation(&gateway));
    for (value, enabled) in [
        ("false", false),
        ("FALSE", false),
        ("FaLsE", false),
        ("0", false),
        ("no", false),
        ("NO", false),
        ("No", false),
        ("off", false),
        ("OFF", false),
        ("OfF", false),
        ("true", true),
        ("TRUE", true),
        ("1", true),
        ("arbitrary", true),
        ("", true),
        (" false ", true),
    ] {
        gateway.metadata.annotations.as_mut().unwrap().insert(
            edgion_resources::constants::annotations::edgion::ENABLE_HTTP2.into(),
            value.into(),
        );
        assert_eq!(parse_enable_http2_annotation(&gateway), enabled, "value: {value:?}");
    }
}

#[test]
fn same_port_distinct_host_ips_are_independent() {
    let first = "192.0.2.10:8080".parse().unwrap();
    let second = "192.0.2.11:8080".parse().unwrap();
    let wildcard = "0.0.0.0:8080".parse().unwrap();
    assert!(!socket_bindings_overlap(first, false, second, false));
    assert!(socket_bindings_overlap(wildcard, false, first, false));
    assert!(socket_bindings_overlap(first, false, first, false));
    assert!(!socket_bindings_overlap(first, false, first, true));
    assert!(!socket_bindings_overlap(
        wildcard,
        false,
        "[::1]:8080".parse().unwrap(),
        false
    ));
    assert!(socket_bindings_overlap(
        "[::]:8080".parse().unwrap(),
        false,
        first,
        false
    ));
    assert!(socket_bindings_overlap(
        first,
        false,
        "[::ffff:192.0.2.10]:8080".parse().unwrap(),
        false
    ));
}

#[test]
fn startup_parameters_validate_before_any_target_read() {
    use serde_json::json;
    let valid = json!({"metadata":{"name":"class"},"spec":{"controllerName":"example.net/controller","parametersRef":{"group":"edgion.io","kind":"EdgionGatewayConfig","name":"config"}}});
    for (field, value) in [
        ("group", "wrong.io"),
        ("kind", "Secret"),
        ("name", "bad/name"),
        ("name", ""),
        ("namespace", ""),
        ("namespace", "default"),
    ] {
        let mut raw = valid.clone();
        raw["spec"]["parametersRef"][field] = json!(value);
        let class = serde_json::from_value(raw).unwrap();
        assert!(resolve_gateway_config_name(&class, |_| panic!("invalid reference queried target")).is_err());
    }
    let mut class: GatewayClass = serde_json::from_value(valid).unwrap();
    let mut reads = Vec::new();
    assert_eq!(
        resolve_gateway_config_name(&class, |name| {
            reads.push(name.to_owned());
            true
        })
        .unwrap(),
        Some("config".into())
    );
    assert_eq!(reads, ["config"]);
    assert!(resolve_gateway_config_name(&class, |_| false).is_err());
    class.spec.parameters_ref = None;
    assert!(
        resolve_gateway_config_name(&class, |_| panic!("defaults queried target"))
            .unwrap()
            .is_none()
    );
}
