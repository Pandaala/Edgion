use super::*;

fn startup_context(gateway: &Gateway) -> listener_builder::ListenerContext {
    let listener = gateway.spec.listeners.as_ref().unwrap()[0].clone();
    listener_builder::ListenerContext {
        gateway_namespace: gateway.metadata.namespace.clone(),
        gateway_name: gateway.name_any(),
        gateway_key: gateway.key_name(),
        listener_key: ListenerKey::new(resolve_listener_addr(gateway, listener.port), &listener.protocol),
        listener,
        bind_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        access_logger: Arc::new(AccessLogger::new()),
        egc: crate::config::bind_gateway_config(None),
        server_conf: Arc::new(Default::default()),
        downstream_h2_policy: DownstreamH2Policy::try_from(&crate::cli::config::ServerConfig::default()).unwrap(),
        downstream_connections: Arc::new(
            crate::runtime::server::downstream_connections::ConnectionCounter::unlimited(),
        ),
        enable_http2: parse_enable_http2_annotation(gateway),
        gateway_annotations: gateway
            .metadata
            .annotations
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect(),
        proxy_protocol_policy: PreparedProxyProtocolPolicy::Disabled,
        proxy_protocol_source: None,
    }
}

fn startup_gateway(name: &str, protocol: &str) -> Gateway {
    crate::test_current_status::from_value(serde_json::json!({
        "metadata": {"name": name, "namespace": "test"},
        "spec": {"gatewayClassName": "shared", "listeners": [{"name": "listener", "port": 8443, "protocol": protocol}]}
    }))
    .unwrap()
}

#[test]
fn startup_prefers_admitted_physical_policy_over_native_fallback_in_both_orders() {
    use edgion_resources::resources::gateway::HttpServiceDeclaration;
    for protocol in ["HTTP", "HTTPS"] {
        let mut pending = startup_gateway("pending", protocol);
        pending.spec.current_status = None;
        pending.metadata.annotations = Some(std::collections::BTreeMap::from([
            ("edgion.io/http-to-https-redirect".into(), "true".into()),
            ("edgion.io/enable-http2".into(), "true".into()),
        ]));
        let mut admitted = startup_gateway("admitted", protocol);
        admitted.metadata.annotations = Some(std::collections::BTreeMap::from([(
            "edgion.io/enable-http2".into(),
            "false".into(),
        )]));
        for gateways in [[&pending, &admitted], [&admitted, &pending]] {
            let contexts = gateways
                .into_iter()
                .map(|gateway| {
                    (
                        startup_listener_priority(gateway, "listener").unwrap(),
                        startup_context(gateway),
                    )
                })
                .collect();
            let selected = select_startup_listener_contexts(contexts).unwrap();
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].gateway_name, "admitted");
            assert!(!selected[0].enable_http2);
            assert_eq!(
                HttpServiceDeclaration::from_annotation_lookup(protocol, |key| {
                    selected[0].gateway_annotations.get(key).map(String::as_str)
                }),
                HttpServiceDeclaration::from_gateway(&admitted, protocol)
            );
        }
        let selected = select_startup_listener_contexts(vec![(true, startup_context(&pending))]).unwrap();
        assert_eq!(selected.len(), 1, "native fallback must retain a physical acceptor");
        assert_eq!(selected[0].gateway_name, "pending");
        let equal = select_startup_listener_contexts(vec![
            (false, startup_context(&admitted)),
            (false, startup_context(&pending)),
        ])
        .unwrap();
        assert_eq!(
            equal[0].gateway_name, "admitted",
            "equal priority preserves source order"
        );
    }
}

#[test]
fn startup_excludes_policy_conflicts_but_keeps_recoverable_native_tls() {
    let mut gateway = startup_gateway("tls", "HTTPS");
    let conditions = &mut gateway
        .spec
        .current_status
        .as_mut()
        .unwrap()
        .listeners
        .as_mut()
        .unwrap()[0]
        .conditions;
    let mut programmed = conditions[0].clone();
    programmed.type_ = "Programmed".into();
    programmed.status = "False".into();
    programmed.reason = "Invalid".into();
    conditions.push(programmed);
    assert_eq!(startup_listener_priority(&gateway, "listener"), Some(true));
    let conditions = &mut gateway
        .spec
        .current_status
        .as_mut()
        .unwrap()
        .listeners
        .as_mut()
        .unwrap()[0]
        .conditions;
    let accepted = conditions
        .iter_mut()
        .find(|condition| condition.type_ == "Accepted")
        .unwrap();
    accepted.status = "False".into();
    accepted.reason = "SharedListenerPolicyConflict".into();
    assert_eq!(startup_listener_priority(&gateway, "listener"), None);
    let conditions = &mut gateway
        .spec
        .current_status
        .as_mut()
        .unwrap()
        .listeners
        .as_mut()
        .unwrap()[0]
        .conditions;
    conditions
        .iter_mut()
        .find(|condition| condition.type_ == "Accepted")
        .unwrap()
        .reason = "Invalid".into();
    let mut conflicted = conditions[0].clone();
    conflicted.type_ = "Conflicted".into();
    conflicted.status = "True".into();
    conditions.push(conflicted);
    assert_eq!(startup_listener_priority(&gateway, "listener"), None);
}

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
