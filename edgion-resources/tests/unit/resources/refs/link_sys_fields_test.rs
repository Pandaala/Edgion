use super::*;
use serde_json::json;

fn link_sys(provider: &str, config: serde_json::Value) -> LinkSys {
    serde_json::from_value(json!({
        "apiVersion":"edgion.io/v1", "kind":"LinkSys",
        "metadata":{"name":"connection", "namespace":"links", "uid":"uid", "generation":3},
        "spec":{"type":provider, "config":config}
    }))
    .unwrap()
}

#[test]
fn auth_borrows_actual_provider_owner_and_defaults_target_namespace() {
    let owner = link_sys(
        "redis",
        json!({"endpoints":["redis://redis:6379"], "auth":{"secretRef":{"group":"core", "name":"password"}}}),
    );
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let references = binding.references().unwrap();
    let token = references.auth_secret().unwrap();
    let reference = token.validated().unwrap();
    assert_eq!(reference.owner().name(), "connection");
    assert_eq!(reference.source().group_kind().kind(), "LinkSys");
    assert_eq!(reference.target().scope().namespace(), Some("links"));
    assert_eq!(reference.target().group_kind().group(), "");
    assert!(references.active_bootstrap_secret().is_none());
}

#[test]
fn source_checks_actual_owner_even_without_active_references() {
    let original = link_sys("webhook", json!({"target":{"name":"svc", "port":8080}}));
    for (namespace, name, expected) in [
        (None, Some("connection"), Ok("default")),
        (Some("links"), Some("connection"), Ok("links")),
        (Some(""), Some("connection"), Err(ReferenceError::InvalidNamespace)),
        (Some("Bad"), Some("connection"), Err(ReferenceError::InvalidNamespace)),
        (None, None, Err(ReferenceError::InvalidName)),
        (None, Some("bad/name"), Err(ReferenceError::InvalidName)),
    ] {
        let mut owner = original.clone();
        owner.metadata.namespace = namespace.map(str::to_owned);
        owner.metadata.name = name.map(str::to_owned);
        let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
        let binding = prepared.bind(&owner).unwrap();
        assert_eq!(
            binding.source().map(|source| source.scope().namespace().unwrap()),
            expected
        );
    }
}

#[test]
fn disabled_tls_lends_no_slots_and_keeps_independent_auth() {
    let owner = link_sys(
        "redis",
        json!({
            "auth":{"secretRef":{"name":"password"}},
            "tls":{"enabled":false, "validation":{"caCertificateRefs":[{"group":"foreign", "kind":"", "name":"bad"}]}, "clientCertificateRef":{"name":"bad/name"}}
        }),
    );
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let references = binding.references().unwrap();
    assert!(references.tls().is_none());
    assert!(references.auth_secret().unwrap().validated().is_ok());
}

#[test]
fn provider_ca_skip_policy_never_skips_wrong_group_secret_or_malformed_identity() {
    let tls = json!({"enabled":true, "validation":{"caCertificateRefs":[
        {"kind":"ConfigMap", "name":"cm"},
        {"kind":"Secret", "group":"foreign.example", "name":"foreign"},
        {"kind":"", "name":"bad"},
        {"group":"core", "kind":"Secret", "name":"ca"}
    ]}});
    for provider in ["redis", "otlp"] {
        let owner = link_sys(provider, json!({"endpoint":"https://collector.example", "tls":tls}));
        let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let references = binding.references().unwrap();
        let tls = references.tls().unwrap();
        let slots: Vec<_> = tls.ca_certificates().collect();
        if provider == "redis" {
            assert!(matches!(slots[0], LinkSysCaSlot::UnsupportedSkipped));
        } else {
            let LinkSysCaSlot::Secret(token) = &slots[0] else {
                panic!("OTLP reports unsupported CA");
            };
            assert_eq!(token.validated().unwrap_err(), ReferenceError::UnsupportedTarget);
        }
        let LinkSysCaSlot::Secret(token) = &slots[1] else {
            panic!("wrong group must be invalid");
        };
        assert_eq!(token.validated().unwrap_err(), ReferenceError::UnsupportedTarget);
        let LinkSysCaSlot::Secret(token) = &slots[2] else {
            panic!("malformed kind must be invalid");
        };
        assert_eq!(token.validated().unwrap_err(), ReferenceError::InvalidKind);
        let LinkSysCaSlot::Secret(token) = &slots[3] else {
            panic!("valid sibling preserved");
        };
        assert_eq!(token.validated().unwrap().target().name(), "ca");
    }
}

#[test]
fn same_reference_under_changed_auth_variant_cannot_reuse_binding() {
    let owner = link_sys(
        "elasticsearch",
        json!({"endpoints":["https://es.example"], "auth":{"type":"basic", "secretRef":{"name":"credential"}}}),
    );
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    let changed = link_sys(
        "elasticsearch",
        json!({"endpoints":["https://es.example"], "auth":{"type":"bearer", "secretRef":{"name":"credential"}}}),
    );
    assert!(matches!(prepared.bind(&changed), Err(ReferenceError::StaleBinding)));
}

#[test]
fn resolved_material_write_is_allowed_but_new_reference_loan_detects_auth_drift() {
    let mut owner = link_sys("redis", json!({"auth":{"secretRef":{"name":"password"}}}));
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    let mut binding = prepared.bind_mut(&mut owner).unwrap();
    let SystemConfig::Redis(config) = binding.config_mut() else {
        panic!("expected Redis");
    };
    config.auth.as_mut().unwrap().slot.secret = Some(Default::default());
    assert!(binding.references().is_ok());
    let SystemConfig::Redis(config) = binding.config_mut() else {
        panic!("expected Redis");
    };
    config.auth.as_mut().unwrap().slot.secret_ref.name = "other".to_string();
    assert!(matches!(binding.references(), Err(ReferenceError::StaleBinding)));
}

#[test]
fn frozen_link_sys_binding_rejects_tls_reordering_and_wrong_owner_family() {
    let mut owner = link_sys(
        "redis",
        json!({
            "auth":{"secretRef":{"name":"password"}},
            "tls":{"enabled":true, "validation":{"caCertificateRefs":[{"kind":"Secret", "name":"one"}, {"kind":"Secret", "name":"two"}]}}
        }),
    );
    let frozen = {
        let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let references = binding.references().unwrap();
        references.auth_secret().unwrap().freeze().unwrap()
    };
    assert!(frozen.matches_link_sys(&owner));
    let plugin_owner = crate::EdgionPlugins::new("connection", Default::default());
    assert!(!frozen.matches_plugins(&plugin_owner));
    let SystemConfig::Redis(config) = &mut owner.spec.config else {
        panic!("expected Redis");
    };
    config
        .tls
        .as_mut()
        .unwrap()
        .outbound
        .validation
        .as_mut()
        .unwrap()
        .ca_certificate_refs
        .as_mut()
        .unwrap()
        .swap(0, 1);
    assert!(!frozen.matches_link_sys(&owner));
}

fn auth_owner(provider: &str, variant: &str) -> LinkSys {
    let slot = json!({"secretRef":{"name":"auth","namespace":"credentials","group":"core"},
        "secret":{"metadata":{"name":"auth","namespace":"credentials"},"data":{"password":"cA=="}}});
    let config = match provider {
        "redis" => json!({"endpoints":["redis://redis:6379"],"auth":slot}),
        "etcd" => json!({"endpoints":["http://etcd:2379"],"auth":slot}),
        "elasticsearch" => {
            let mut auth = slot;
            auth["type"] = json!(variant);
            json!({"endpoints":["http://es:9200"],"auth":auth})
        }
        "kafka" => json!({"brokers":["kafka:9092"],"sasl":{"username":"alice","password":slot}}),
        "otlp" => json!({"endpoint":"http://collector:4317","auth":slot}),
        _ => unreachable!(),
    };
    link_sys(provider, config)
}

fn auth_slot_mut(owner: &mut LinkSys) -> &mut crate::resources::common::SecretSlot {
    match &mut owner.spec.config {
        SystemConfig::Redis(c) => &mut c.auth.as_mut().unwrap().slot,
        SystemConfig::Etcd(c) => &mut c.auth.as_mut().unwrap().slot,
        SystemConfig::Elasticsearch(c) => match c.auth.as_mut().unwrap() {
            ElasticsearchAuth::Basic { slot }
            | ElasticsearchAuth::ApiKey { slot }
            | ElasticsearchAuth::Bearer { slot } => slot,
        },
        SystemConfig::Kafka(c) => &mut c.sasl.as_mut().unwrap().password,
        SystemConfig::Otlp(c) => &mut c.auth.as_mut().unwrap().slot,
        _ => unreachable!(),
    }
}

#[test]
fn auth_material_is_an_actual_owner_borrow_with_strict_metadata_for_every_provider() {
    for (provider, variant) in [
        ("redis", ""),
        ("etcd", ""),
        ("elasticsearch", "basic"),
        ("elasticsearch", "apiKey"),
        ("elasticsearch", "bearer"),
        ("kafka", ""),
        ("otlp", ""),
    ] {
        let mut owner = auth_owner(provider, variant);
        let expected = auth_slot_mut(&mut owner).secret.as_ref().unwrap() as *const _;
        let secret = {
            let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
            prepared.resolved_auth_secret(&owner).unwrap().unwrap()
        };
        assert_eq!(secret as *const _, expected);
        assert!(secret.metadata.uid.is_none());
        for dimension in 0..6 {
            let mut bad = owner.clone();
            let material = auth_slot_mut(&mut bad).secret.as_mut().unwrap();
            match dimension {
                0 => material.metadata.name = None,
                1 => material.metadata.name = Some(String::new()),
                2 => material.metadata.name = Some("other".into()),
                3 => material.metadata.namespace = None,
                4 => material.metadata.namespace = Some(String::new()),
                _ => material.metadata.namespace = Some("links".into()),
            }
            assert_eq!(
                PreparedLinkSysReferences::from_link_sys(&bad)
                    .resolved_auth_secret(&bad)
                    .unwrap_err(),
                ReferenceError::StaleBinding
            );
        }
        let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
        auth_slot_mut(&mut owner).secret_ref.name = "retargeted".into();
        assert_eq!(
            prepared.resolved_auth_secret(&owner).unwrap_err(),
            ReferenceError::StaleBinding
        );
        assert_eq!(
            PreparedLinkSysReferences::from_link_sys(&owner)
                .resolved_auth_secret(&owner)
                .unwrap_err(),
            ReferenceError::StaleBinding
        );
        auth_slot_mut(&mut owner).secret = None;
        auth_slot_mut(&mut owner).secret_ref.group = Some("invalid/group".into());
        assert!(PreparedLinkSysReferences::from_link_sys(&owner)
            .resolved_auth_secret(&owner)
            .unwrap()
            .is_none());
    }
}

#[test]
fn auth_material_defaults_and_inactive_axes_do_not_borrow_tls_or_bootstrap_authority() {
    let mut owner = auth_owner("redis", "");
    owner.metadata.namespace = None;
    let slot = auth_slot_mut(&mut owner);
    slot.secret_ref.namespace = None;
    slot.secret.as_mut().unwrap().metadata.namespace = Some("default".into());
    for group in [None, Some(""), Some("core")] {
        auth_slot_mut(&mut owner).secret_ref.group = group.map(str::to_owned);
        assert!(PreparedLinkSysReferences::from_link_sys(&owner)
            .resolved_auth_secret(&owner)
            .unwrap()
            .is_some());
    }
    if let SystemConfig::Redis(c) = &mut owner.spec.config {
        c.tls =
            Some(serde_json::from_value(json!({"enabled":true,"clientCertificateRef":{"name":"bad/name"}})).unwrap());
    }
    assert!(PreparedLinkSysReferences::from_link_sys(&owner)
        .resolved_auth_secret(&owner)
        .unwrap()
        .is_some());
    auth_slot_mut(&mut owner).secret.as_mut().unwrap().metadata.namespace = None;
    assert_eq!(
        PreparedLinkSysReferences::from_link_sys(&owner)
            .resolved_auth_secret(&owner)
            .unwrap_err(),
        ReferenceError::StaleBinding
    );
    auth_slot_mut(&mut owner).secret.as_mut().unwrap().metadata.namespace = Some("default".into());
    owner.metadata.name = None;
    assert!(PreparedLinkSysReferences::from_link_sys(&owner)
        .resolved_auth_secret(&owner)
        .is_err());
    if let SystemConfig::Redis(c) = &mut owner.spec.config {
        c.auth = None;
    }
    assert!(PreparedLinkSysReferences::from_link_sys(&owner)
        .resolved_auth_secret(&owner)
        .unwrap()
        .is_none());
    let webhook = link_sys("webhook", json!({"target":{"name":"svc","port":8080}}));
    assert!(PreparedLinkSysReferences::from_link_sys(&webhook)
        .resolved_auth_secret(&webhook)
        .unwrap()
        .is_none());
}

#[test]
fn auth_material_stale_owner_and_variant_require_a_fresh_prepared_binding() {
    let original = auth_owner("elasticsearch", "basic");
    let prepared = PreparedLinkSysReferences::from_link_sys(&original);
    assert!(prepared.resolved_auth_secret(&original).unwrap().is_some());
    for variant_change in [false, true] {
        let mut changed = original.clone();
        if variant_change {
            let SystemConfig::Elasticsearch(config) = &mut changed.spec.config else {
                unreachable!()
            };
            let Some(ElasticsearchAuth::Basic { slot }) = config.auth.take() else {
                unreachable!()
            };
            config.auth = Some(ElasticsearchAuth::Bearer { slot });
        } else {
            changed.metadata.uid = Some("replacement-owner-uid".into());
        }
        let expected = auth_slot_mut(&mut changed).secret.as_ref().unwrap() as *const _;
        // Ref and material stay mutually valid: only the prepared owner/topology is stale.
        assert_eq!(
            prepared.resolved_auth_secret(&changed).unwrap_err(),
            ReferenceError::StaleBinding
        );
        let fresh = PreparedLinkSysReferences::from_link_sys(&changed);
        assert_eq!(
            fresh.resolved_auth_secret(&changed).unwrap().unwrap() as *const _,
            expected
        );
    }
}
