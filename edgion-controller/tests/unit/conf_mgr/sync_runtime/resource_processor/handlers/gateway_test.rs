use super::{
    assigned_service_addresses, build_gateway_attachment_proof, classify_dedicated_addresses, compute_supported_kinds,
    dedicated_gateway_service_name, derive_gateway_addresses, gateway_service_lookup_candidates,
    has_all_ca_refs_invalid, requested_addresses_match_assigned, resolve_gateway_stream_plugin_ref,
    resolve_inbound_proxy_protocol, validate_gateway_dns_resolver_ref, validate_gateway_plugin_ref,
    validate_gateway_stream_plugin_ref, validate_listener_resolved_refs, DedicatedAddressState, GatewayHandler,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind},
    HandlerContext, ProcessResult, ProcessorHandler, ResourceProcessor, ResourceRef,
};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::constants::app::GATEWAY_NAME;
use edgion_resources::prelude_resources::{EdgionConfigData, Gateway};
use edgion_resources::resources::common::{ParentReference, SecretObjectReference};
use edgion_resources::resources::edgion_tls::{EdgionTls, EdgionTlsSpec};
use edgion_resources::resources::gateway::{
    AllowedRoutes, FrontendCACertificateRef, FrontendCAResolutionError, FrontendCASourceIdentity, FrontendCASourceKind,
    FrontendMatcherEligibility, FrontendTLSValidation, FrontendValidationMode, GatewayBackendTLS, GatewayFrontendTLS,
    GatewayFrontendTLSDefault, GatewayFrontendTLSPerPort, GatewaySpec, GatewaySpecTLS, GatewayTLSConfig,
    GatewayTlsMode, Listener as GatewayListener, ResolvedFrontendCAReference, ResolvedServingCertificateReference,
    RouteGroupKind, RouteNamespaces, ServingCertificateSourceIdentity,
};
use edgion_resources::resources::proxy_protocol::{
    CanonicalProxyProtocolTrustPolicy, ProxyProtocolConfigErrorCode, ProxyProtocolPolicySourceKind,
    ResolvedInboundProxyProtocol,
};
use edgion_resources::ResourceKind;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use k8s_openapi::ByteString;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

#[tokio::test]
async fn gateway_rejects_invalid_local_bind_ip() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let gateway: Gateway = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "edge", "namespace": "demo", "annotations": {"edgion.io/local-bind-ip": "not-an-ip"}},
        "spec": {"gatewayClassName": "test", "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]}
    }))
    .unwrap();
    let handler = GatewayHandler::new(None, None);
    assert!(handler
        .validate(&gateway, &ctx)
        .iter()
        .any(|error| error.contains("local-bind-ip")));
    let ProcessResult::Continue(gateway) = handler.parse(gateway, &ctx).await else {
        panic!("invalid Gateway must be retained for status reporting");
    };
    assert!(gateway.spec.listeners.is_none());
}

#[tokio::test]
async fn canonical_gateway_unsupported_field_sets_invalid_status_and_withdraws_listeners() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let handler = GatewayHandler::new(None, None);
    let gateway: Gateway = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
        "metadata": {"name": "edge", "namespace": "demo"},
        "spec": {"gatewayClassName": "test",
            "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}],
            "infrastructure": {"labels": {"team": "web"}}}
    }))
    .unwrap();
    let errors = handler.validate(&gateway, &ctx);
    assert_eq!(errors, ["spec.infrastructure is unsupported"]);
    let ProcessResult::Continue(mut gateway) = handler.parse(gateway, &ctx).await else {
        panic!("Gateway parse must retain the object for status reporting");
    };
    assert!(gateway.spec.listeners.is_none());
    handler.update_status(&mut gateway, &ctx, &errors);
    assert!(gateway
        .status
        .as_ref()
        .unwrap()
        .conditions
        .as_ref()
        .unwrap()
        .iter()
        .any(|condition| condition.type_ == "Accepted" && condition.status == "False"));
    handler.finalize_transport(&mut gateway, &ctx);
    assert!(gateway.spec.infrastructure.is_none());

    gateway.spec.listeners = Some(vec![GatewayListener {
        name: "http".into(),
        hostname: None,
        port: 80,
        protocol: "HTTP".into(),
        tls: None,
        allowed_routes: None,
    }]);
    let errors = handler.validate(&gateway, &ctx);
    assert!(errors.is_empty());
    let ProcessResult::Continue(mut corrected) = handler.parse(gateway, &ctx).await else {
        panic!("Corrected Gateway must parse");
    };
    handler.update_status(&mut corrected, &ctx, &errors);
    assert!(corrected.spec.listeners.is_some());
    assert!(corrected
        .status
        .as_ref()
        .unwrap()
        .conditions
        .as_ref()
        .unwrap()
        .iter()
        .any(|condition| condition.type_ == "Accepted" && condition.status == "True"));
}

#[test]
fn invalid_infrastructure_parameters_use_gateway_api_reason() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let handler = GatewayHandler::new(None, None);
    let mut gateway: Gateway = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
            "metadata": {"name": "edge", "namespace": "demo"},
            "spec": {"gatewayClassName": "test",
                "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}],
                "infrastructure": {"parametersRef": {"group": "invalid.io", "kind": "InvalidParameters", "name": "invalid"}}}
        }))
        .unwrap();
    let errors = handler.validate(&gateway, &ctx);
    handler.update_status(&mut gateway, &ctx, &errors);
    let accepted = gateway
        .status
        .unwrap()
        .conditions
        .unwrap()
        .into_iter()
        .find(|condition| condition.type_ == "Accepted")
        .unwrap();
    assert_eq!(
        (accepted.status.as_str(), accepted.reason.as_str()),
        ("False", "InvalidParameters")
    );
}

#[test]
fn unsupported_protocol_only_invalidates_its_listener() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let handler = GatewayHandler::new(None, None);
    for (with_http, expected_gateway_status) in [(false, "False"), (true, "True")] {
        let mut listeners = vec![serde_json::json!({"name": "invalid", "port": 1111, "protocol": "INVALID"})];
        if with_http {
            listeners.push(serde_json::json!({"name": "http", "port": 80, "protocol": "HTTP"}));
        }
        let mut gateway: Gateway = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
            "metadata": {"name": "edge", "namespace": "demo"},
            "spec": {"gatewayClassName": "test", "listeners": listeners}
        }))
        .unwrap();
        let errors = handler.validate(&gateway, &ctx);
        assert!(errors.is_empty());
        handler.update_status(&mut gateway, &ctx, &errors);
        let status = gateway.status.unwrap();
        let accepted = status
            .conditions
            .as_ref()
            .unwrap()
            .iter()
            .find(|condition| condition.type_ == "Accepted")
            .unwrap();
        assert_eq!(
            (accepted.status.as_str(), accepted.reason.as_str()),
            (expected_gateway_status, "ListenersNotValid")
        );
        let programmed = status
            .conditions
            .as_ref()
            .unwrap()
            .iter()
            .find(|condition| condition.type_ == "Programmed")
            .unwrap();
        assert_eq!(programmed.status, expected_gateway_status);
        for listener in status.listeners.unwrap() {
            let accepted = listener
                .conditions
                .iter()
                .find(|condition| condition.type_ == "Accepted")
                .unwrap();
            if listener.name == "invalid" {
                assert_eq!(
                    (accepted.status.as_str(), accepted.reason.as_str()),
                    ("False", "UnsupportedProtocol")
                );
                assert!(listener.supported_kinds.is_empty());
            } else {
                assert_eq!(
                    (accepted.status.as_str(), accepted.reason.as_str()),
                    ("True", "Accepted")
                );
            }
            let programmed = listener
                .conditions
                .iter()
                .find(|condition| condition.type_ == "Programmed")
                .unwrap();
            assert_eq!(
                programmed.status,
                if listener.name == "invalid" { "False" } else { "True" }
            );
        }
    }
}

#[test]
fn unadvertised_tls_terminate_has_no_supported_kinds() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let handler = GatewayHandler::new(None, None);
    let mut gateway: Gateway = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
        "metadata": {"name": "edge", "namespace": "demo"},
        "spec": {
            "gatewayClassName": "test",
            "listeners": [{
                "name": "tls-terminate", "port": 443, "protocol": "TLS",
                "tls": {"mode": "Terminate", "certificateRefs": [{"name": "cert"}]}
            }]
        }
    }))
    .unwrap();
    let errors = handler.validate(&gateway, &ctx);
    handler.update_status(&mut gateway, &ctx, &errors);
    let listener = &gateway.status.unwrap().listeners.unwrap()[0];
    let accepted = listener
        .conditions
        .iter()
        .find(|condition| condition.type_ == "Accepted")
        .unwrap();
    assert_eq!(
        (accepted.status.as_str(), accepted.reason.as_str()),
        ("False", "UnsupportedValue")
    );
    assert!(listener.supported_kinds.is_empty());
}

#[tokio::test]
async fn gateway_address_without_value_is_status_rejected_and_recovers() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let handler = GatewayHandler::new(None, None);
    let source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
        "metadata": {"name": "edge", "namespace": "demo"},
        "spec": {"gatewayClassName": "test", "addresses": [{"type": "IPAddress"}],
            "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]}
    });
    let gateway: Gateway = serde_json::from_value(source.clone()).unwrap();
    let errors = handler.validate(&gateway, &ctx);
    assert_eq!(
        errors,
        ["spec.addresses contains an address without value, which Edgion cannot advertise"]
    );
    let ProcessResult::Continue(mut rejected) = handler.parse(gateway, &ctx).await else {
        panic!("unsupported address must remain available for status");
    };
    assert!(rejected.spec.listeners.is_none());
    assert_eq!(ctx.runtime().listener_port_manager.stats().1, 0);
    handler.update_status(&mut rejected, &ctx, &errors);
    let status = rejected.status.as_ref().unwrap();
    assert!(status
        .conditions
        .as_ref()
        .unwrap()
        .iter()
        .any(|condition| condition.type_ == "Accepted" && condition.status == "False"));
    assert!(status.addresses.as_ref().unwrap().is_empty());

    let mut corrected_source = source;
    corrected_source["spec"]["addresses"][0]["value"] = serde_json::json!("192.0.2.1");
    let corrected: Gateway = serde_json::from_value(corrected_source).unwrap();
    let errors = handler.validate(&corrected, &ctx);
    assert!(errors.is_empty());
    let ProcessResult::Continue(mut corrected) = handler.parse(corrected, &ctx).await else {
        panic!("corrected Gateway must parse");
    };
    assert_eq!(ctx.runtime().listener_port_manager.stats().1, 1);
    handler.update_status(&mut corrected, &ctx, &errors);
    assert!(corrected
        .status
        .as_ref()
        .unwrap()
        .conditions
        .as_ref()
        .unwrap()
        .iter()
        .any(|condition| condition.type_ == "Accepted" && condition.status == "True"));
}

fn make_test_gateway(ns: &str) -> Gateway {
    Gateway {
        metadata: kube::core::ObjectMeta {
            namespace: Some(ns.to_string()),
            ..Default::default()
        },
        spec: GatewaySpec {
            current_status: None,
            gateway_class_name: "test".to_string(),
            listeners: None,
            addresses: None,
            tls: None,
            allowed_listeners: None,
            infrastructure: None,
            default_scope: None,
            resolved_inbound_proxy_protocol: Default::default(),
            resolved_attachment_proof: None,
        },
        status: None,
    }
}

#[test]
fn supported_kinds_intersect_explicit_kinds_with_listener_protocol() {
    let listener = GatewayListener {
        name: "http".to_string(),
        hostname: None,
        port: 80,
        protocol: "HTTP".to_string(),
        tls: None,
        allowed_routes: Some(AllowedRoutes {
            namespaces: None,
            kinds: Some(vec![
                RouteGroupKind {
                    group: None,
                    kind: "HTTPRoute".to_string(),
                },
                RouteGroupKind {
                    group: None,
                    kind: "TLSRoute".to_string(),
                },
            ]),
        }),
    };

    let (supported, errors) = compute_supported_kinds(&listener);
    assert_eq!(supported.len(), 1);
    assert_eq!(supported[0].kind, "HTTPRoute");
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        super::GatewayResolvedRefsError::InvalidRouteKind { kind, protocol, .. }
            if kind == "TLSRoute" && protocol == "HTTP"
    ));
}

fn proxy_protocol_test_context(runtime: Arc<ControllerRuntime>) -> HandlerContext {
    HandlerContext::new(None, None, Default::default(), 5, 20, runtime)
}

#[tokio::test]
async fn conflict_status_messages_converge_across_repeated_reconciliation() {
    use crate::conf_mgr::sync_runtime::resource_processor::status_diff::status_semantically_changed;

    for with_proxy_conflict in [false, true] {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = proxy_protocol_test_context(runtime);
        let handler = GatewayHandler::new(None, None);
        if with_proxy_conflict {
            let peer = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
            assert!(matches!(handler.parse(peer, &ctx).await, ProcessResult::Continue(_)));
        }
        // Distinct protocols keep each (port, protocol, hostname) tuple
        // unique, as required by Gateway API admission. The controller's
        // socket-level conflict detector still reports this combination.
        let mut gateway: Gateway = serde_json::from_value(serde_json::json!({
            "metadata": {"namespace": "default", "name": "conflicts", "generation": 1},
            "spec": {"gatewayClassName": "test", "listeners": [
                {"name": "z-http", "port": 80, "protocol": "HTTP"},
                {"name": "a-tcp", "port": 80, "protocol": "TCP"},
                {"name": "m-tls", "port": 80, "protocol": "TLS", "tls": {"mode": "Passthrough"}}
            ]}
        }))
        .unwrap();
        let mut previous = None::<String>;
        for round in 0..12 {
            gateway.spec.listeners.as_mut().unwrap().rotate_left(1);
            let errors = handler.validate(&gateway, &ctx);
            assert!(errors.is_empty(), "{errors:?}");
            let ProcessResult::Continue(mut computed) = handler.parse(gateway, &ctx).await else {
                panic!("conflicting Gateway must remain observable");
            };
            handler.update_status(&mut computed, &ctx, &errors);
            let status = computed.status.as_ref().unwrap();
            let aggregate = status
                .conditions
                .as_ref()
                .unwrap()
                .iter()
                .find(|condition| condition.type_ == "ListenersNotValid")
                .unwrap();
            assert_eq!(
                aggregate.reason,
                if with_proxy_conflict {
                    "ProxyProtocolPolicyConflict"
                } else {
                    "ListenerConflict"
                }
            );
            let message = &aggregate.message;
            assert!(message.ends_with("[\"a-tcp\", \"m-tls\", \"z-http\"]"), "{message}");
            for listener in status.listeners.as_ref().unwrap() {
                let others = ["a-tcp", "m-tls", "z-http"]
                    .into_iter()
                    .filter(|name| *name != listener.name)
                    .collect::<Vec<_>>()
                    .join(", ");
                let conflict = listener
                    .conditions
                    .iter()
                    .find(|condition| condition.type_ == "Conflicted")
                    .unwrap();
                assert!(
                    conflict.message.contains(&format!("conflicts with: {others}")),
                    "{}: {}",
                    listener.name,
                    conflict.message
                );
            }
            let current = serde_json::to_string(status).unwrap();
            if let Some(previous) = &previous {
                assert!(
                    !status_semantically_changed("Gateway", previous, &current, Some("default")),
                    "round {round}, proxy={with_proxy_conflict}"
                );
            }
            previous = Some(current);
            gateway = computed;
        }
    }
}

fn make_proxy_protocol_gateway(annotation: &str) -> Gateway {
    let mut gateway = make_test_gateway("default");
    gateway.metadata.name = Some("gateway".to_string());
    gateway.metadata.annotations = Some(BTreeMap::from([(
        "edgion.io/inbound-proxy-protocol".to_string(),
        annotation.to_string(),
    )]));
    gateway.spec.listeners = Some(vec![
        GatewayListener {
            name: "http".to_string(),
            hostname: None,
            port: 80,
            protocol: "HTTP".to_string(),
            tls: None,
            allowed_routes: None,
        },
        GatewayListener {
            name: "udp".to_string(),
            hostname: None,
            port: 53,
            protocol: "UDP".to_string(),
            tls: None,
            allowed_routes: None,
        },
    ]);
    gateway
}

#[test]
fn proxy_protocol_omitted_listener_selection_ignores_udp() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = proxy_protocol_test_context(runtime);
    let gateway = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
    let resource_ref = ResourceRef::new(
        ResourceKind::Gateway,
        Some("default".to_string()),
        "gateway".to_string(),
    );

    assert_eq!(
        resolve_inbound_proxy_protocol(&gateway, &ctx, &resource_ref),
        ResolvedInboundProxyProtocol::Ready {
            listener_names: vec!["http".to_string()],
            policy: CanonicalProxyProtocolTrustPolicy::Required {},
            source: ProxyProtocolPolicySourceKind::Inline,
        }
    );
}

#[test]
fn proxy_protocol_explicit_udp_selection_is_invalid_only_for_udp() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = proxy_protocol_test_context(runtime);
    let gateway =
        make_proxy_protocol_gateway(r#"{"source":"inline","listeners":["udp"],"config":{"mode":"required"}}"#);
    let resource_ref = ResourceRef::new(
        ResourceKind::Gateway,
        Some("default".to_string()),
        "gateway".to_string(),
    );

    assert_eq!(
        resolve_inbound_proxy_protocol(&gateway, &ctx, &resource_ref),
        ResolvedInboundProxyProtocol::Invalid {
            code: ProxyProtocolConfigErrorCode::InvalidListenerSelection,
            listener_names: vec!["udp".to_string()],
            reference: None,
        }
    );
}

#[test]
fn proxy_protocol_missing_reference_is_invalid_and_registered() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = proxy_protocol_test_context(runtime.clone());
    let gateway = make_proxy_protocol_gateway(r#"{"source":"resourceRef","resourceRef":{"name":"missing"}}"#);
    let resource_ref = ResourceRef::new(
        ResourceKind::Gateway,
        Some("default".to_string()),
        "gateway".to_string(),
    );

    assert!(matches!(
        resolve_inbound_proxy_protocol(&gateway, &ctx, &resource_ref),
        ResolvedInboundProxyProtocol::Invalid {
            code: ProxyProtocolConfigErrorCode::ReferenceNotFound,
            ..
        }
    ));
    assert_eq!(
        runtime.edgion_config_data_refs.get_refs("default/missing"),
        vec!["Gateway:default/gateway".to_string()]
    );
}

#[test]
fn proxy_protocol_same_namespace_reference_resolves_strict_variant() {
    let runtime = Arc::new(ControllerRuntime::default());
    let target: EdgionConfigData = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionConfigData",
        "metadata": {"namespace": "default", "name": "policy"},
        "spec": {
            "enable": true,
            "visibility": "Namespace",
            "data": {"type": "ProxyProtocolTrust", "config": {"mode": "required"}}
        }
    }))
    .unwrap();
    runtime
        .edgion_config_data_store
        .update(HashMap::from([("default/policy".to_string(), target)]), &HashSet::new());
    let ctx = proxy_protocol_test_context(runtime);
    let gateway = make_proxy_protocol_gateway(r#"{"source":"resourceRef","resourceRef":{"name":"policy"}}"#);
    let resource_ref = ResourceRef::new(
        ResourceKind::Gateway,
        Some("default".to_string()),
        "gateway".to_string(),
    );

    assert!(matches!(
        resolve_inbound_proxy_protocol(&gateway, &ctx, &resource_ref),
        ResolvedInboundProxyProtocol::Ready {
            source: ProxyProtocolPolicySourceKind::ResourceRef,
            ..
        }
    ));
}

#[test]
fn proxy_protocol_reference_rejects_target_with_captured_unknown_field() {
    let runtime = Arc::new(ControllerRuntime::default());
    let mut target: EdgionConfigData = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionConfigData",
        "metadata": {"namespace": "default", "name": "policy"},
        "spec": {
            "data": {
                "type": "ProxyProtocolTrust",
                "config": {"mode": "required", "typo": true}
            }
        }
    }))
    .expect("target typo must remain deserializable");
    target.spec.unknown_fields = vec!["data.config.typo".to_string()];
    runtime
        .edgion_config_data_store
        .update(HashMap::from([("default/policy".to_string(), target)]), &HashSet::new());
    let ctx = proxy_protocol_test_context(runtime);
    let mut gateway = make_proxy_protocol_gateway(r#"{"source":"resourceRef","resourceRef":{"name":"policy"}}"#);
    let resource_ref = ResourceRef::new(
        ResourceKind::Gateway,
        Some("default".to_string()),
        "gateway".to_string(),
    );

    gateway.spec.resolved_inbound_proxy_protocol = resolve_inbound_proxy_protocol(&gateway, &ctx, &resource_ref);
    assert!(matches!(
        gateway.spec.resolved_inbound_proxy_protocol,
        ResolvedInboundProxyProtocol::Invalid {
            code: ProxyProtocolConfigErrorCode::ReferenceInvalidPolicy,
            ..
        }
    ));
    GatewayHandler::new(None, None).update_status(&mut gateway, &ctx, &[]);
    let resolved_refs = gateway
        .status
        .unwrap()
        .conditions
        .unwrap()
        .into_iter()
        .find(|condition| condition.type_ == "ResolvedRefs")
        .unwrap();
    assert_eq!(resolved_refs.status, "False");
    assert_eq!(resolved_refs.reason, "InvalidProxyProtocolRef");
}

#[test]
fn proxy_protocol_udp_status_does_not_pollute_unselected_tcp_listener() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = proxy_protocol_test_context(runtime);
    let mut gateway =
        make_proxy_protocol_gateway(r#"{"source":"inline","listeners":["udp"],"config":{"mode":"required"}}"#);
    gateway.spec.resolved_inbound_proxy_protocol = ResolvedInboundProxyProtocol::Invalid {
        code: ProxyProtocolConfigErrorCode::InvalidListenerSelection,
        listener_names: vec!["udp".to_string()],
        reference: None,
    };

    GatewayHandler::new(None, None).update_status(&mut gateway, &ctx, &[]);
    let statuses = gateway.status.unwrap().listeners.unwrap();
    let http = statuses.iter().find(|status| status.name == "http").unwrap();
    let udp = statuses.iter().find(|status| status.name == "udp").unwrap();
    assert_eq!(
        http.conditions
            .iter()
            .find(|condition| condition.type_ == "Accepted")
            .unwrap()
            .status,
        "True"
    );
    let udp_accepted = udp
        .conditions
        .iter()
        .find(|condition| condition.type_ == "Accepted")
        .unwrap();
    assert_eq!(udp_accepted.status, "False");
    assert_eq!(udp_accepted.reason, "InvalidProxyProtocolListener");
}

#[test]
fn proxy_protocol_reference_error_merges_exact_conditions() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = proxy_protocol_test_context(runtime);
    let mut gateway = make_proxy_protocol_gateway(r#"{"source":"resourceRef","resourceRef":{"name":"missing"}}"#);
    gateway.spec.resolved_inbound_proxy_protocol = ResolvedInboundProxyProtocol::Invalid {
        code: ProxyProtocolConfigErrorCode::ReferenceNotFound,
        listener_names: vec!["http".to_string()],
        reference: Some(
            edgion_resources::resources::proxy_protocol::BoundedProxyProtocolReferenceIdentity::new(
                "default".to_string(),
                "missing".to_string(),
            )
            .unwrap(),
        ),
    };

    GatewayHandler::new(None, None).update_status(&mut gateway, &ctx, &[]);
    let status = gateway.status.unwrap();
    assert_eq!(
        status
            .conditions
            .as_ref()
            .unwrap()
            .iter()
            .filter(|condition| condition.type_ == "ResolvedRefs")
            .count(),
        1
    );
    let gateway_resolved = status
        .conditions
        .as_ref()
        .unwrap()
        .iter()
        .find(|condition| condition.type_ == "ResolvedRefs")
        .unwrap();
    assert_eq!(gateway_resolved.status, "False");
    assert_eq!(gateway_resolved.reason, "InvalidProxyProtocolRef");

    let listeners = status.listeners.unwrap();
    let http = listeners.iter().find(|listener| listener.name == "http").unwrap();
    let udp = listeners.iter().find(|listener| listener.name == "udp").unwrap();
    assert_eq!(
        http.conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap()
            .reason,
        "InvalidProxyProtocolRef"
    );
    assert_eq!(
        udp.conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap()
            .status,
        "True"
    );
}

#[tokio::test]
async fn proxy_protocol_shared_listener_conflict_syncs_invalid_and_status() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = proxy_protocol_test_context(runtime);
    let handler = GatewayHandler::new(None, None);
    let first = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
    let ProcessResult::Continue(_first) = handler.parse(first, &ctx).await else {
        panic!("first Gateway should continue");
    };

    let mut second = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
    second.metadata.name = Some("second".to_string());
    second.metadata.annotations = None;
    let ProcessResult::Continue(mut second) = handler.parse(second, &ctx).await else {
        panic!("second Gateway should continue");
    };
    assert!(matches!(
        second.spec.resolved_inbound_proxy_protocol,
        ResolvedInboundProxyProtocol::Invalid {
            code: ProxyProtocolConfigErrorCode::SharedListenerConflict,
            ..
        }
    ));

    handler.update_status(&mut second, &ctx, &[]);
    let status = second.status.unwrap();
    let conditions = status.conditions.unwrap();
    assert_eq!(
        conditions
            .iter()
            .find(|condition| condition.type_ == "ListenersNotValid")
            .unwrap()
            .reason,
        "ProxyProtocolPolicyConflict"
    );
    assert_eq!(
        conditions
            .iter()
            .find(|condition| condition.type_ == "Accepted")
            .unwrap()
            .status,
        "True"
    );
    let http = status
        .listeners
        .unwrap()
        .into_iter()
        .find(|listener| listener.name == "http")
        .unwrap();
    let conflicted = http
        .conditions
        .iter()
        .find(|condition| condition.type_ == "Conflicted")
        .unwrap();
    assert_eq!(conflicted.status, "True");
    assert_eq!(conflicted.reason, "ProxyProtocolPolicyConflict");
}

#[tokio::test]
async fn proxy_protocol_primary_invalid_survives_shared_conflict() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = proxy_protocol_test_context(runtime);
    let handler = GatewayHandler::new(None, None);
    let first = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
    let ProcessResult::Continue(_first) = handler.parse(first, &ctx).await else {
        panic!("first Gateway should continue");
    };

    let mut second =
        make_proxy_protocol_gateway(r#"{"source":"inline","listeners":["udp"],"config":{"mode":"required"}}"#);
    second.metadata.name = Some("second".to_string());
    let ProcessResult::Continue(mut second) = handler.parse(second, &ctx).await else {
        panic!("second Gateway should continue");
    };
    assert!(matches!(
        second.spec.resolved_inbound_proxy_protocol,
        ResolvedInboundProxyProtocol::Invalid {
            code: ProxyProtocolConfigErrorCode::InvalidListenerSelection,
            ..
        }
    ));

    handler.update_status(&mut second, &ctx, &[]);
    let status = second.status.unwrap();
    let conditions = status.conditions.unwrap();
    assert_eq!(
        conditions
            .iter()
            .find(|condition| condition.type_ == "Accepted")
            .unwrap()
            .reason,
        "InvalidProxyProtocolConfig"
    );
    assert_eq!(
        conditions
            .iter()
            .find(|condition| condition.type_ == "ListenersNotValid")
            .unwrap()
            .reason,
        "ProxyProtocolPolicyConflict"
    );
    let listeners = status.listeners.unwrap();
    assert_eq!(
        listeners
            .iter()
            .find(|listener| listener.name == "udp")
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == "Accepted")
            .unwrap()
            .reason,
        "InvalidProxyProtocolListener"
    );
    assert_eq!(
        listeners
            .iter()
            .find(|listener| listener.name == "http")
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == "Conflicted")
            .unwrap()
            .reason,
        "ProxyProtocolPolicyConflict"
    );
}

#[tokio::test]
async fn proxy_protocol_conflict_precedes_mixed_tls_and_port_conflict() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = proxy_protocol_test_context(runtime);
    let handler = GatewayHandler::new(None, None);
    let tls = |name: &str, mode: GatewayTlsMode| GatewayListener {
        name: name.to_string(),
        hostname: None,
        port: 443,
        protocol: "TLS".to_string(),
        tls: Some(GatewayTLSConfig {
            mode: Some(mode),
            certificate_refs: None,
            options: None,
            resolved_certificate_refs: None,
            frontend_validation: None,
            resolved_frontend_ca_refs: None,
            frontend_matcher_eligibility: None,
        }),
        allowed_routes: None,
    };

    let mut first = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
    first.spec.listeners = Some(vec![tls("tls", GatewayTlsMode::Passthrough)]);
    let ProcessResult::Continue(_first) = handler.parse(first, &ctx).await else {
        panic!("first Gateway should continue");
    };

    let mut second = make_proxy_protocol_gateway(r#"{"source":"inline","config":{"mode":"required"}}"#);
    second.metadata.name = Some("second".to_string());
    second.metadata.annotations = None;
    second.spec.listeners = Some(vec![
        tls("terminate", GatewayTlsMode::Terminate),
        tls("passthrough", GatewayTlsMode::Passthrough),
    ]);
    let ProcessResult::Continue(mut second) = handler.parse(second, &ctx).await else {
        panic!("second Gateway should continue");
    };
    handler.update_status(&mut second, &ctx, &[]);

    let status = second.status.unwrap();
    let gateway_conflict = status
        .conditions
        .as_ref()
        .unwrap()
        .iter()
        .find(|condition| condition.type_ == "ListenersNotValid")
        .unwrap();
    assert_eq!(gateway_conflict.reason, "ProxyProtocolPolicyConflict");
    assert!(gateway_conflict.message.contains("port conflicts"));
    for listener in status.listeners.unwrap() {
        let conflicted = listener
            .conditions
            .iter()
            .find(|condition| condition.type_ == "Conflicted")
            .unwrap();
        assert_eq!(conflicted.reason, "ProxyProtocolPolicyConflict");
        assert!(conflicted.message.contains("mixed TLS"));
        assert!(conflicted.message.contains("port conflict"));
        assert_eq!(
            listener
                .conditions
                .iter()
                .find(|condition| condition.type_ == "Accepted")
                .unwrap()
                .reason,
            "ProtocolConflict"
        );
    }
}

fn make_gateway_with_annotation(ns: &str, annotation_value: &str) -> Gateway {
    let mut gw = make_test_gateway(ns);
    let mut annotations = BTreeMap::new();
    annotations.insert(
        "edgion.io/edgion-stream-plugins".to_string(),
        annotation_value.to_string(),
    );
    gw.metadata.annotations = Some(annotations);
    gw
}

#[test]
fn validate_stream_plugin_ref_no_annotation_returns_none() {
    let runtime = ControllerRuntime::default();
    let gw = make_test_gateway("default");
    assert!(validate_gateway_stream_plugin_ref(&gw, &runtime).is_none());
}

#[test]
fn validate_stream_plugin_ref_empty_annotation_returns_none() {
    let runtime = ControllerRuntime::default();
    let gw = make_gateway_with_annotation("default", "  ");
    assert!(validate_gateway_stream_plugin_ref(&gw, &runtime).is_none());
}

#[test]
fn validate_stream_plugin_ref_missing_resource_returns_error() {
    let runtime = ControllerRuntime::default();
    let gw = make_gateway_with_annotation("default", "my-stream");
    let result = validate_gateway_stream_plugin_ref(&gw, &runtime);
    assert!(result.is_some());
    assert!(result.unwrap().contains("default/my-stream"));
}

#[test]
fn validate_stream_plugin_ref_fallback_to_gateway_ns() {
    let runtime = ControllerRuntime::default();
    let gw = make_gateway_with_annotation("my-ns", "my-stream");
    let result = validate_gateway_stream_plugin_ref(&gw, &runtime);
    assert!(result.is_some());
    assert!(result.unwrap().contains("my-ns/my-stream"));
}

fn make_gateway_with_dns_resolver_ref(ns: &str, reference: &str) -> Gateway {
    let mut gw = make_test_gateway(ns);
    gw.metadata.name = Some("dns-gateway".into());
    let mut annotations = BTreeMap::new();
    annotations.insert("edgion.io/dns-resolver-ref".to_string(), reference.to_string());
    gw.metadata.annotations = Some(annotations);
    gw
}

#[test]
fn validate_dns_resolver_ref_no_annotation_returns_none() {
    let runtime = ControllerRuntime::default();
    let gw = make_test_gateway("default");
    assert!(validate_gateway_dns_resolver_ref(&gw, &runtime).is_none());
}

#[test]
fn validate_dns_resolver_ref_same_namespace_is_allowed() {
    // A same-namespace ref is never grant-gated.
    let runtime = ControllerRuntime::default();
    let gw = make_gateway_with_dns_resolver_ref("app", "app/aliyun");
    assert!(validate_gateway_dns_resolver_ref(&gw, &runtime).is_none());
}

#[test]
fn validate_dns_resolver_ref_cross_namespace_without_grant_is_denied() {
    // Empty ref_grant_store + default (enabled) validation → cross-ns denied.
    let runtime = ControllerRuntime::default();
    let gw = make_gateway_with_dns_resolver_ref("app", "dns/aliyun");
    let result = validate_gateway_dns_resolver_ref(&gw, &runtime);
    assert!(
        result.is_some(),
        "cross-namespace ref without a ReferenceGrant must be denied"
    );
    let msg = result.unwrap();
    assert!(msg.contains("dns/aliyun"), "message must name the ref, got: {msg}");
    assert!(
        msg.contains("not permitted"),
        "message must state the denial, got: {msg}"
    );
}

#[test]
fn resolve_gateway_stream_plugin_ref_rejects_qualified_name() {
    let error = resolve_gateway_stream_plugin_ref("other-ns/my-plugin", "default").unwrap_err();
    assert!(error.contains("unqualified resource name"));
}

#[test]
fn resolve_gateway_stream_plugin_ref_uses_gateway_ns() {
    assert_eq!(
        resolve_gateway_stream_plugin_ref("my-plugin", "default").unwrap(),
        "default/my-plugin"
    );
}

#[test]
fn validate_stream_plugin_ref_rejects_qualified_name() {
    let runtime = ControllerRuntime::default();
    let gw = make_gateway_with_annotation("default", "other-ns/my-plugin");
    let error = validate_gateway_stream_plugin_ref(&gw, &runtime).unwrap();
    assert!(error.contains("unqualified resource name"));
}

fn listener_with_ca_refs(refs: Vec<(&str, &str)>) -> GatewayListener {
    let ca_refs = refs
        .into_iter()
        .map(|(ns, name)| FrontendCACertificateRef {
            group: String::new(),
            kind: "Secret".to_string(),
            name: name.to_string(),
            namespace: Some(ns.to_string()),
        })
        .collect();
    GatewayListener {
        name: "https".to_string(),
        hostname: None,
        port: 443,
        protocol: "HTTPS".to_string(),
        tls: Some(GatewayTLSConfig {
            mode: Some(GatewayTlsMode::Terminate),
            certificate_refs: None,
            options: None,
            resolved_certificate_refs: None,
            frontend_validation: Some(FrontendTLSValidation {
                ca_certificate_refs: ca_refs,
                mode: None,
            }),
            resolved_frontend_ca_refs: None,
            frontend_matcher_eligibility: None,
        }),
        allowed_routes: None,
    }
}

#[test]
fn gateway_service_lookup_prefers_shared_service_over_unrelated_same_named_service() {
    let candidates = gateway_service_lookup_candidates("gateway-conformance-infra", "same-namespace");

    assert_eq!(
        candidates,
        vec![
            ("edgion-system".to_string(), GATEWAY_NAME.to_string()),
            ("gateway-conformance-infra".to_string(), "same-namespace".to_string()),
        ]
    );
}

#[test]
fn gateway_service_lookup_avoids_duplicate_fixed_service_candidate() {
    let candidates = gateway_service_lookup_candidates("edgion-system", GATEWAY_NAME);

    assert_eq!(
        candidates,
        vec![("edgion-system".to_string(), GATEWAY_NAME.to_string())]
    );
}

#[test]
fn gateway_service_address_uses_assigned_load_balancer_ingress() {
    let service: k8s_openapi::api::core::v1::Service = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "edge", "namespace": "demo"},
        "spec": {"type": "LoadBalancer", "clusterIP": "10.96.0.20"},
        "status": {"loadBalancer": {"ingress": [{"hostname": "edge.example.com"}]}}
    }))
    .unwrap();
    let addresses = assigned_service_addresses(&service);
    let address = &addresses[0];
    assert_eq!(address.address_type.as_deref(), Some("Hostname"));
    assert_eq!(address.value, "edge.example.com");

    let mut pending = service;
    pending.status = None;
    assert!(assigned_service_addresses(&pending).is_empty());
}

#[test]
fn dedicated_gateway_accepts_all_assigned_load_balancer_ingresses() {
    let service: k8s_openapi::api::core::v1::Service = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "edge", "namespace": "demo"},
        "spec": {"type": "LoadBalancer", "clusterIP": "10.96.0.20"},
        "status": {"loadBalancer": {"ingress": [
            {"ip": "192.0.2.10"}, {"ip": "192.0.2.11"},
            {"ip": "2001:0db8::1"}, {"hostname": "edge.example.com"}
        ]}}
    }))
    .unwrap();
    let assigned = assigned_service_addresses(&service);
    assert_eq!(assigned.len(), 4);

    let gateway: Gateway = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "edge", "namespace": "demo"},
        "spec": {"gatewayClassName": "test", "addresses": [
            {"type": "IPAddress", "value": "192.0.2.11"},
            {"type": "IPAddress", "value": "2001:db8::1"},
            {"type": "Hostname", "value": "edge.example.com"}
        ], "listeners": []}
    }))
    .unwrap();
    assert!(requested_addresses_match_assigned(&gateway, &assigned));
    assert!(matches!(
        classify_dedicated_addresses(&gateway, assigned),
        DedicatedAddressState::Assigned(addresses) if addresses.len() == 4
    ));
    assert!(matches!(
        classify_dedicated_addresses(&gateway, Vec::new()),
        DedicatedAddressState::Pending
    ));
}

#[test]
fn dedicated_gateway_service_name_matches_provisioner() {
    assert_eq!(
        dedicated_gateway_service_name(
            "gateway-conformance-infra/same-namespace",
            "e78e966c-29ec-4827-8a1d-a4c3b07d1169"
        ),
        "edgion-gw-ffb7d8f091282df4"
    );
}

#[test]
fn dedicated_gateway_without_service_does_not_advertise_shared_address() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, Arc::clone(&runtime));
    let handler = GatewayHandler::new(None, None);
    let mut gateway: Gateway = serde_json::from_value(serde_json::json!({
        "metadata": {
            "name": "edge", "namespace": "demo", "uid": "uid-1",
            "annotations": {"edgion.io/dedicated-gateway-instance": "true"}
        },
        "spec": {
            "gatewayClassName": "test",
            "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]
        }
    }))
    .unwrap();
    assert!(derive_gateway_addresses(&gateway, None, &runtime).is_empty());
    handler.update_status(&mut gateway, &ctx, &[]);
    let status = gateway.status.unwrap();
    assert!(status.addresses.unwrap().is_empty());
    assert!(status.conditions.unwrap().iter().any(|condition| {
        condition.type_ == "Programmed" && condition.status == "False" && condition.reason == "AddressNotAssigned"
    }));
}

#[test]
fn dedicated_gateway_requested_address_must_match_service_assignment() {
    let mut gateway: Gateway = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "edge", "namespace": "demo"},
        "spec": {"gatewayClassName": "test", "addresses": [{"type": "IPAddress", "value": "192.0.2.10"}], "listeners": []}
    })).unwrap();
    let assigned = edgion_resources::resources::gateway::GatewayStatusAddress {
        address_type: Some("IPAddress".into()),
        value: "192.0.2.11".into(),
    };
    assert!(!requested_addresses_match_assigned(
        &gateway,
        std::slice::from_ref(&assigned)
    ));
    assert!(matches!(
        classify_dedicated_addresses(&gateway, vec![assigned.clone()]),
        DedicatedAddressState::Unusable(addresses) if addresses.len() == 1
    ));
    gateway.spec.addresses = None;
    assert!(requested_addresses_match_assigned(&gateway, &[assigned]));
}

#[test]
fn has_all_ca_refs_invalid_returns_false_when_no_refs_configured() {
    let listener = listener_with_ca_refs(vec![]);
    assert!(!has_all_ca_refs_invalid(&listener, "gw-ns"));
}

#[test]
fn has_all_ca_refs_invalid_uses_each_declared_index_including_duplicates() {
    let mut listener = listener_with_ca_refs(vec![("ns-a", "same"), ("ns-a", "same")]);
    listener.tls.as_mut().unwrap().resolved_frontend_ca_refs = Some(vec![
        ResolvedFrontendCAReference {
            index: 0,
            source: FrontendCASourceIdentity {
                kind: FrontendCASourceKind::Secret,
                namespace: "ns-a".to_string(),
                name: "same".to_string(),
            },
            ca_certificate: None,
            error: Some(FrontendCAResolutionError::NotFound),
        },
        ResolvedFrontendCAReference {
            index: 1,
            source: FrontendCASourceIdentity {
                kind: FrontendCASourceKind::Secret,
                namespace: "ns-a".to_string(),
                name: "same".to_string(),
            },
            ca_certificate: None,
            error: Some(FrontendCAResolutionError::NotFound),
        },
    ]);
    assert!(has_all_ca_refs_invalid(&listener, "gw-ns"));
}

#[test]
fn has_all_ca_refs_invalid_is_false_when_one_index_is_valid() {
    let mut listener = listener_with_ca_refs(vec![("ns-a", "ref-a"), ("ns-b", "ref-b")]);
    listener.tls.as_mut().unwrap().resolved_frontend_ca_refs = Some(vec![
        ResolvedFrontendCAReference {
            index: 0,
            source: FrontendCASourceIdentity {
                kind: FrontendCASourceKind::Secret,
                namespace: "ns-a".to_string(),
                name: "ref-a".to_string(),
            },
            ca_certificate: None,
            error: Some(FrontendCAResolutionError::NotFound),
        },
        ResolvedFrontendCAReference {
            index: 1,
            source: FrontendCASourceIdentity {
                kind: FrontendCASourceKind::Secret,
                namespace: "ns-b".to_string(),
                name: "ref-b".to_string(),
            },
            ca_certificate: Some(ByteString(b"certificate".to_vec())),
            error: None,
        },
    ]);
    assert!(!has_all_ca_refs_invalid(&listener, "gw-ns"));
}

#[test]
fn frontend_ca_status_distinguishes_partial_from_fully_invalid_pem_sets() {
    let runtime = ControllerRuntime::default();
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["client.example.com".to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let valid_pem = params.self_signed(&key).unwrap().pem();
    let make_secret = |name: &str, pem: &[u8]| Secret {
        metadata: kube::api::ObjectMeta {
            namespace: Some("gw-ns".to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([("ca.crt".to_string(), ByteString(pem.to_vec()))])),
        ..Default::default()
    };
    runtime.secret_store.update(
        HashMap::from([
            ("gw-ns/valid".to_string(), make_secret("valid", valid_pem.as_bytes())),
            (
                "gw-ns/garbage".to_string(),
                make_secret("garbage", b"not a certificate"),
            ),
        ]),
        &HashSet::new(),
    );
    let mut gateway = make_test_gateway("gw-ns");
    gateway.metadata.name = Some("gateway".to_string());

    let mut partial = listener_with_ca_refs(vec![("gw-ns", "valid"), ("gw-ns", "garbage")]);
    partial.tls.as_mut().unwrap().resolved_frontend_ca_refs = Some(vec![
        ResolvedFrontendCAReference {
            index: 0,
            source: FrontendCASourceIdentity {
                kind: FrontendCASourceKind::Secret,
                namespace: "gw-ns".to_string(),
                name: "valid".to_string(),
            },
            ca_certificate: Some(ByteString(valid_pem.as_bytes().to_vec())),
            error: None,
        },
        ResolvedFrontendCAReference {
            index: 1,
            source: FrontendCASourceIdentity {
                kind: FrontendCASourceKind::Secret,
                namespace: "gw-ns".to_string(),
                name: "garbage".to_string(),
            },
            ca_certificate: None,
            error: Some(FrontendCAResolutionError::InvalidCertificate),
        },
    ]);
    let partial_errors = validate_listener_resolved_refs(&gateway, &partial, &runtime);
    assert!(!partial_errors.is_empty());
    assert!(!has_all_ca_refs_invalid(&partial, "gw-ns"));

    let mut full = listener_with_ca_refs(vec![("gw-ns", "garbage")]);
    full.tls.as_mut().unwrap().resolved_frontend_ca_refs = Some(vec![ResolvedFrontendCAReference {
        index: 0,
        source: FrontendCASourceIdentity {
            kind: FrontendCASourceKind::Secret,
            namespace: "gw-ns".to_string(),
            name: "garbage".to_string(),
        },
        ca_certificate: None,
        error: Some(FrontendCAResolutionError::InvalidCertificate),
    }]);
    let _full_errors = validate_listener_resolved_refs(&gateway, &full, &runtime);
    assert!(has_all_ca_refs_invalid(&full, "gw-ns"));
}

#[test]
fn insecure_frontend_missing_ca_does_not_create_status_error() {
    let runtime = ControllerRuntime::default();
    let gateway = make_test_gateway("gw-ns");
    let mut listener = listener_with_ca_refs(vec![("gw-ns", "missing")]);
    let tls = listener.tls.as_mut().unwrap();
    tls.frontend_validation.as_mut().unwrap().mode = Some(FrontendValidationMode::AllowInsecureFallback);
    tls.resolved_frontend_ca_refs = Some(vec![ResolvedFrontendCAReference {
        index: 0,
        source: FrontendCASourceIdentity {
            kind: FrontendCASourceKind::Secret,
            namespace: "gw-ns".to_string(),
            name: "missing".to_string(),
        },
        ca_certificate: None,
        error: Some(FrontendCAResolutionError::NotFound),
    }]);
    assert!(validate_listener_resolved_refs(&gateway, &listener, &runtime).is_empty());
    assert!(!has_all_ca_refs_invalid(&listener, "gw-ns"));
}

#[test]
fn frontend_without_serving_refs_requires_exact_edgion_tls_opt_in() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let handler = GatewayHandler::new(None, None);

    for (provider, expected) in [
        (None, FrontendMatcherEligibility::StrictReject),
        (Some("edgiontls"), FrontendMatcherEligibility::StrictReject),
        (
            Some("EdgionTls"),
            FrontendMatcherEligibility::RequireExactEdgionTlsCertificate,
        ),
    ] {
        let mut gateway = make_test_gateway("gw-ns");
        gateway.metadata.name = Some("gateway".to_string());
        let mut listener = listener_with_ca_refs(vec![("gw-ns", "missing")]);
        let tls = listener.tls.as_mut().unwrap();
        tls.frontend_validation.as_mut().unwrap().mode = Some(FrontendValidationMode::AllowInsecureFallback);
        tls.certificate_refs = None;
        tls.options = provider.map(|value| serde_json::json!({"edgion.io/cert-provider": value}));
        tls.resolved_frontend_ca_refs = Some(vec![ResolvedFrontendCAReference {
            index: 0,
            source: FrontendCASourceIdentity {
                kind: FrontendCASourceKind::Secret,
                namespace: "gw-ns".to_string(),
                name: "missing".to_string(),
            },
            ca_certificate: None,
            error: Some(FrontendCAResolutionError::NotFound),
        }]);
        gateway.spec.listeners = Some(vec![listener]);

        handler.update_status(&mut gateway, &ctx, &[]);
        assert_eq!(
            gateway.spec.listeners.as_ref().unwrap()[0]
                .tls
                .as_ref()
                .unwrap()
                .frontend_matcher_eligibility,
            Some(expected),
            "provider={provider:?}"
        );
    }
}

#[tokio::test]
async fn real_serving_certificate_and_private_key_are_resolved_and_validated() {
    let runtime = Arc::new(ControllerRuntime::default());
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec!["server.example.com".to_string()]).unwrap();
    let cert = params.self_signed(&key).unwrap().pem();
    let secret = Secret {
        metadata: kube::api::ObjectMeta {
            namespace: Some("gw-ns".to_string()),
            name: Some("server".to_string()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([
            ("tls.crt".to_string(), ByteString(cert.into_bytes())),
            ("tls.key".to_string(), ByteString(key.serialize_pem().into_bytes())),
        ])),
        ..Default::default()
    };
    runtime
        .secret_store
        .update(HashMap::from([("gw-ns/server".to_string(), secret)]), &HashSet::new());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
    let mut gateway = make_test_gateway("gw-ns");
    gateway.metadata.name = Some("gateway".to_string());
    gateway.spec.listeners = Some(vec![GatewayListener {
        name: "https".to_string(),
        hostname: Some("server.example.com".to_string()),
        port: 443,
        protocol: "HTTPS".to_string(),
        tls: Some(GatewayTLSConfig {
            mode: Some(GatewayTlsMode::Terminate),
            certificate_refs: Some(vec![SecretObjectReference {
                group: None,
                kind: None,
                namespace: None,
                name: "server".to_string(),
            }]),
            options: None,
            resolved_certificate_refs: None,
            frontend_validation: None,
            resolved_frontend_ca_refs: None,
            frontend_matcher_eligibility: None,
        }),
        allowed_routes: None,
    }]);

    let handler = GatewayHandler::new(None, None);
    let parsed = match handler.parse(gateway, &ctx).await {
        ProcessResult::Continue(gateway) => gateway,
        ProcessResult::Skip { reason } => panic!("unexpected Skip: {reason}"),
    };
    let listener = &parsed.spec.listeners.as_ref().unwrap()[0];
    assert!(validate_listener_resolved_refs(&parsed, listener, &runtime).is_empty());
    let outcome = &listener
        .tls
        .as_ref()
        .unwrap()
        .resolved_certificate_refs
        .as_ref()
        .unwrap()[0];
    let payload = outcome.secret.as_ref().expect("real server Secret must resolve");
    assert_eq!(payload.metadata.name.as_deref(), Some("server"));
    assert_eq!(payload.metadata.namespace.as_deref(), Some("gw-ns"));
}

/// Verify that processing a Gateway with `edgion.io/edgion-plugins` annotation registers
/// both the explicit-namespace ref ("ns1/p1") and the bare-name ref (expanded to the
/// Gateway's namespace) into `plugin_ref_mgr`.
#[tokio::test]
async fn gateway_plugin_annotation_registers_refs() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());

    // Build a Gateway in "gw-ns" with two plugin refs: one fully-qualified, one bare.
    let mut gw = make_test_gateway("gw-ns");
    gw.metadata.name = Some("my-gateway".to_string());
    let mut annotations = BTreeMap::new();
    annotations.insert("edgion.io/edgion-plugins".to_string(), "ns1/p1, p2".to_string());
    gw.metadata.annotations = Some(annotations);

    let handler = GatewayHandler::new(None, None);
    handler.parse(gw, &ctx).await;

    // "ns1/p1" ref must be registered under the explicit namespace.
    let refs_p1 = runtime.plugin_ref_mgr.get_refs("ns1/p1");
    assert_eq!(refs_p1.len(), 1, "expected one Gateway ref for ns1/p1");
    assert_eq!(refs_p1[0].resource_key(), "gw-ns/my-gateway");

    // "p2" (bare name) must be expanded with the Gateway's namespace "gw-ns".
    let refs_p2 = runtime.plugin_ref_mgr.get_refs("gw-ns/p2");
    assert_eq!(refs_p2.len(), 1, "expected one Gateway ref for gw-ns/p2");
    assert_eq!(refs_p2[0].resource_key(), "gw-ns/my-gateway");
}

#[tokio::test]
async fn gateway_backend_client_certificate_is_resolved_and_registered() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());

    let mut secret = Secret::default();
    secret.metadata.namespace = Some("gw-ns".to_string());
    secret.metadata.name = Some("client-cert".to_string());
    secret.data = Some(BTreeMap::from([
        ("tls.crt".to_string(), ByteString(b"certificate".to_vec())),
        ("tls.key".to_string(), ByteString(b"private-key".to_vec())),
    ]));
    runtime.secret_store.update(
        HashMap::from([("gw-ns/client-cert".to_string(), secret)]),
        &HashSet::new(),
    );

    let mut gw = make_test_gateway("gw-ns");
    gw.metadata.name = Some("my-gateway".to_string());
    gw.spec.tls = Some(GatewaySpecTLS {
        backend: Some(GatewayBackendTLS {
            client_certificate_ref: Some(SecretObjectReference {
                group: None,
                kind: None,
                name: "client-cert".to_string(),
                namespace: None,
            }),
            resolved_client_certificate: None,
        }),
        frontend: None,
    });

    let handler = GatewayHandler::new(None, None);
    let ProcessResult::Continue(parsed) = handler.parse(gw, &ctx).await else {
        panic!("Gateway parse unexpectedly skipped the resource");
    };

    assert!(
        parsed
            .spec
            .tls
            .and_then(|tls| tls.backend)
            .and_then(|backend| backend.resolved_client_certificate)
            .is_some(),
        "Gateway backend client certificate must be embedded for conf_sync"
    );
    let refs = runtime.secret_ref_manager.get_refs("gw-ns/client-cert");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].resource_key(), "gw-ns/my-gateway");
}

fn frontend_validation(kind: &str, name: &str, mode: Option<FrontendValidationMode>) -> FrontendTLSValidation {
    FrontendTLSValidation {
        ca_certificate_refs: vec![FrontendCACertificateRef {
            group: String::new(),
            kind: kind.to_string(),
            name: name.to_string(),
            namespace: None,
        }],
        mode,
    }
}

#[tokio::test]
async fn standard_frontend_projection_is_https_only_authoritative_and_resolves_all_kinds() {
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.secret_store.update(
        HashMap::from([(
            "gw-ns/default-ca".to_string(),
            Secret {
                metadata: kube::api::ObjectMeta {
                    namespace: Some("gw-ns".to_string()),
                    name: Some("default-ca".to_string()),
                    ..Default::default()
                },
                data: Some(BTreeMap::from([(
                    "ca.crt".to_string(),
                    ByteString(b"secret-ca".to_vec()),
                )])),
                ..Default::default()
            },
        )]),
        &HashSet::new(),
    );
    runtime.configmap_store.update(
        HashMap::from([(
            "gw-ns/port-ca".to_string(),
            ConfigMap {
                metadata: kube::api::ObjectMeta {
                    namespace: Some("gw-ns".to_string()),
                    name: Some("port-ca".to_string()),
                    ..Default::default()
                },
                data: Some(BTreeMap::from([("ca.crt".to_string(), "configmap-ca".to_string())])),
                ..Default::default()
            },
        )]),
        &HashSet::new(),
    );
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
    let raw = frontend_validation("Secret", "raw-ca", None);
    let mut gateway = make_test_gateway("gw-ns");
    gateway.metadata.name = Some("gateway".to_string());
    gateway.spec.tls = Some(GatewaySpecTLS {
        backend: None,
        frontend: Some(GatewayFrontendTLS {
            default: GatewayFrontendTLSDefault {
                validation: Some(frontend_validation("Secret", "default-ca", None)),
            },
            per_port: Some(vec![GatewayFrontendTLSPerPort {
                port: 8443,
                tls: GatewayFrontendTLSDefault {
                    validation: Some(frontend_validation(
                        "ConfigMap",
                        "port-ca",
                        Some(FrontendValidationMode::AllowInsecureFallback),
                    )),
                },
            }]),
        }),
    });
    gateway.spec.listeners = Some(vec![
        GatewayListener {
            name: "https-default".to_string(),
            hostname: None,
            port: 443,
            protocol: "HTTPS".to_string(),
            tls: Some(GatewayTLSConfig {
                mode: Some(GatewayTlsMode::Terminate),
                certificate_refs: None,
                options: None,
                resolved_certificate_refs: None,
                frontend_validation: Some(raw.clone()),
                resolved_frontend_ca_refs: None,
                frontend_matcher_eligibility: None,
            }),
            allowed_routes: None,
        },
        GatewayListener {
            name: "https-port".to_string(),
            hostname: None,
            port: 8443,
            protocol: "HTTPS".to_string(),
            tls: None,
            allowed_routes: None,
        },
        GatewayListener {
            name: "tls-ignored".to_string(),
            hostname: None,
            port: 9443,
            protocol: "TLS".to_string(),
            tls: Some(GatewayTLSConfig {
                mode: Some(GatewayTlsMode::Terminate),
                certificate_refs: None,
                options: None,
                resolved_certificate_refs: None,
                frontend_validation: Some(raw),
                resolved_frontend_ca_refs: None,
                frontend_matcher_eligibility: None,
            }),
            allowed_routes: None,
        },
    ]);

    let ProcessResult::Continue(parsed) = GatewayHandler::new(None, None).parse(gateway, &ctx).await else {
        panic!("parse skipped");
    };
    let listeners = parsed.spec.listeners.unwrap();
    let default_tls = listeners[0].tls.as_ref().unwrap();
    assert_eq!(
        default_tls.frontend_validation.as_ref().unwrap().ca_certificate_refs[0].name,
        "default-ca"
    );
    assert_eq!(default_tls.resolved_frontend_ca_refs.as_ref().unwrap().len(), 1);
    let port_tls = listeners[1].tls.as_ref().unwrap();
    assert_eq!(
        port_tls.frontend_validation.as_ref().unwrap().ca_certificate_refs[0].name,
        "port-ca"
    );
    assert_eq!(port_tls.resolved_frontend_ca_refs.as_ref().unwrap().len(), 1);
    assert!(listeners[2].tls.as_ref().unwrap().frontend_validation.is_none());
    assert!(listeners[2].tls.as_ref().unwrap().resolved_frontend_ca_refs.is_none());
    assert_eq!(
        runtime
            .frontend_ca_ref_manager
            .get_refs(&FrontendCaSourceKey::new(FrontendCaSourceKind::Secret, "gw-ns", "default-ca").index_key())
            .len(),
        1
    );
    assert_eq!(
        runtime
            .frontend_ca_ref_manager
            .get_refs(&FrontendCaSourceKey::new(FrontendCaSourceKind::ConfigMap, "gw-ns", "port-ca").index_key())
            .len(),
        1
    );
}

#[tokio::test]
async fn frontend_projects_sixteen_ca_references_in_fallback_mode() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let mut gateway = make_test_gateway("gw-ns");
    let mut validation = frontend_validation("ConfigMap", "ca-0", Some(FrontendValidationMode::AllowInsecureFallback));
    validation.ca_certificate_refs = (0..16)
        .map(|index| FrontendCACertificateRef {
            group: String::new(),
            kind: "ConfigMap".to_string(),
            name: format!("ca-{index}"),
            namespace: None,
        })
        .collect();
    gateway.spec.tls = Some(GatewaySpecTLS {
        backend: None,
        frontend: Some(GatewayFrontendTLS {
            default: GatewayFrontendTLSDefault {
                validation: Some(validation),
            },
            per_port: None,
        }),
    });
    gateway.spec.listeners = Some(vec![GatewayListener {
        name: "https".to_string(),
        hostname: None,
        port: 443,
        protocol: "HTTPS".to_string(),
        tls: Some(GatewayTLSConfig {
            mode: Some(GatewayTlsMode::Terminate),
            certificate_refs: None,
            options: None,
            resolved_certificate_refs: None,
            frontend_validation: None,
            resolved_frontend_ca_refs: None,
            frontend_matcher_eligibility: None,
        }),
        allowed_routes: None,
    }]);

    let ProcessResult::Continue(parsed) = GatewayHandler::new(None, None).parse(gateway, &ctx).await else {
        panic!("gateway parse unexpectedly stopped");
    };
    let listeners = parsed.spec.listeners.as_ref().unwrap();
    let tls = listeners[0].tls.as_ref().unwrap();
    assert_eq!(tls.frontend_validation.as_ref().unwrap().ca_certificate_refs.len(), 16);
    assert_eq!(tls.resolved_frontend_ca_refs.as_ref().unwrap().len(), 16);
    assert_ne!(
        tls.frontend_matcher_eligibility,
        Some(FrontendMatcherEligibility::StrictReject)
    );
}

#[tokio::test]
async fn per_port_empty_tls_explicitly_disables_default_without_fallback() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let mut gateway = make_test_gateway("gw-ns");
    gateway.spec.tls = Some(GatewaySpecTLS {
        backend: None,
        frontend: Some(GatewayFrontendTLS {
            default: GatewayFrontendTLSDefault {
                validation: Some(frontend_validation("Secret", "default-ca", None)),
            },
            per_port: Some(vec![GatewayFrontendTLSPerPort {
                port: 443,
                tls: GatewayFrontendTLSDefault { validation: None },
            }]),
        }),
    });
    gateway.spec.listeners = Some(vec![GatewayListener {
        name: "https".to_string(),
        hostname: None,
        port: 443,
        protocol: "HTTPS".to_string(),
        tls: Some(GatewayTLSConfig {
            mode: Some(GatewayTlsMode::Terminate),
            certificate_refs: None,
            options: None,
            resolved_certificate_refs: None,
            frontend_validation: Some(frontend_validation("Secret", "forged", None)),
            resolved_frontend_ca_refs: None,
            frontend_matcher_eligibility: None,
        }),
        allowed_routes: None,
    }]);

    let ProcessResult::Continue(parsed) = GatewayHandler::new(None, None).parse(gateway, &ctx).await else {
        panic!("gateway parse unexpectedly stopped")
    };
    let tls = parsed.spec.listeners.unwrap()[0].tls.as_ref().unwrap().clone();
    assert!(tls.frontend_validation.is_none());
    assert!(tls.resolved_frontend_ca_refs.is_none());
}

#[test]
fn frontend_runtime_validation_rejects_duplicate_range_and_nonempty_group() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let mut gateway = make_test_gateway("gw-ns");
    let invalid = FrontendTLSValidation {
        ca_certificate_refs: vec![FrontendCACertificateRef {
            group: "core".to_string(),
            kind: "Secret".to_string(),
            name: "ca".to_string(),
            namespace: None,
        }],
        mode: None,
    };
    gateway.spec.tls = Some(GatewaySpecTLS {
        backend: None,
        frontend: Some(GatewayFrontendTLS {
            default: GatewayFrontendTLSDefault {
                validation: Some(invalid),
            },
            per_port: Some(vec![
                GatewayFrontendTLSPerPort {
                    port: 0,
                    tls: GatewayFrontendTLSDefault { validation: None },
                },
                GatewayFrontendTLSPerPort {
                    port: 0,
                    tls: GatewayFrontendTLSDefault { validation: None },
                },
            ]),
        }),
    });
    let errors = GatewayHandler::new(None, None).validate(&gateway, &ctx);
    assert!(errors.iter().any(|error| error.contains("outside 1..=65535")));
    assert!(errors.iter().any(|error| error.contains("duplicate port")));
    assert!(errors.iter().any(|error| error.contains("exact empty core group")));
}

#[test]
fn frontend_namespace_validation_accepts_internal_consecutive_hyphens() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let mut gateway = make_test_gateway("gw-ns");
    gateway.spec.tls = Some(GatewaySpecTLS {
        backend: None,
        frontend: Some(GatewayFrontendTLS {
            default: GatewayFrontendTLSDefault {
                validation: Some(FrontendTLSValidation {
                    ca_certificate_refs: vec![FrontendCACertificateRef {
                        group: String::new(),
                        kind: "Secret".to_string(),
                        name: "ca".to_string(),
                        namespace: Some("team--trust".to_string()),
                    }],
                    mode: None,
                }),
            },
            per_port: None,
        }),
    });
    assert!(GatewayHandler::new(None, None).validate(&gateway, &ctx).is_empty());
}

#[tokio::test]
async fn oversized_raw_listener_vector_is_revoked_before_traversal() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let handler = GatewayHandler::new(None, None);
    let mut gateway = make_test_gateway("gw-ns");
    gateway.metadata.name = Some("oversized".to_string());
    gateway.spec.listeners = Some(
        (0..=edgion_resources::resources::gateway::MAX_GATEWAY_LISTENERS)
            .map(|index| GatewayListener {
                name: format!("listener-{index}"),
                hostname: None,
                port: 10_000 + index as i32,
                protocol: "HTTPS".to_string(),
                tls: None,
                allowed_routes: None,
            })
            .collect(),
    );
    let errors = handler.validate(&gateway, &ctx);
    assert_eq!(errors.len(), 1);
    let parsed = match handler.parse(gateway, &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert!(
        parsed.spec.listeners.is_none(),
        "invalid update must revoke old listeners"
    );
}

#[test]
fn attachment_proof_excludes_private_key_and_metadata_noise() {
    let mut gateway = make_test_gateway("gw-ns");
    gateway.metadata.name = Some("gateway".to_string());
    gateway.metadata.uid = Some("uid-1".to_string());
    gateway.metadata.generation = Some(7);
    gateway.metadata.resource_version = Some("11".to_string());
    gateway.spec.listeners = Some(vec![GatewayListener {
        name: "https".to_string(),
        hostname: Some("example.com".to_string()),
        port: 443,
        protocol: "HTTPS".to_string(),
        tls: Some(GatewayTLSConfig {
            mode: Some(GatewayTlsMode::Terminate),
            certificate_refs: Some(vec![SecretObjectReference {
                group: None,
                kind: None,
                namespace: None,
                name: "server".to_string(),
            }]),
            options: None,
            resolved_certificate_refs: Some(vec![ResolvedServingCertificateReference {
                index: 0,
                source: ServingCertificateSourceIdentity {
                    group: String::new(),
                    kind: "Secret".to_string(),
                    namespace: "gw-ns".to_string(),
                    name: "server".to_string(),
                },
                secret: Some(Secret {
                    data: Some(BTreeMap::from([
                        ("tls.crt".to_string(), ByteString(b"public-cert".to_vec())),
                        ("tls.key".to_string(), ByteString(b"private-key-one".to_vec())),
                    ])),
                    ..Default::default()
                }),
                error: None,
            }]),
            frontend_validation: None,
            resolved_frontend_ca_refs: None,
            frontend_matcher_eligibility: None,
        }),
        allowed_routes: None,
    }]);

    let first = build_gateway_attachment_proof(&gateway, "boot-a", 10);
    gateway.spec.tls = Some(GatewaySpecTLS {
        backend: Some(GatewayBackendTLS {
            client_certificate_ref: Some(SecretObjectReference {
                group: None,
                kind: None,
                namespace: None,
                name: "backend-client".to_string(),
            }),
            resolved_client_certificate: Some(Secret {
                data: Some(BTreeMap::from([(
                    "tls.key".to_string(),
                    ByteString(b"unrelated-backend-private-key".to_vec()),
                )])),
                ..Default::default()
            }),
        }),
        frontend: None,
    });
    let backend_tls_changed = build_gateway_attachment_proof(&gateway, "boot-a", 10);
    assert_eq!(
        first.attachment_digest, backend_tls_changed.attachment_digest,
        "unrelated Gateway backend TLS must not churn attachment proof"
    );
    let restarted = build_gateway_attachment_proof(&gateway, "boot-b", 10);
    assert_eq!(
        first.instance, restarted.instance,
        "Kubernetes UID remains authoritative"
    );
    assert_ne!(
        first.epoch, restarted.epoch,
        "controller restarts must create a new proof epoch"
    );
    assert_eq!(first.attachment_digest, restarted.attachment_digest);

    let mut local_gateway = gateway.clone();
    local_gateway.metadata.uid = None;
    let local_first = build_gateway_attachment_proof(&local_gateway, "boot-a", 10);
    let local_restarted = build_gateway_attachment_proof(&local_gateway, "boot-b", 10);
    assert_ne!(local_first.instance, local_restarted.instance);
    assert_ne!(local_first.epoch, local_restarted.epoch);
    assert_eq!(local_first.attachment_digest, local_restarted.attachment_digest);
    gateway.metadata.resource_version = Some("99".to_string());
    gateway.spec.listeners.as_mut().unwrap()[0]
        .tls
        .as_mut()
        .unwrap()
        .resolved_certificate_refs
        .as_mut()
        .unwrap()[0]
        .secret
        .as_mut()
        .unwrap()
        .data
        .as_mut()
        .unwrap()
        .insert("tls.key".to_string(), ByteString(b"private-key-two".to_vec()));
    let private_key_changed = build_gateway_attachment_proof(&gateway, "boot-a", 10);
    assert_eq!(first.attachment_digest, private_key_changed.attachment_digest);
    assert_eq!(first.instance, "uid-1");
    assert_eq!(first.generation, 7);

    gateway.spec.listeners.as_mut().unwrap()[0]
        .tls
        .as_mut()
        .unwrap()
        .resolved_certificate_refs
        .as_mut()
        .unwrap()[0]
        .secret
        .as_mut()
        .unwrap()
        .data
        .as_mut()
        .unwrap()
        .insert("tls.crt".to_string(), ByteString(b"new-public-cert".to_vec()));
    let public_cert_changed = build_gateway_attachment_proof(&gateway, "boot-a", 10);
    assert_ne!(first.attachment_digest, public_cert_changed.attachment_digest);

    gateway.spec.listeners.as_mut().unwrap()[0]
        .tls
        .as_mut()
        .unwrap()
        .frontend_matcher_eligibility = Some(FrontendMatcherEligibility::StrictReject);
    let final_eligibility_changed = build_gateway_attachment_proof(&gateway, "boot-a", 10);
    assert_ne!(
        public_cert_changed.attachment_digest, final_eligibility_changed.attachment_digest,
        "final derived admission/transport outcome must be proof-bound"
    );

    gateway.spec.listeners.as_mut().unwrap()[0].allowed_routes = Some(AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("Same".to_string()),
            selector: None,
        }),
        kinds: None,
    });
    let authorization_changed = build_gateway_attachment_proof(&gateway, "boot-a", 10);
    assert_ne!(
        final_eligibility_changed.attachment_digest, authorization_changed.attachment_digest,
        "allowedRoutes attachment authorization must be proof-bound"
    );

    gateway.spec.resolved_attachment_proof = Some(authorization_changed.clone());
    assert_eq!(
        edgion_resources::resources::gateway::gateway_attachment_proof(&gateway),
        authorization_changed,
        "the downstream projection must expose the exact finalized proof"
    );
}

#[test]
fn delete_recreate_of_same_gateway_spec_receives_new_authoritative_epoch() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let handler = GatewayHandler::new(None, None);
    let mut gateway = make_test_gateway("gw-ns");
    gateway.metadata.name = Some("gateway".to_string());
    gateway.metadata.uid = Some("uid-1".to_string());
    gateway.metadata.generation = Some(1);

    handler.finalize_transport(&mut gateway, &ctx);
    let first = gateway.spec.resolved_attachment_proof.take().unwrap();
    handler.finalize_transport(&mut gateway, &ctx);
    let recreated = gateway.spec.resolved_attachment_proof.as_ref().unwrap();

    assert_eq!(first.instance, recreated.instance);
    assert_eq!(first.generation, recreated.generation);
    assert_eq!(first.attachment_digest, recreated.attachment_digest);
    assert_ne!(first.epoch, recreated.epoch);
}

#[tokio::test]
async fn attachment_dependency_barrier_covers_publish_unpublish_and_same_key_recreate() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::edgion_tls::EdgionTlsHandler;

    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
    let gateway_handler = Arc::new(GatewayHandler::new(None, None));
    let gateway_processor = Arc::new(ResourceProcessor::<Gateway>::new(
        ResourceKind::Gateway.as_str(),
        16,
        gateway_handler.clone(),
        runtime.clone(),
    ));
    let tls_handler = Arc::new(EdgionTlsHandler::default());
    let tls_processor = Arc::new(ResourceProcessor::<EdgionTls>::new(
        ResourceKind::EdgionTls.as_str(),
        16,
        tls_handler.clone(),
        runtime.clone(),
    ));
    runtime.processor_registry.register(gateway_processor.clone());
    runtime.processor_registry.register(tls_processor.clone());

    let mut gateway = make_test_gateway("test-ns");
    gateway.metadata.name = Some("gateway".to_string());
    gateway.metadata.uid = Some("uid-1".to_string());
    gateway.metadata.generation = Some(1);
    gateway.spec.listeners = Some(vec![GatewayListener {
        name: "https".to_string(),
        hostname: Some("example.com".to_string()),
        port: 443,
        protocol: "HTTPS".to_string(),
        tls: None,
        allowed_routes: None,
    }]);
    gateway_handler.finalize_transport(&mut gateway, &ctx);
    let first_proof = gateway.spec.resolved_attachment_proof.clone().unwrap();
    gateway_processor.save(gateway.clone(), None);

    let tls = EdgionTls {
        metadata: kube::core::ObjectMeta {
            name: Some("tls".to_string()),
            namespace: Some("test-ns".to_string()),
            ..Default::default()
        },
        spec: EdgionTlsSpec {
            current_status: None,
            parent_refs: Some(vec![ParentReference {
                group: None,
                kind: None,
                namespace: None,
                name: "gateway".to_string(),
                section_name: Some("https".to_string()),
                port: None,
            }]),
            hosts: vec!["example.com".to_string()],
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                namespace: None,
                name: "missing".to_string(),
            },
            client_auth: None,
            min_tls_version: None,
            ciphers: None,
            secret: None,
            resolved_listeners: None,
            resolved_listener_attachments: None,
            resolved_listener_attachment_index: Default::default(),
            resolved_log_labels: None,
        },
        status: None,
    };

    // Cache-before-edge: parse first installs the dependency, then reads
    // the already-published authoritative Gateway generation.
    let parsed = match tls_handler.parse(tls.clone(), &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert_eq!(
        parsed.spec.resolved_listener_attachments.as_ref().unwrap()[0].gateway_proof,
        first_proof
    );

    // Edge-before-hook: the post-publish hook reads the real reverse index
    // and queues the registered consumer only after cache publication.
    gateway_handler.after_publish(&gateway, &ctx).await;
    let queued = tokio::time::timeout(std::time::Duration::from_secs(1), tls_processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(queued.key, "test-ns/tls");
    tls_processor.workqueue().done(&queued.key);

    gateway_processor.remove("test-ns/gateway", None);
    gateway_handler.after_unpublish(&gateway, &ctx).await;
    let deleted = tokio::time::timeout(std::time::Duration::from_secs(1), tls_processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deleted.key, "test-ns/tls");
    tls_processor.workqueue().done(&deleted.key);

    gateway_handler.finalize_transport(&mut gateway, &ctx);
    let recreated_proof = gateway.spec.resolved_attachment_proof.clone().unwrap();
    assert_ne!(first_proof.epoch, recreated_proof.epoch);
    gateway_processor.save(gateway.clone(), None);
    gateway_handler.after_publish(&gateway, &ctx).await;
    let recreated = tokio::time::timeout(std::time::Duration::from_secs(1), tls_processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    tls_processor.workqueue().done(&recreated.key);

    let reparsed = match tls_handler.parse(tls, &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert_eq!(
        reparsed.spec.resolved_listener_attachments.as_ref().unwrap()[0].gateway_proof,
        recreated_proof
    );
}

fn make_gateway_with_plugin_annotation(ns: &str, annotation_value: &str) -> Gateway {
    let mut gw = make_test_gateway(ns);
    gw.metadata.name = Some("plugin-gateway".into());
    let mut annotations = BTreeMap::new();
    annotations.insert("edgion.io/edgion-plugins".to_string(), annotation_value.to_string());
    gw.metadata.annotations = Some(annotations);
    gw
}

#[test]
fn validate_plugin_ref_no_annotation_returns_none() {
    let runtime = ControllerRuntime::default();
    let gw = make_test_gateway("default");
    assert!(validate_gateway_plugin_ref(&gw, &runtime).is_none());
}

#[test]
fn validate_plugin_ref_empty_annotation_returns_none() {
    let runtime = ControllerRuntime::default();
    let gw = make_gateway_with_plugin_annotation("default", "  ");
    assert!(validate_gateway_plugin_ref(&gw, &runtime).is_none());
}

#[test]
fn validate_plugin_ref_missing_resource_returns_error() {
    let runtime = ControllerRuntime::default();
    // "default/my-plugin" does not exist in the (empty) registry.
    let gw = make_gateway_with_plugin_annotation("default", "default/my-plugin");
    let result = validate_gateway_plugin_ref(&gw, &runtime);
    assert!(result.is_some());
    assert!(result.unwrap().contains("default/my-plugin"));
}

#[test]
fn validate_plugin_ref_fallback_to_gateway_ns() {
    let runtime = ControllerRuntime::default();
    // Bare name "my-plugin" should be expanded to "my-ns/my-plugin".
    let gw = make_gateway_with_plugin_annotation("my-ns", "my-plugin");
    let result = validate_gateway_plugin_ref(&gw, &runtime);
    assert!(result.is_some());
    assert!(result.unwrap().contains("my-ns/my-plugin"));
}

#[test]
fn proxy_protocol_unsupported_extension_invalidates_previous_trust_and_recovers() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = proxy_protocol_test_context(runtime.clone());
    let gateway = make_proxy_protocol_gateway(r#"{"source":"resourceRef","resourceRef":{"name":"policy"}}"#);
    let resource_ref = ResourceRef::new(ResourceKind::Gateway, Some("default".into()), "gateway".into());
    for (kind, expected_valid) in [
        ("ProxyProtocolTrust", true),
        ("FutureTrust", false),
        ("ProxyProtocolTrust", true),
    ] {
        let target: EdgionConfigData = serde_json::from_value(serde_json::json!({
            "apiVersion":"edgion.io/v1","kind":"EdgionConfigData","metadata":{"namespace":"default","name":"policy"},
            "spec":{"data":{"type":kind,"config":{"mode":"required"}}}
        }))
        .unwrap();
        runtime
            .edgion_config_data_store
            .update(HashMap::from([("default/policy".into(), target)]), &HashSet::new());
        let resolved = resolve_inbound_proxy_protocol(&gateway, &ctx, &resource_ref);
        if expected_valid {
            assert!(matches!(resolved, ResolvedInboundProxyProtocol::Ready { .. }));
        } else {
            assert!(matches!(
                resolved,
                ResolvedInboundProxyProtocol::Invalid {
                    code: ProxyProtocolConfigErrorCode::ReferenceWrongType,
                    ..
                }
            ));
        }
    }
}

#[tokio::test]
async fn gateway_plugin_reference_resolves_bare_source_but_reports_collision() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::EdgionPlugins;
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    let mut source = EdgionPlugins::new("plugins", Default::default());
    raw.upsert("plugins".into(), Arc::new(source.clone()));
    let gateway = make_gateway_with_plugin_annotation("default", "plugins");
    assert!(validate_gateway_plugin_ref(&gateway, &runtime).is_none());
    source.metadata.namespace = Some("default".into());
    raw.upsert("default/plugins".into(), Arc::new(source));
    assert_eq!(
        validate_gateway_plugin_ref(&gateway, &runtime).as_deref(),
        Some("EdgionPlugins reference default/plugins is ambiguous")
    );
    raw.remove("plugins");
    assert!(validate_gateway_plugin_ref(&gateway, &runtime).is_none());
}

#[tokio::test]
async fn dns_gateway_actual_source_grant_transitions_and_invalid_owner_register_no_edges() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
    let handler = GatewayHandler::new(None, None);
    let mut source = make_gateway_with_dns_resolver_ref("default", "dns/resolver");
    source.metadata.name = Some("dns-gateway".into());
    source.metadata.namespace = None;
    for kind in [None, Some("Service"), Some("Gateway"), None] {
        runtime.ref_grant_store.remove("dns/allow");
        if let Some(kind) = kind {
            let group = if kind == "Service" {
                ""
            } else {
                "gateway.networking.k8s.io"
            };
            runtime.ref_grant_store.upsert("dns/allow".into(),Arc::new(serde_json::from_value(serde_json::json!({"metadata":{"namespace":"dns","name":"allow"},"spec":{"from":[{"group":group,"kind":kind,"namespace":"default"}],"to":[{"group":"edgion.io","kind":"LinkSys","name":"resolver"}]}})).unwrap()));
        }
        let ProcessResult::Continue(parsed) = handler.parse(source.clone(), &ctx).await else {
            panic!("parse")
        };
        assert_eq!(
            validate_gateway_dns_resolver_ref(&parsed, &runtime).is_none(),
            kind == Some("Gateway")
        );
        assert_eq!(
            runtime.cross_ns_ref_mgr.get_refs("dns"),
            vec![ResourceRef::new(ResourceKind::Gateway, None, "dns-gateway".into())]
        );
    }
    handler.on_delete(&source, &ctx).await;
    assert!(runtime.cross_ns_ref_mgr.get_refs("dns").is_empty());
    source.metadata.namespace = Some("".into());
    let ProcessResult::Continue(parsed) = handler.parse(source, &ctx).await else {
        panic!("parse")
    };
    assert!(validate_gateway_dns_resolver_ref(&parsed, &runtime).is_some());
    assert!(runtime.cross_ns_ref_mgr.get_refs("dns").is_empty());
}

#[tokio::test]
async fn actual_http_attachment_replaces_raw_dependencies_and_rejects_invalid_owner() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
    let handler = GatewayHandler::new(None, None);
    let mut source = make_gateway_with_plugin_annotation("default", "missing, /bad, default/good, bad/a/b");
    source.metadata.namespace = None;
    let _ = handler.parse(source.clone(), &ctx).await;
    let owner = ResourceRef::new(ResourceKind::Gateway, None, "plugin-gateway".into());
    assert_eq!(runtime.plugin_ref_mgr.get_refs("default/missing"), vec![owner.clone()]);
    assert_eq!(runtime.plugin_ref_mgr.get_refs("default/good"), vec![owner.clone()]);
    assert!(runtime.plugin_ref_mgr.get_refs("/bad").is_empty());
    assert!(runtime.plugin_ref_mgr.get_refs("bad/a/b").is_empty());
    assert!(
        validate_gateway_plugin_ref(&source, &runtime)
            .unwrap()
            .contains("not found"),
        "unwatched EP remains Missing error"
    );
    source
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert("edgion.io/edgion-plugins".into(), "replacement".into());
    let _ = handler.parse(source.clone(), &ctx).await;
    assert!(runtime.plugin_ref_mgr.get_refs("default/missing").is_empty());
    assert!(runtime.plugin_ref_mgr.get_refs("default/good").is_empty());
    assert_eq!(runtime.plugin_ref_mgr.get_refs("default/replacement"), vec![owner]);
    handler.on_delete(&source, &ctx).await;
    assert!(runtime.plugin_ref_mgr.get_refs("default/replacement").is_empty());
    source.metadata.name = None;
    let _ = handler.parse(source.clone(), &ctx).await;
    assert!(runtime.plugin_ref_mgr.get_refs("default/replacement").is_empty());
    assert!(validate_gateway_plugin_ref(&source, &runtime)
        .unwrap()
        .contains("invalid reference name"));
    source
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert("edgion.io/edgion-plugins".into(), " , \t\n".into());
    assert!(validate_gateway_plugin_ref(&source, &runtime).is_none());
}

#[tokio::test]
async fn actual_invalid_gateway_owner_is_rejected_before_watched_plugin_selection() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::EdgionPlugins;

    use crate::conf_mgr::sync_runtime::resource_processor::RawStore;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Default)]
    struct CountingPlugins {
        raw: LocalRawStore<EdgionPlugins>,
        reads: AtomicUsize,
    }
    impl RawStore<EdgionPlugins> for CountingPlugins {
        fn get(&self, key: &str) -> Option<Arc<EdgionPlugins>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.raw.get(key)
        }
        fn snapshot(&self) -> Vec<Arc<EdgionPlugins>> {
            self.raw.snapshot()
        }
    }
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(CountingPlugins::default());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    let mut source = EdgionPlugins::new("plugins", Default::default());
    raw.raw.upsert("plugins".into(), Arc::new(source.clone()));
    let mut owner = make_gateway_with_plugin_annotation("default", "plugins");
    assert!(validate_gateway_plugin_ref(&owner, &runtime).is_none());
    assert_eq!(raw.reads.swap(0, Ordering::SeqCst), 2);
    owner.metadata.name = None;
    let error = validate_gateway_plugin_ref(&owner, &runtime).unwrap();
    assert!(error.contains("invalid reference name"), "{error}");
    source.metadata.namespace = Some("default".into());
    raw.raw.upsert("default/plugins".into(), Arc::new(source));
    assert_eq!(validate_gateway_plugin_ref(&owner, &runtime), Some(error));
    assert_eq!(raw.reads.load(Ordering::SeqCst), 0);
    owner.metadata.name = Some("plugin-gateway".into());
    assert!(validate_gateway_plugin_ref(&owner, &runtime)
        .unwrap()
        .contains("ambiguous"));
    assert_eq!(raw.reads.load(Ordering::SeqCst), 2);
}
