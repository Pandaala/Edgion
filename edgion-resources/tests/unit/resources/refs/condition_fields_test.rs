use super::*;

fn leaf(name: &str) -> serde_json::Value {
    serde_json::json!({"type":"secretMatch","key":{"type":"header","name":"X-Token"},
        "secretKey":"token","secretRef":{"name":name},"resolvedValues":["forged"]})
}
fn owner() -> EdgionPlugins {
    serde_json::from_value(serde_json::json!({
        "metadata":{"name":"plugins","namespace":"apps","uid":"owner-uid","generation":1},
        "spec":{"requestPlugins":[{"type":"Wasm","config":{"source":{"url":"https://repo.example/module.wasm"}},
            "conditions":{"run":{"anyOf":[leaf("first"),{"name":"nested","allOf":[leaf("second"),{
                "type":"hmacAuth","secretGroups":[{"name":"group","secretRefs":[{"group":"core","name":"hmac","namespace":"ops"}]}]
            }]}]}}}]}
    })).unwrap()
}

#[test]
fn condition_loans_preserve_structure_source_defaults_and_reject_freezing() {
    let owner = owner();
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    let mut identities = Vec::new();
    prepared
        .visit(&owner, |leaf| {
            let references = leaf.references().unwrap();
            let mut check = |token: ReferenceToken<'_>| {
                let reference = token.validated().unwrap();
                assert_eq!(reference.source().group_kind().kind(), "EdgionPlugins");
                assert_eq!(reference.source().scope().namespace(), Some("apps"));
                assert_eq!(reference.target().group_kind().group(), "");
                identities.push((
                    reference.target().scope().namespace().unwrap().to_string(),
                    reference.target().name().to_string(),
                ));
                assert!(matches!(token.freeze(), Err(ReferenceError::UnsupportedTarget)));
            };
            if let Some(token) = references.secret_match_secret() {
                check(token);
            }
            for group in references.hmac_secret_groups() {
                assert_eq!(group.name(), "group");
                for token in group.references() {
                    check(token);
                }
            }
        })
        .unwrap();
    assert_eq!(
        identities,
        [
            ("apps".into(), "first".into()),
            ("apps".into(), "second".into()),
            ("ops".into(), "hmac".into())
        ]
    );
    let set = prepared
        .snapshot
        .roots
        .iter()
        .find_map(|root| root.set.as_ref())
        .unwrap();
    assert_eq!(
        set.leaves.iter().map(|leaf| leaf.position).collect::<Vec<_>>(),
        [
            LeafPosition::AnyOf(0),
            LeafPosition::AnyClauseAll { clause: 1, leaf: 0 },
            LeafPosition::AnyClauseAll { clause: 1, leaf: 1 }
        ]
    );
}

#[test]
fn owner_activity_container_and_reference_mutations_invalidate_binding() {
    let original = owner();
    let prepared = PreparedConditionReferences::from_plugins(&original);
    for changed in ["uid", "enable", "container", "shape", "key", "group", "name"] {
        let mut wire = serde_json::to_value(&original).unwrap();
        match changed {
            "uid" => wire["metadata"]["uid"] = serde_json::json!("replacement"),
            "enable" => wire["spec"]["requestPlugins"][0]["enable"] = serde_json::json!(false),
            "container" => wire["spec"]["requestPlugins"][0]["conditions"]["skip"] = serde_json::json!({"allOf":[]}),
            "shape" => {
                wire["spec"]["requestPlugins"][0]["conditions"]["run"] = serde_json::json!({"allOf":[leaf("first")]})
            }
            "key" => {
                wire["spec"]["requestPlugins"][0]["conditions"]["run"]["anyOf"][0]["secretKey"] =
                    serde_json::json!("other")
            }
            "group" => {
                wire["spec"]["requestPlugins"][0]["conditions"]["run"]["anyOf"][0]["secretRef"]["group"] =
                    serde_json::json!("foreign.example")
            }
            "name" => {
                wire["spec"]["requestPlugins"][0]["conditions"]["run"]["anyOf"][0]["secretRef"]["name"] =
                    serde_json::json!("other")
            }
            _ => unreachable!(),
        }
        let mut candidate: EdgionPlugins = serde_json::from_value(wire).unwrap();
        assert!(
            matches!(
                prepared.visit(&candidate, |_| panic!("stale callback")),
                Err(ReferenceError::StaleBinding)
            ),
            "{changed}"
        );
        assert!(
            matches!(
                prepared.visit_mut(&mut candidate, |_| panic!("stale callback")),
                Err(ReferenceError::StaleBinding)
            ),
            "{changed}"
        );
    }
}

#[test]
fn resolved_material_mutation_keeps_loans_but_authored_ref_mutation_rejects_reloan() {
    let mut owner = owner();
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    let mut first = true;
    prepared
        .visit_mut(&mut owner, |mut leaf| {
            if let Condition::SecretMatch(config) = leaf.config_mut() {
                config.resolved_values = None;
                assert!(leaf.references().is_ok());
                if first {
                    first = false;
                    if let Condition::SecretMatch(config) = leaf.config_mut() {
                        config.secret_ref.name = "changed".into();
                    }
                    assert!(matches!(leaf.references(), Err(ReferenceError::StaleBinding)));
                }
            }
        })
        .unwrap();
    assert!(matches!(
        prepared.visit(&owner, |_| {}),
        Err(ReferenceError::StaleBinding)
    ));
}

#[test]
fn disabled_leaf_material_can_be_cleared_without_tokens() {
    let mut owner = owner();
    owner.spec.request_plugins.as_mut().unwrap()[0].enable = false;
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    let mut visited = 0;
    prepared
        .visit_mut(&mut owner, |mut leaf| {
            visited += 1;
            assert!(!leaf.is_active());
            {
                let references = leaf.references().unwrap();
                assert!(references.secret_match_secret().is_none());
                assert_eq!(references.hmac_secret_groups().count(), 0);
            }
            match leaf.config_mut() {
                Condition::SecretMatch(config) => config.resolved_values = None,
                Condition::HmacAuth(config) => config.resolved_credentials = None,
                _ => {}
            }
        })
        .unwrap();
    assert_eq!(visited, 3);
    prepared
        .visit(&owner, |leaf| {
            if let Condition::SecretMatch(config) = leaf.config() {
                assert!(config.resolved_values.is_none());
            }
        })
        .unwrap();
}

fn root_fixture() -> EdgionPlugins {
    let set = |name: &str| serde_json::json!({"allOf":[leaf(name)]});
    let gates = |name: &str| serde_json::json!({"skip":set(&format!("{name}-skip")),"run":set(&format!("{name}-run"))});
    let profile = serde_json::json!({"requiredRuleGroups":[{"name":"gate","anyOfRules":[
        {"name":"direct","type":"directPeerIp","config":{"ips":["192.0.2.1"],"conditions":set("direct")}},
        {"name":"remote","type":"remoteIp","config":{"ips":["192.0.2.2"],"conditions":set("remote")}},
        {"name":"url","type":"url","config":{"hosts":["example.com"],"paths":[{"type":"Exact","value":"/"}],
            "conditions":set("authored-url"),"resolvedCandidates":[{"name":"imported","paths":[{"type":"Exact","value":"/"}],"conditions":set("effective-url")}]}}
    ]}]});
    let entries = serde_json::json!([
        {"type":"Wasm","config":{"source":{"url":"https://repo.example/module.wasm"}},"conditions":gates("entry"),
            "body":{"conditions":gates("body")},"dye":{"request":[{"name":"X-Request","conditions":gates("request-dye")}],
                "response":[{"name":"X-Response","conditions":gates("response-dye")}] }},
        {"type":"RequestRestriction","config":{"mode":"Deny","conditions":set("restriction")}},
        {"type":"RequestAccessPolicy","config":{"defaultProfile":"a","profiles":{"a":profile},"resolvedProfiles":{"z":profile,"a":profile}}}
    ]);
    let mut wire = serde_json::json!({"metadata":{"name":"plugins","namespace":"apps","uid":"uid"},"spec":{}});
    for stage in [
        "requestPlugins",
        "upstreamResponseFilterPlugins",
        "upstreamResponseBodyFilterPlugins",
        "upstreamResponsePlugins",
    ] {
        wire["spec"][stage] = entries.clone();
    }
    serde_json::from_value(wire).unwrap()
}

#[test]
fn all_roots_and_wrong_stage_inner_conditions_are_reached_for_clearing() {
    let mut owner = root_fixture();
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    let mut total = 0;
    let mut active = 0;
    prepared
        .visit_mut(&mut owner, |mut leaf| {
            total += 1;
            active += usize::from(leaf.is_active());
            let references = leaf.references().unwrap();
            assert_eq!(references.secret_match_secret().is_some(), leaf.is_active());
            if let Condition::SecretMatch(config) = leaf.config_mut() {
                config.resolved_values = None;
            }
        })
        .unwrap();
    // Entry + both dye directions at every stage, body at the two body-owning
    // stages, restriction + three effective RAP leaves for each profile.
    assert_eq!(total, 4 * (6 + 1 + 2 * 3) + 2 * 2);
    assert_eq!(active, 3 * 6 + 2 + 1 + 2 * 3);
    prepared
        .visit(&owner, |leaf| {
            if let Condition::SecretMatch(config) = leaf.config() {
                assert!(config.resolved_values.is_none());
            }
        })
        .unwrap();
}

#[test]
fn rap_map_order_is_stable_but_profile_names_rule_branches_and_candidates_are_bound() {
    let owner = root_fixture();
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    let mut reordered = owner.clone();
    let EdgionPlugin::RequestAccessPolicy(config) = &mut reordered.spec.request_plugins.as_mut().unwrap()[2].plugin
    else {
        unreachable!()
    };
    let mut profiles = config.resolved_profiles.take().unwrap();
    config.resolved_profiles = Some(
        [
            ("z".to_string(), profiles.remove("z").unwrap()),
            ("a".to_string(), profiles.remove("a").unwrap()),
        ]
        .into_iter()
        .collect(),
    );
    prepared.visit(&reordered, |_| {}).unwrap();
    for change in ["profile", "branch", "candidate", "group", "rule"] {
        let mut changed = owner.clone();
        let EdgionPlugin::RequestAccessPolicy(config) = &mut changed.spec.request_plugins.as_mut().unwrap()[2].plugin
        else {
            unreachable!()
        };
        let profiles = config.resolved_profiles.as_mut().unwrap();
        if change == "profile" {
            let profile = profiles.remove("a").unwrap();
            profiles.insert("replacement".into(), profile);
        } else {
            let group = &mut profiles.get_mut("a").unwrap().required_rule_groups[0];
            match change {
                "group" => group.name = "changed".into(),
                "rule" => group.any_of_rules[0].name = "changed".into(),
                "branch" => {
                    let RequestAccessRuleType::DirectPeerIp(ip) = &group.any_of_rules[0].rule else {
                        unreachable!()
                    };
                    group.any_of_rules[0].rule = RequestAccessRuleType::RemoteIp(ip.clone());
                }
                "candidate" => {
                    let RequestAccessRuleType::Url(url) = &mut group.any_of_rules[2].rule else {
                        unreachable!()
                    };
                    url.resolved_candidates.as_mut().unwrap()[0].name = "changed".into();
                }
                _ => unreachable!(),
            }
        }
        assert!(
            matches!(
                prepared.visit(&changed, |_| panic!("stale callback")),
                Err(ReferenceError::StaleBinding)
            ),
            "{change}"
        );
    }
}

#[test]
fn effective_rap_expansion_does_not_invalidate_raw_frozen_credentials() {
    use crate::resources::refs::plugin_fields::PreparedPluginReferences;
    let mut owner = root_fixture();
    owner.spec.request_plugins.as_mut().unwrap().push(
        serde_json::from_value(serde_json::json!({"type":"Csrf","config":{"secretRef":{"name":"key"}}})).unwrap(),
    );
    let frozen = {
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().last().unwrap();
        let refs = entry.references().unwrap();
        refs.csrf_secret().unwrap().freeze().unwrap()
    };
    let EdgionPlugin::RequestAccessPolicy(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[2].plugin else {
        unreachable!()
    };
    config.resolved_profiles = None;
    assert!(frozen.matches_plugins(&owner));
}
