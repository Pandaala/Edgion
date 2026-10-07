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
