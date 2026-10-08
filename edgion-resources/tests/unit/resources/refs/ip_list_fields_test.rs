use super::*;
use serde_json::json;

fn http(reference: serde_json::Value) -> EdgionPlugins {
    serde_json::from_value(json!({"metadata":{"name":"owner"},"spec":{"requestPlugins":[{
        "type":"IpRestriction","enable":false,"config":{"allowRefs":[reference],"denyRefs":[{"name":"deny"}]}
    }]}}))
    .unwrap()
}
#[test]
fn actual_disabled_http_slots_keep_default_identity_and_cannot_freeze() {
    let owner = http(json!({"name":"allow"}));
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    let binding = prepared.bind_plugins(&owner).unwrap();
    let entry = binding.http_entry(0).unwrap();
    let refs = entry.references();
    refs.validate_owner().unwrap();
    for (token, denied) in refs.allow().chain(refs.deny()) {
        let bound = token.validated().unwrap();
        assert_eq!(bound.source().group_kind().kind(), "EdgionPlugins");
        assert_eq!(bound.source().scope().namespace(), Some("default"));
        assert_eq!(bound.target().scope().namespace(), Some("default"));
        assert_eq!(bound.target().group_kind().kind(), "EdgionConfigData");
        assert!(!denied);
        assert!(token.freeze().is_err());
    }
}
#[test]
fn owner_failure_is_separate_from_invalid_target_and_inline_owner_needs_no_name() {
    let owner = http(json!({"name":"nets","namespace":""}));
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    let binding = prepared.bind_plugins(&owner).unwrap();
    let refs = binding.http_entry(0).unwrap().references();
    assert!(refs.validate_owner().is_ok());
    assert!(refs.allow().next().unwrap().0.validated().is_err());
    let mut owner = owner;
    owner.metadata.namespace = Some(String::new());
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    let binding = prepared.bind_plugins(&owner).unwrap();
    assert!(binding.http_entry(0).unwrap().references().validate_owner().is_err());
    let owner: EdgionPlugins =
        serde_json::from_value(json!({"metadata":{},"spec":{"requestPlugins":[{"type":"IpRestriction","config":{}}]}}))
            .unwrap();
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    assert!(prepared
        .bind_plugins(&owner)
        .unwrap()
        .http_entry(0)
        .unwrap()
        .references()
        .validate_owner()
        .is_ok());
}
#[test]
fn authored_topology_drift_rejects_but_denial_marker_updates_remain_bound() {
    let mut owner = http(json!({"name":"allow"}));
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    prepared
        .visit_plugins_mut(&mut owner, |mut slot| {
            slot.set_denied(Some(RefDenied {
                target_namespace: "default".into(),
                target_name: "allow".into(),
                reason: None,
            }))
        })
        .unwrap();
    assert!(
        prepared
            .bind_plugins(&owner)
            .unwrap()
            .http_entry(0)
            .unwrap()
            .references()
            .allow()
            .next()
            .unwrap()
            .1
    );
    for mutation in ["uid", "enable", "target", "entry"] {
        let mut changed = owner.clone();
        match mutation {
            "uid" => changed.metadata.uid = Some("different".into()),
            "enable" => changed.spec.request_plugins.as_mut().unwrap()[0].enable = true,
            "entry" => changed.spec.request_plugins.as_mut().unwrap().clear(),
            _ => {
                let EdgionPlugin::IpRestriction(config) = &mut changed.spec.request_plugins.as_mut().unwrap()[0].plugin
                else {
                    panic!()
                };
                config.allow_refs.as_mut().unwrap()[0].name = "other".into();
            }
        }
        assert!(matches!(
            prepared.bind_plugins(&changed),
            Err(ReferenceError::StaleBinding)
        ));
    }
}
#[test]
fn stream_profiles_and_tls_bind_actual_owner_and_detect_profile_drift() {
    let owner: EdgionStreamPlugins = serde_json::from_value(json!({"metadata":{"name":"stream"},"spec":{
        "plugins":[{"type":"IpRestriction","enable":false,"config":{"allowRefs":[{"name":"l4"}]}},
        {"type":"GlobalConnectionIpRestriction","config":{"enable":true,"activeProfile":"a","profiles":{
            "z":{"denyRefs":[{"name":"z"}]},"a":{"allowRefs":[{"name":"a"}]}}}}],
        "tlsRoutePlugins":[{"type":"IpRestriction","config":{"denyRefs":[{"name":"tls"}]}}]
    }}))
    .unwrap();
    let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
    let binding = prepared.bind_stream_plugins(&owner).unwrap();
    let names: Vec<_> = binding
        .references()
        .flat_map(|refs| {
            refs.allow()
                .chain(refs.deny())
                .map(|(token, _)| token.validated().unwrap().target().name().to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(names, ["l4", "a", "z", "tls"]);
    assert_eq!(
        binding
            .global_entry(1)
            .unwrap()
            .profiles()
            .map(|(name, _)| name)
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    let mut changed = owner.clone();
    let EdgionStreamPlugin::GlobalConnectionIpRestriction(config) =
        &mut changed.spec.plugins.as_mut().unwrap()[1].plugin
    else {
        panic!()
    };
    config.profiles.remove("a");
    assert!(matches!(
        prepared.bind_stream_plugins(&changed),
        Err(ReferenceError::StaleBinding)
    ));
}

#[test]
fn side_exchange_and_ordered_target_reordering_invalidate_prepared_slots() {
    let mut owner = http(json!({"name":"first"}));
    let EdgionPlugin::IpRestriction(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
        panic!()
    };
    config.allow_refs.as_mut().unwrap().push(EdgionConfigDataRef {
        name: "second".into(),
        ..Default::default()
    });
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    for swap_sides in [false, true] {
        let mut changed = owner.clone();
        let EdgionPlugin::IpRestriction(config) = &mut changed.spec.request_plugins.as_mut().unwrap()[0].plugin else {
            panic!()
        };
        if swap_sides {
            std::mem::swap(&mut config.allow_refs, &mut config.deny_refs);
        } else {
            config.allow_refs.as_mut().unwrap().swap(0, 1);
        }
        assert!(matches!(
            prepared.bind_plugins(&changed),
            Err(ReferenceError::StaleBinding)
        ));
    }
}
