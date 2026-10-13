use super::*;
use crate::RefDenied;
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
        .visit_acl_plugins_mut(&mut owner, |mut slot| {
            slot.set_denied(
                IpListSide::Allow,
                vec![Some(RefDenied {
                    target_namespace: "default".into(),
                    target_name: "allow".into(),
                    reason: None,
                })],
            )
            .unwrap();
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
                config.refs.allow_refs.as_mut().unwrap()[0].name = "other".into();
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
    config.refs.allow_refs.as_mut().unwrap().push(EdgionConfigDataRef {
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
            std::mem::swap(&mut config.refs.allow_refs, &mut config.refs.deny_refs);
        } else {
            config.refs.allow_refs.as_mut().unwrap().swap(0, 1);
        }
        assert!(matches!(
            prepared.bind_plugins(&changed),
            Err(ReferenceError::StaleBinding)
        ));
    }
}

fn geo_owner() -> crate::EdgionPlugins {
    serde_json::from_value(serde_json::json!({"metadata":{"name":"geo","namespace":"source"},"spec":{"requestPlugins":[{"type":"GeoIpLocation","config":{"failOpen":false,"allowRuleGroups":[{"rules":[{"source":"DirectPeerIp","key":"cidr","ipRefs":[{"name":"networks","namespace":"target"}]}]}]}}]}})).unwrap()
}

#[test]
fn geo_actual_loan_rejects_wrong_scope_source_domain_and_authored_retarget() {
    use crate::resources::edgion_plugins::ip_reference_set::{IpReadDomain, IpReadScope};
    let mut owner = geo_owner();
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    prepared
        .visit_geo_plugins_mut(&mut owner, |mut leaf| {
            assert!(leaf.is_supported());
            assert!(leaf.set_read_scopes(&[IpReadScope::SameNamespace]).is_err());
            leaf.set_read_scopes(&[IpReadScope::ClusterOnly]).unwrap();
        })
        .unwrap();
    prepared
        .visit_geo_plugins(&owner, |leaf| {
            assert_eq!(leaf.read_scopes().unwrap(), Some(vec![IpReadScope::ClusterOnly]));
            assert_eq!(
                leaf.config().resolved_refs.as_deref().unwrap().domain,
                IpReadDomain::IpList
            );
        })
        .unwrap();
    prepared
        .visit_geo_plugins_mut(&mut owner, |mut leaf| {
            leaf.config_mut().resolved_refs.as_mut().unwrap().domain = IpReadDomain::Condition
        })
        .unwrap();
    prepared
        .visit_geo_plugins(&owner, |leaf| assert!(leaf.read_scopes().is_err()))
        .unwrap();
    prepared
        .visit_geo_plugins_mut(&mut owner, |mut leaf| {
            leaf.set_read_scopes(&[IpReadScope::ClusterOnly]).unwrap()
        })
        .unwrap();
    let EdgionPlugin::GeoIpLocation(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
        unreachable!()
    };
    config.allow_rule_groups[0].rules[0].source =
        crate::resources::edgion_plugins::plugin_configs::GeoRuleSource::RemoteIp;
    assert!(prepared.bind_plugins(&owner).is_err());
}

#[test]
fn geo_disabled_and_wrong_stage_remain_clearable_without_consuming_authority() {
    let mut owner = geo_owner();
    owner.spec.request_plugins.as_mut().unwrap()[0].enable = false;
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    let mut count = 0;
    prepared
        .visit_geo_plugins_mut(&mut owner, |mut leaf| {
            count += 1;
            assert!(!leaf.is_supported());
            leaf.config_mut().resolved_refs = None;
            for reference in &mut leaf.config_mut().ip_refs {
                reference.ref_denied = None;
            }
        })
        .unwrap();
    assert_eq!(count, 1);
    let entry = owner.spec.request_plugins.take().unwrap().remove(0);
    owner.spec.upstream_response_body_filter_plugins =
        Some(vec![
            serde_json::from_value(serde_json::to_value(entry).unwrap()).unwrap()
        ]);
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    prepared
        .visit_geo_plugins(&owner, |leaf| assert!(!leaf.is_supported()))
        .unwrap();
}

#[test]
fn actual_condition_trust_proof_cannot_be_transplanted_into_geo_root_zero() {
    use crate::resources::edgion_plugins::ip_reference_set::IpReadScope;
    use crate::resources::refs::condition_fields::PreparedConditionReferences;
    let mut owner: crate::EdgionPlugins=serde_json::from_value(serde_json::json!({"metadata":{"name":"geo","namespace":"source"},"spec":{"requestPlugins":[{"type":"RealIp","config":{"trustedIpRefs":[{"name":"networks","namespace":"target"}]}},{"type":"GeoIpLocation","config":{"failOpen":false,"allowRuleGroups":[{"rules":[{"source":"DirectPeerIp","key":"cidr","ipRefs":[{"name":"networks","namespace":"target"}]}]}]}}]}})).unwrap();
    let condition = PreparedConditionReferences::from_plugins(&owner);
    condition
        .visit_ip_mut(
            &mut owner,
            |mut leaf, _| leaf.set_read_scopes(&[IpReadScope::ClusterOnly]).unwrap(),
            |_, _| {},
        )
        .unwrap();
    let mut proof = None;
    condition
        .visit_ip(&owner, |leaf| proof = leaf.config().resolved_refs.clone())
        .unwrap();
    let geo = PreparedIpListReferences::from_plugins(&owner);
    geo.visit_geo_plugins_mut(&mut owner, |mut leaf| leaf.config_mut().resolved_refs = proof.clone())
        .unwrap();
    geo.visit_geo_plugins(&owner, |leaf| {
        assert!(
            leaf.read_scopes().is_err(),
            "same owner/root/leaf/targets cannot cross catalogue domain"
        )
    })
    .unwrap();
}

#[test]
fn acl_actual_side_profile_source_domain_proofs_reject_transplants() {
    use crate::resources::edgion_plugins::ip_reference_set::IpReadScope;
    let mut owner:EdgionPlugins=serde_json::from_value(json!({"metadata":{"name":"owner","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"IpRestriction","config":{"allowRefs":[{"name":"a"},{"name":"b"}],"denyRefs":[{"name":"a"}]}}]}})).unwrap();
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    prepared
        .visit_acl_plugins_mut(&mut owner, |mut leaf| {
            leaf.set_read_scopes(
                IpListSide::Allow,
                &[IpReadScope::SameNamespace, IpReadScope::SameNamespace],
            )
            .unwrap();
            leaf.set_read_scopes(IpListSide::Deny, &[IpReadScope::SameNamespace])
                .unwrap();
        })
        .unwrap();
    let wire = serde_json::to_value(&owner).unwrap();
    let valid = |value: serde_json::Value, side| {
        let owner: EdgionPlugins = serde_json::from_value(value).unwrap();
        let prepared = PreparedIpListReferences::from_plugins(&owner);
        let binding = prepared.bind_plugins(&owner).unwrap();
        binding.http_entry(0).unwrap().references().read_scopes(side).is_ok()
    };
    assert!(valid(wire.clone(), IpListSide::Allow));
    for mutation in ["side", "order", "source", "domain", "scope"] {
        let mut changed = wire.clone();
        match mutation {
            "side" => {
                changed["spec"]["requestPlugins"][0]["config"]["resolvedDenyRefs"] =
                    changed["spec"]["requestPlugins"][0]["config"]["resolvedAllowRefs"].clone()
            }
            "order" => changed["spec"]["requestPlugins"][0]["config"]["allowRefs"]
                .as_array_mut()
                .unwrap()
                .reverse(),
            "source" => changed["metadata"]["name"] = json!("retargeted"),
            "domain" => {
                changed["spec"]["requestPlugins"][0]["config"]["resolvedAllowRefs"]["domain"] = json!("condition")
            }
            "scope" => {
                changed["spec"]["requestPlugins"][0]["config"]["allowRefs"][0]["namespace"] = json!("other");
                changed["spec"]["requestPlugins"][0]["config"]["resolvedAllowRefs"]["slots"][0]["namespace"] =
                    json!("other");
            }
            _ => unreachable!(),
        }
        assert!(
            !valid(
                changed,
                if mutation == "side" {
                    IpListSide::Deny
                } else {
                    IpListSide::Allow
                }
            ),
            "{mutation}"
        );
    }
    let mut stream:EdgionStreamPlugins=serde_json::from_value(json!({"metadata":{"name":"profiles","namespace":"default"},
        "spec":{"plugins":[{"type":"GlobalConnectionIpRestriction","config":{"enable":true,"activeProfile":"a","profiles":{
            "a":{"allowRefs":[{"name":"shared"}]},"b":{"allowRefs":[{"name":"shared"}]}
        }}}]}})).unwrap();
    let prepared = PreparedIpListReferences::from_stream_plugins(&stream);
    prepared
        .visit_acl_stream_plugins_mut(&mut stream, |mut leaf| {
            leaf.set_read_scopes(IpListSide::Allow, &[IpReadScope::SameNamespace])
                .unwrap()
        })
        .unwrap();
    let mut encoded = serde_json::to_value(&stream).unwrap();
    encoded["spec"]["plugins"][0]["config"]["profiles"]["b"]["resolvedAllowRefs"] =
        encoded["spec"]["plugins"][0]["config"]["profiles"]["a"]["resolvedAllowRefs"].clone();
    let changed: EdgionStreamPlugins = serde_json::from_value(encoded).unwrap();
    let new_prepared = PreparedIpListReferences::from_stream_plugins(&changed);
    let binding = new_prepared.bind_stream_plugins(&changed).unwrap();
    let entry = binding.global_entry(0).unwrap();
    assert!(entry
        .profiles()
        .find(|(name, _)| *name == "b")
        .unwrap()
        .1
        .read_scopes(IpListSide::Allow)
        .is_err());
    let EdgionStreamPlugin::GlobalConnectionIpRestriction(config) =
        &mut stream.spec.plugins.as_mut().unwrap()[0].plugin
    else {
        panic!()
    };
    let renamed = config.profiles.remove("a").unwrap();
    config.profiles.insert("z".into(), renamed);
    config.active_profile = "z".into();
    assert!(prepared.bind_stream_plugins(&stream).is_err());
}

#[test]
fn passive_controller_sites_do_not_confer_executable_authority() {
    let owner:EdgionPlugins=serde_json::from_value(json!({"metadata":{"name":"inactive"},"spec":{
        "requestPlugins":[{"type":"IpRestriction","enable":false,"config":{"allowRefs":[{"name":"request"}]}}],
        "upstreamResponseFilterPlugins":[{"type":"IpRestriction","config":{"allowRefs":[{"name":"response-filter"}]}}],
        "upstreamResponseBodyFilterPlugins":[{"type":"IpRestriction","config":{"allowRefs":[{"name":"response-body"}]}}],
        "upstreamResponsePlugins":[{"type":"IpRestriction","config":{"allowRefs":[{"name":"response"}]}}]
    }})).unwrap();
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    let binding = prepared.bind_plugins(&owner).unwrap();
    let states: Vec<_> = binding
        .references()
        .map(|refs| {
            let name = refs
                .allow()
                .next()
                .unwrap()
                .0
                .validated()
                .unwrap()
                .target()
                .name()
                .to_owned();
            (name, refs.is_controller_reference_site(), refs.is_supported())
        })
        .collect();
    assert_eq!(states, [("request".into(), true, false)]);
    // Constructor loans are request-only; the clearing catalogue still reaches
    // every actual owning field, including unsupported-stage entries.
    assert_eq!(owner.spec.upstream_response_filter_plugins.as_ref().unwrap().len(), 1);
    assert_eq!(
        owner.spec.upstream_response_body_filter_plugins.as_ref().unwrap().len(),
        1
    );
    assert_eq!(owner.spec.upstream_response_plugins.as_ref().unwrap().len(), 1);
    let mut cleared_owner = owner.clone();
    for plugin in cleared_owner
        .spec
        .request_plugins
        .iter_mut()
        .flatten()
        .map(|e| &mut e.plugin)
        .chain(
            cleared_owner
                .spec
                .upstream_response_filter_plugins
                .iter_mut()
                .flatten()
                .map(|e| &mut e.plugin),
        )
        .chain(
            cleared_owner
                .spec
                .upstream_response_body_filter_plugins
                .iter_mut()
                .flatten()
                .map(|e| &mut e.plugin),
        )
        .chain(
            cleared_owner
                .spec
                .upstream_response_plugins
                .iter_mut()
                .flatten()
                .map(|e| &mut e.plugin),
        )
    {
        let EdgionPlugin::IpRestriction(config) = plugin else {
            panic!("expected actual ACL field")
        };
        let reference = &mut config.refs.allow_refs.as_mut().unwrap()[0];
        reference.ref_denied = Some(RefDenied {
            target_namespace: "default".into(),
            target_name: reference.name.clone(),
            reason: None,
        });
    }
    let prepared = PreparedIpListReferences::from_plugins(&cleared_owner);
    prepared
        .visit_acl_plugins_mut(&mut cleared_owner, |mut refs| {
            refs.set_read_scopes(IpListSide::Allow, &[IpReadScope::Denied]).unwrap();
        })
        .unwrap();
    let mut visited = 0;
    prepared
        .visit_acl_plugins_mut(&mut cleared_owner, |mut refs| {
            assert!(!refs.is_supported());
            assert_eq!(
                refs.borrowed().read_scopes(IpListSide::Allow).unwrap(),
                Some(vec![IpReadScope::Denied])
            );
            assert!(refs.references(IpListSide::Allow).next().unwrap().1);
            refs.clear_read_metadata();
            assert!(refs.borrowed().read_scopes(IpListSide::Allow).unwrap().is_none());
            assert!(!refs.references(IpListSide::Allow).next().unwrap().1);
            visited += 1;
        })
        .unwrap();
    assert_eq!(visited, 4);
    let owner:EdgionStreamPlugins=serde_json::from_value(json!({"metadata":{"name":"inactive"},"spec":{
        "plugins":[{"type":"IpRestriction","enable":false,"config":{"allowRefs":[{"name":"connection"}]}},
            {"type":"GlobalConnectionIpRestriction","enable":false,"config":{"enable":true,"activeProfile":"base","profiles":{"base":{"allowRefs":[{"name":"outer-gcir"}]}}}},
            {"type":"GlobalConnectionIpRestriction","config":{"enable":false,"activeProfile":"base","profiles":{"base":{"allowRefs":[{"name":"inner-gcir"}]}}}}],
        "tlsRoutePlugins":[{"type":"IpRestriction","enable":false,"config":{"allowRefs":[{"name":"tls"}]}}]
    }})).unwrap();
    let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
    let binding = prepared.bind_stream_plugins(&owner).unwrap();
    assert_eq!(binding.references().count(), 4);
    for refs in binding.references() {
        assert!(refs.is_controller_reference_site());
        assert!(!refs.is_supported());
    }
}
