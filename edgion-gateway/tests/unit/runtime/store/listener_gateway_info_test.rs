use super::*;

#[test]
fn dns_family_cold_publication_pins_old_gateway_and_preserves_invalid_override() {
    let store = ListenerGatewayInfoStore::new();
    let mut gateway = make_gateway("https", vec![]);
    let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
    gateway.metadata.annotations = Some(BTreeMap::from([(
        annotations::DNS_LOOKUP_FAMILY.into(),
        "ipv6Only".into(),
    )]));
    store.rebuild(&[gateway.clone()]);
    let old = store.get(&key)[0].clone();
    assert_eq!(old.dns_lookup_family, Some(Ok(DnsLookupFamily::Ipv6Only)));
    for token in ["all", "", "invalid"] {
        gateway
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(annotations::DNS_LOOKUP_FAMILY.into(), token.into());
        store.rebuild(&[gateway.clone()]);
        let current = store.get(&key);
        assert_eq!(
            current[0].dns_lookup_family,
            DnsLookupFamily::from_annotation(Some(token)).transpose()
        );
        assert_eq!(old.dns_lookup_family, Some(Ok(DnsLookupFamily::Ipv6Only)));
    }
}
use edgion_common::dns::resolver_select::{LinkSysKey, ResolverCandidate};
use edgion_resources::resources::common::Condition;
use edgion_resources::resources::gateway::{GatewaySpec, GatewayStatus, Listener, ListenerStatus};
use kube::api::ObjectMeta;
use std::collections::BTreeMap;

fn make_condition(type_: &str, status: &str) -> Condition {
    Condition {
        last_transition_time: "2024-01-01T00:00:00Z".to_string(),
        message: String::new(),
        observed_generation: None,
        reason: "Test".to_string(),
        status: status.to_string(),
        type_: type_.to_string(),
    }
}

fn make_listener_status(name: &str, conditions: Vec<Condition>) -> ListenerStatus {
    ListenerStatus {
        name: name.to_string(),
        supported_kinds: vec![],
        attached_routes: 0,
        conditions,
    }
}

fn make_gateway(listener_name: &str, conditions: Vec<Condition>) -> Gateway {
    crate::test_current_status::processed(Gateway {
        metadata: ObjectMeta {
            name: Some("test-gw".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: GatewaySpec {
            current_status: None,
            gateway_class_name: "test".to_string(),
            listeners: Some(vec![Listener {
                name: listener_name.to_string(),
                hostname: None,
                port: 443,
                protocol: "HTTPS".to_string(),
                tls: None,
                allowed_routes: None,
            }]),
            addresses: None,
            tls: None,
            allowed_listeners: None,
            infrastructure: None,
            default_scope: None,
            resolved_inbound_proxy_protocol: Default::default(),
            resolved_attachment_proof: None,
        },
        status: Some(GatewayStatus {
            addresses: None,
            conditions: None,
            listeners: Some(vec![make_listener_status(listener_name, conditions)]),
        }),
    })
}

#[test]
fn annotation_projection_preserves_absent_empty_and_unrelated_maps() {
    for annotations in [
        None,
        Some(BTreeMap::new()),
        Some(BTreeMap::from([(
            "example.com/unrelated".to_owned(),
            "ignored".repeat(128),
        )])),
    ] {
        let store = ListenerGatewayInfoStore::new();
        let mut gateway = make_gateway("https", vec![]);
        gateway.metadata.annotations = annotations;
        store.rebuild(&[gateway]);
        let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let flat = store.get(&key);
        assert_eq!(flat.len(), 1);
        let info = &flat[0];
        assert_eq!(info.metrics_test_key, None);
        assert_eq!(info.metrics_test_type, None);
        assert_eq!(info.require_sni_host_match, None);
        assert!(info.plugin_store_keys.is_empty());
        assert_eq!(info.dns_resolver_candidate, None);
    }
}

#[test]
fn annotation_projection_preserves_metrics_and_exact_sni_boolean() {
    for (sni, expected_sni, metrics_type, expected_type) in [
        ("true", true, "LB", TestType::Lb),
        ("false", false, "retry", TestType::Retry),
        ("True", false, "LATENCY", TestType::Latency),
        (" true ", false, "unknown", TestType::None),
        ("", false, "", TestType::None),
    ] {
        let store = ListenerGatewayInfoStore::new();
        let mut gateway = make_gateway("https", vec![]);
        gateway.metadata.annotations = Some(BTreeMap::from([
            (annotations::METRICS_TEST_KEY.to_owned(), sni.to_owned()),
            (annotations::METRICS_TEST_TYPE.to_owned(), metrics_type.to_owned()),
            (annotations::REQUIRE_SNI_HOST_MATCH.to_owned(), sni.to_owned()),
            ("example.com/unrelated".to_owned(), "ignored".to_owned()),
        ]));
        store.rebuild(&[gateway]);
        let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let flat = store.get(&key);
        let info = &flat[0];
        assert_eq!(info.metrics_test_key.as_deref(), Some(sni));
        assert_eq!(info.metrics_test_type, Some(expected_type));
        assert_eq!(info.require_sni_host_match, Some(expected_sni));
    }
}

#[test]
fn annotation_projection_preserves_plugin_namespace_fallback() {
    for (namespace, expected) in [
        (None, "default/local"),
        (Some(""), "/local"),
        (Some("team"), "team/local"),
    ] {
        for (plugins, expected_keys) in [
            (
                "local, shared/plugin local",
                if namespace == Some("") {
                    vec![]
                } else {
                    vec![expected, "shared/plugin"]
                },
            ),
            ("", vec![]),
        ] {
            let store = ListenerGatewayInfoStore::new();
            let mut gateway = make_gateway("https", vec![]);
            gateway.metadata.namespace = namespace.map(str::to_owned);
            gateway.metadata.annotations = Some(BTreeMap::from([(
                annotations::EDGION_PLUGINS.to_owned(),
                plugins.to_owned(),
            )]));
            store.rebuild(&[gateway]);
            let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
            let flat = store.get(&key);
            assert_eq!(flat[0].plugin_store_keys, expected_keys);
            assert_eq!(flat[0].namespace(), namespace);
        }
    }
}

#[test]
fn annotation_projection_preserves_dns_candidate_precedence() {
    let servers = ResolverCandidate::Servers(vec!["1.1.1.1".to_owned(), "8.8.8.8:5353".to_owned()]);
    let reference = ResolverCandidate::HttpDnsRef(LinkSysKey {
        namespace: "dns".to_owned(),
        name: "resolver".to_owned(),
    });
    for (inline, dns_ref, expected) in [
        (Some("1.1.1.1, 8.8.8.8:5353\t1.1.1.1"), None, Some(servers.clone())),
        (None, Some(" dns / resolver "), Some(reference.clone())),
        (Some("9.9.9.9"), Some("dns/resolver"), Some(reference)),
        (Some("1.1.1.1\n8.8.8.8:5353"), Some("bare-name"), Some(servers)),
        (None, Some("bare-name"), None),
        (Some(" , \t\n"), Some(""), None),
    ] {
        let store = ListenerGatewayInfoStore::new();
        let mut gateway = make_gateway("https", vec![]);
        let mut map = BTreeMap::from([("example.com/unrelated".to_owned(), "ignored".to_owned())]);
        for (key, value) in [
            (annotations::DNS_RESOLVER, inline),
            (annotations::DNS_RESOLVER_REF, dns_ref),
        ] {
            if let Some(value) = value {
                map.insert(key.to_owned(), value.to_owned());
            }
        }
        gateway.metadata.annotations = Some(map);
        store.rebuild(&[gateway]);
        let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let flat = store.get(&key);
        assert_eq!(flat[0].dns_resolver_candidate, expected);
    }
}

#[test]
fn test_not_programmed_no_status() {
    // Missing Controller projection cannot authorize a listener.
    let gw = Gateway {
        metadata: ObjectMeta::default(),
        spec: GatewaySpec {
            current_status: None,
            gateway_class_name: "t".to_string(),
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
    };
    assert!(gateway_listener_is_explicitly_ineligible(&gw, "https"));
}

#[test]
fn test_not_programmed_when_programmed_false() {
    let gw = make_gateway(
        "https",
        vec![
            make_condition("Accepted", "True"),
            make_condition("Programmed", "False"),
        ],
    );
    assert!(gateway_listener_is_explicitly_ineligible(&gw, "https"));
}

#[test]
fn test_not_programmed_when_accepted_false() {
    let gw = make_gateway("https", vec![make_condition("Accepted", "False")]);
    assert!(gateway_listener_is_explicitly_ineligible(&gw, "https"));
}

#[test]
fn test_not_programmed_false_when_all_true() {
    let gw = make_gateway(
        "https",
        vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
    );
    assert!(!gateway_listener_is_explicitly_ineligible(&gw, "https"));
}

#[test]
fn test_listener_missing_from_current_status_is_ineligible() {
    let gw = make_gateway("https", vec![make_condition("Programmed", "False")]);
    assert!(gateway_listener_is_explicitly_ineligible(&gw, "http"));
}

#[test]
fn listener_publication_uses_only_current_status() {
    let store = ListenerGatewayInfoStore::new();
    let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
    let mut gateway = make_gateway("https", vec![make_condition("Accepted", "True")]);
    let allowed = gateway.spec.current_status.clone();
    let rejected = make_gateway("https", vec![make_condition("Accepted", "False")])
        .spec
        .current_status;

    gateway.status = rejected.clone();
    store.rebuild(&[gateway.clone()]);
    assert!(!store.get_index(&key).is_empty());

    gateway.status = allowed.clone();
    gateway.spec.current_status = rejected;
    store.rebuild(&[gateway.clone()]);
    assert!(store.get_index(&key).is_empty());

    gateway.spec.current_status = None;
    store.rebuild(&[gateway.clone()]);
    assert!(store.get_index(&key).is_empty());

    gateway.spec.current_status = allowed;
    store.rebuild(&[gateway]);
    assert!(!store.get_index(&key).is_empty());
}

#[test]
fn test_rebuild_skips_not_programmed_listener() {
    let store = ListenerGatewayInfoStore::new();

    // Gateway with Programmed=False on its only listener
    let invalid_gw = make_gateway("https", vec![make_condition("Programmed", "False")]);
    store.rebuild(&[invalid_gw]);

    use edgion_resources::listener_key::ListenerKey;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    let key = ListenerKey::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 443), "HTTPS");
    assert!(
        store.get_index(&key).is_empty(),
        "Programmed=False listener should not be registered in the index"
    );
}

#[test]
fn test_rebuild_includes_programmed_true_listener() {
    let store = ListenerGatewayInfoStore::new();

    let valid_gw = make_gateway(
        "https",
        vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
    );
    store.rebuild(&[valid_gw]);

    use edgion_resources::listener_key::ListenerKey;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    let key = ListenerKey::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 443), "HTTPS");
    assert!(
        !store.get_index(&key).is_empty(),
        "Programmed=True listener should be registered in the index"
    );
}

#[test]
fn flat_and_index_share_the_same_gateway_info() {
    let store = ListenerGatewayInfoStore::new();
    let gateway = make_gateway(
        "https",
        vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
    );
    store.rebuild(&[gateway]);

    let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
    let flat = store.get(&key);
    let index = store.get_index(&key);
    let indexed = index.get("default/test-gw", "https").unwrap();

    assert_eq!(flat.len(), 1);
    assert!(Arc::ptr_eq(&flat[0], &indexed.info));
    assert_eq!(flat[0].gateway_key(), "default/test-gw");
    assert_eq!(flat[0].gateway_key().as_ptr(), indexed.info.gateway_key().as_ptr());
}

#[test]
fn index_carries_the_gateway_attachment_proof_from_the_same_snapshot() {
    let store = ListenerGatewayInfoStore::new();
    let mut gateway = make_gateway(
        "https",
        vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
    );
    let proof = GatewayAttachmentProof {
        instance: "controller-a".to_string(),
        generation: 7,
        epoch: "epoch-a".to_string(),
        attachment_digest: "a".repeat(64),
    };
    gateway.spec.resolved_attachment_proof = Some(proof.clone());
    store.rebuild(&[gateway]);

    let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
    let index = store.get_index(&key);
    let entry = index.get("default/test-gw", "https").unwrap();
    assert_eq!(entry.gateway_proof.as_ref(), Some(&proof));
}

#[test]
fn in_flight_gateway_info_survives_generation_replacement() {
    let store = ListenerGatewayInfoStore::new();
    let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
    let mut gateway = make_gateway(
        "https",
        vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
    );
    gateway.metadata.annotations = Some(std::collections::BTreeMap::from([(
        annotations::METRICS_TEST_KEY.to_string(),
        "old-generation".to_string(),
    )]));
    store.rebuild(&[gateway.clone()]);
    let old = Arc::clone(&store.get(&key)[0]);

    gateway
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(annotations::METRICS_TEST_KEY.to_string(), "new-generation".to_string());
    store.rebuild(&[gateway]);
    let new = Arc::clone(&store.get(&key)[0]);

    assert!(!Arc::ptr_eq(&old, &new));
    assert_eq!(old.metrics_test_key.as_deref(), Some("old-generation"));
    assert_eq!(new.metrics_test_key.as_deref(), Some("new-generation"));
    assert_eq!(old.gateway_key(), "default/test-gw");
    assert_eq!(new.gateway_key(), "default/test-gw");
}

#[test]
fn listener_port_replacement_preserves_the_old_index_snapshot() {
    let store = ListenerGatewayInfoStore::new();
    let mut gateway = make_gateway("https", vec![]);
    let old_key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
    let new_key = ListenerKey::new("0.0.0.0:8443".parse().unwrap(), "HTTPS");
    store.rebuild(&[gateway.clone()]);
    let old = store.get_index(&old_key);
    assert_eq!(old.get("default/test-gw", "https").unwrap().port, 443);
    gateway.spec.listeners.as_mut().unwrap()[0].port = 8443;
    store.rebuild(&[gateway]);
    assert!(store.get_index(&old_key).is_empty());
    assert_eq!(
        store.get_index(&new_key).get("default/test-gw", "https").unwrap().port,
        8443
    );
    assert_eq!(old.get("default/test-gw", "https").unwrap().port, 443);
}

#[test]
fn retained_gateway_identity_survives_store_replacement() {
    let store = ListenerGatewayInfoStore::new();
    let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
    let mut gateway = make_gateway(
        "https",
        vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
    );
    store.rebuild(&[gateway.clone()]);
    let old = Arc::clone(&store.get(&key)[0]);
    gateway.metadata.namespace = Some("new-namespace".to_owned());
    gateway.metadata.name = Some("new-gateway".to_owned());
    store.rebuild(&[gateway]);

    let index = store.get_index(&key);
    assert!(index.get("default/test-gw", "https").is_none());
    let new = &index.get("new-namespace/new-gateway", "https").unwrap().info;
    assert_eq!(old.namespace(), Some("default"));
    assert_eq!(old.gateway_name(), "test-gw");
    assert_eq!(old.gateway_key(), "default/test-gw");
    assert_eq!(new.namespace(), Some("new-namespace"));
    assert_eq!(new.gateway_name(), "new-gateway");
    assert_eq!(new.gateway_key(), "new-namespace/new-gateway");
    assert!(!Arc::ptr_eq(&old, new));
}

#[test]
fn dns_gateway_owner_and_inline_are_independent_and_old_info_stays_pinned() {
    let store = ListenerGatewayInfoStore::new();
    let mut gateway = make_gateway("https", vec![]);
    gateway.metadata.namespace = None;
    gateway.metadata.annotations = Some(BTreeMap::from([
        (annotations::DNS_RESOLVER_REF.into(), "dns/resolver".into()),
        (annotations::DNS_RESOLVER.into(), "192.0.2.53".into()),
    ]));
    let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
    store.rebuild(&[gateway.clone()]);
    let old = store.get(&key);
    assert!(matches!(
        old[0].dns_resolver_candidate,
        Some(ResolverCandidate::HttpDnsRef(_))
    ));
    gateway.metadata.namespace = Some("".into());
    store.rebuild(&[gateway]);
    assert_eq!(
        store.get(&key)[0].dns_resolver_candidate,
        Some(ResolverCandidate::Servers(vec!["192.0.2.53".into()]))
    );
    assert!(matches!(
        old[0].dns_resolver_candidate,
        Some(ResolverCandidate::HttpDnsRef(_))
    ));
}

#[test]
fn actual_http_attachment_keys_match_publication_and_pin_old_listener_info() {
    let store = ListenerGatewayInfoStore::new();
    let mut owner = make_gateway("https", vec![]);
    owner.metadata.namespace = None;
    owner.metadata.annotations = Some(BTreeMap::from([(
        annotations::EDGION_PLUGINS.into(),
        "local, default/local /bad other/good bad/a/b missing".into(),
    )]));
    let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
    store.rebuild(&[owner.clone()]);
    let old = store.get(&key);
    let expected = crate::plugins::publication::inputs::GatewayInput::from_gateway(&owner).http_keys;
    assert_eq!(expected, ["default/local", "other/good", "default/missing"]);
    assert_eq!(old[0].plugin_store_keys, expected);
    owner.metadata.name = None;
    store.rebuild(&[owner.clone()]);
    assert!(store.get(&key)[0].plugin_store_keys.is_empty());
    assert!(crate::plugins::publication::inputs::GatewayInput::from_gateway(&owner)
        .http_keys
        .is_empty());
    assert_eq!(old[0].plugin_store_keys, expected);
}
