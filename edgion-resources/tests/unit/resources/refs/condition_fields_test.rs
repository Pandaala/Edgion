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
        {"name":"direct","type":"directPeerIp","config":{"ipRefs":[{"name":"direct-networks"}],"conditions":set("direct")}},
        {"name":"remote","type":"remoteIp","config":{"ipRefs":[{"name":"remote-networks"}],"conditions":set("remote")}},
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

fn ip_owner() -> EdgionPlugins {
    fn replace(value: &mut serde_json::Value) {
        if value.get("type").and_then(|v| v.as_str()) == Some("secretMatch") {
            *value = serde_json::json!({"type":"remoteIpMatch", "ipRefs":[{"name":"nets"}]});
            return;
        }
        match value {
            serde_json::Value::Object(fields) => {
                for child in fields.values_mut() {
                    replace(child);
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    replace(child);
                }
            }
            _ => {}
        }
    }
    let mut wire = serde_json::to_value(root_fixture()).unwrap();
    replace(&mut wire);
    let mut owner: EdgionPlugins = serde_json::from_value(wire).unwrap();
    if let EdgionPlugin::RequestAccessPolicy(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[2].plugin {
        for profile in config.resolved_profiles.as_mut().unwrap().values_mut() {
            for rule in &mut profile.required_rule_groups[0].any_of_rules {
                if let RequestAccessRuleType::DirectPeerIp(ip) | RequestAccessRuleType::RemoteIp(ip) = &mut rule.rule {
                    ip.refs.ip_refs = vec![serde_json::from_value(serde_json::json!({"name":"direct-nets"})).unwrap()];
                }
            }
        }
    }
    owner
}

#[test]
fn ip_catalogue_covers_actual_roots_and_retains_enabled_only_activity() {
    let mut owner = ip_owner();
    owner.metadata.namespace = None;
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    let mut ordinary = 0;
    let mut rap = 0;
    prepared
        .visit_ip(&owner, |leaf| {
            match leaf.consumer() {
                IpReferenceConsumer::Ordinary => ordinary += 1,
                IpReferenceConsumer::RequestAccessPolicy => rap += 1,
                IpReferenceConsumer::RealIp => panic!("unexpected RealIp root in ordinary fixture"),
            }
            for token in leaf.references().unwrap() {
                let bound = token.validated().unwrap();
                assert_eq!(bound.target().scope().namespace(), Some("default"));
                assert_eq!(bound.source().group_kind().kind(), "EdgionPlugins");
                assert_eq!(bound.target().group_kind().kind(), "EdgionConfigData");
                assert!(matches!(token.freeze(), Err(ReferenceError::UnsupportedTarget)));
            }
        })
        .unwrap();
    assert_eq!((ordinary, rap), (29, 10));
    // Wasm is unsupported in this stage, but existing IP resolution is enabled-only.
    owner.spec.upstream_response_body_filter_plugins.as_mut().unwrap()[0].enable = false;
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    let mut count = 0;
    prepared.visit_ip(&owner, |_| count += 1).unwrap();
    assert_eq!(count, 33);
}

#[test]
fn ip_descriptors_validate_before_access_and_resolved_payload_does_not_change_binding() {
    for (name, namespace, valid) in [
        ("nets", None, true),
        ("nets", Some("ops"), true),
        ("", None, false),
        ("bad/name", None, false),
        ("nets", Some(""), false),
    ] {
        let mut owner = ip_owner();
        let prepared = PreparedConditionReferences::from_plugins(&owner);
        prepared
            .visit_ip_mut(
                &mut owner,
                |mut leaf, _emit| {
                    leaf.config_mut().resolved_refs = None;
                    for wire in &mut leaf.config_mut().ip_refs {
                        wire.ref_denied = None;
                    }
                    assert!(leaf.references().is_ok());
                },
                |_, _| {},
            )
            .unwrap();
        let prepared = PreparedConditionReferences::from_plugins(&owner);
        let mut first = true;
        let result = prepared.visit_ip_mut(
            &mut owner,
            |mut leaf, _emit| {
                if first {
                    first = false;
                    leaf.config_mut().ip_refs[0].name = name.into();
                    leaf.config_mut().ip_refs[0].namespace = namespace.map(str::to_string);
                }
            },
            |_, _| {},
        );
        if name != "nets" || namespace.is_some() {
            assert!(matches!(result, Err(ReferenceError::StaleBinding)));
        }
        let prepared = PreparedConditionReferences::from_plugins(&owner);
        let mut first = true;
        prepared
            .visit_ip(&owner, |leaf| {
                if first {
                    first = false;
                    assert_eq!(leaf.references().unwrap().next().unwrap().validated().is_ok(), valid);
                }
            })
            .unwrap();
    }
}

#[test]
fn rap_warning_application_rechecks_owner_and_preserves_root_order() {
    let mut owner = ip_owner();
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    let mut order = Vec::new();
    prepared
        .visit_ip_mut(
            &mut owner,
            |leaf, emit| {
                if leaf.consumer() == IpReferenceConsumer::RequestAccessPolicy {
                    let name = leaf.references().unwrap().next().unwrap().authored_name().to_string();
                    order.push(name.clone());
                    emit(name);
                }
            },
            |warnings, warning| warnings.push(warning),
        )
        .unwrap();
    assert_eq!(order, ["direct-nets", "nets", "direct-nets", "nets", "nets"].repeat(2));
    let EdgionPlugin::RequestAccessPolicy(config) = &owner.spec.request_plugins.as_ref().unwrap()[2].plugin else {
        unreachable!()
    };
    assert_eq!(config.resolution_warnings, order);
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    let result = prepared.visit_ip_mut(
        &mut owner,
        |mut leaf, emit| {
            if leaf.consumer() == IpReferenceConsumer::RequestAccessPolicy {
                leaf.config_mut().ip_refs.clear();
            }
            emit("must not append".into());
        },
        |warnings, warning| warnings.push(warning),
    );
    assert!(matches!(result, Err(ReferenceError::StaleBinding)));
    let EdgionPlugin::RequestAccessPolicy(config) = &owner.spec.request_plugins.as_ref().unwrap()[2].plugin else {
        unreachable!()
    };
    assert_eq!(config.resolution_warnings, order);
}

#[test]
fn inactive_effective_ip_nodes_clear_seeded_scope_and_negative_markers_without_loans() {
    let mut owner = ip_owner();
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    prepared
        .visit_ip_mut(
            &mut owner,
            |mut leaf, _| {
                let scopes: Vec<_> = leaf
                    .config()
                    .ip_refs
                    .iter()
                    .map(|_| crate::resources::edgion_plugins::ip_reference_set::IpReadScope::Denied)
                    .collect();
                leaf.set_read_scopes(&scopes).unwrap();
            },
            |_, _| {},
        )
        .unwrap();
    if let Some(entries) = &mut owner.spec.request_plugins {
        for entry in entries {
            entry.enable = false;
        }
    }
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    prepared.clear_ip_read_scopes(&mut owner).unwrap();
    prepared
        .visit(&owner, |leaf| {
            if let Condition::DirectPeerIpMatch(ip) | Condition::RemoteIpMatch(ip) = leaf.config() {
                assert!(ip.refs.resolved_refs.is_none());
                assert!(ip.refs.ip_refs.iter().all(|reference| reference.ref_denied.is_none()));
            }
        })
        .unwrap();
    let mut active_count = 0;
    prepared.visit_ip(&owner, |_| active_count += 1).unwrap();
    // Other stage arrays in this fixture remain enabled and keep the original catalogue semantics.
    assert!(active_count > 0);
}

#[test]
fn unsupported_stage_rap_clears_seeded_primary_proof_without_becoming_a_consumer() {
    let mut owner = ip_owner();
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    prepared
        .visit_ip_mut(
            &mut owner,
            |mut leaf, _| {
                let scopes = vec![
                    crate::resources::edgion_plugins::ip_reference_set::IpReadScope::Denied;
                    leaf.config().ip_refs.len()
                ];
                leaf.set_read_scopes(&scopes).unwrap();
            },
            |_, _| {},
        )
        .unwrap();
    let entry = owner.spec.request_plugins.as_mut().unwrap().remove(2);
    let entry = serde_json::from_value(serde_json::to_value(entry).unwrap()).unwrap();
    owner
        .spec
        .upstream_response_body_filter_plugins
        .get_or_insert_default()
        .push(entry);
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    prepared.clear_ip_read_scopes(&mut owner).unwrap();
    let entry = owner
        .spec
        .upstream_response_body_filter_plugins
        .as_ref()
        .unwrap()
        .last()
        .unwrap();
    let EdgionPlugin::RequestAccessPolicy(config) = &entry.plugin else {
        unreachable!()
    };
    for profile in config.resolved_profiles.as_ref().unwrap().values() {
        for group in &profile.required_rule_groups {
            for rule in &group.any_of_rules {
                if let crate::resources::edgion_plugins::plugin_configs::RequestAccessRuleType::DirectPeerIp(ip)
                | crate::resources::edgion_plugins::plugin_configs::RequestAccessRuleType::RemoteIp(ip) = &rule.rule
                {
                    assert!(ip.refs.resolved_refs.is_none());
                    assert!(ip.refs.ip_refs.iter().all(|reference| reference.ref_denied.is_none()));
                }
            }
        }
    }
}

#[test]
fn global_real_ip_loans_have_actual_cluster_authority_and_finite_scopes() {
    use crate::EdgionGatewayConfig;
    let make = || {
        serde_json::from_value::<EdgionGatewayConfig>(serde_json::json!({
            "metadata": {"name": "global", "uid": "egc-uid", "generation": 1},
            "spec": {"realIp": {"trustedIpRefs": [{"name": "proxies", "namespace": "ops"}]}}
        }))
        .unwrap()
    };
    for scope in [
        IpReadScope::SameNamespace,
        IpReadScope::GrantAuthorized,
        IpReadScope::ClusterOnly,
        IpReadScope::ValidationDisabled,
        IpReadScope::Denied,
    ] {
        let mut owner = make();
        let prepared = PreparedConditionReferences::from_gateway_config(&owner);
        prepared
            .visit_gateway_ip(&owner, |leaf| {
                assert_eq!(leaf.execution_role(), IpExecutionRole::Trust);
                assert_eq!(leaf.consumer(), IpReferenceConsumer::RealIp);
                let token = leaf.references().unwrap().next().unwrap();
                let bound = token.validated().unwrap();
                assert_eq!(bound.source().group_kind().kind(), "EdgionGatewayConfig");
                assert_eq!(bound.source().scope().namespace(), None);
                assert_eq!(bound.target().scope().namespace(), Some("ops"));
            })
            .unwrap();
        prepared
            .visit_gateway_ip_mut(&mut owner, |mut leaf| {
                assert_eq!(
                    leaf.set_read_scopes(&[scope]).is_ok(),
                    !matches!(scope, IpReadScope::SameNamespace | IpReadScope::GrantAuthorized)
                );
            })
            .unwrap();
        prepared.clear_gateway_ip_scopes(&mut owner).unwrap();
        assert!(owner.spec.real_ip.as_ref().unwrap().trusted.resolved_refs.is_none());
    }
    let mut owner = make();
    let prepared = PreparedConditionReferences::from_gateway_config(&owner);
    owner.metadata.name = Some("different-owner".into());
    assert!(prepared.visit_gateway_ip(&owner, |_| panic!("stale callback")).is_err());
    let mut owner = make();
    owner.spec.real_ip.as_mut().unwrap().trusted.ip_refs[0].namespace = None;
    PreparedConditionReferences::from_gateway_config(&owner)
        .visit_gateway_ip(&owner, |leaf| {
            assert!(leaf.references().unwrap().next().unwrap().validated().is_err());
        })
        .unwrap();
}

#[test]
fn prepared_ip_comparison_preserves_exact_authored_fields_and_wire_variant() {
    let resource = owner();
    let snapshot = OwnerSnapshot::namespaced("edgion.io", "EdgionPlugins", &resource.metadata);
    let ip: IpReferenceSet = serde_json::from_value(serde_json::json!({
        "ipRefs": [{"name": "first"}, {"name": "second", "namespace": "ops"}]
    }))
    .unwrap();
    let mut prepared = PreparedIp::new(&snapshot, 0, None, &ip);
    assert!(prepared.matches(&ip));
    for mutation in [
        "name",
        "namespace-empty",
        "namespace-explicit",
        "namespace-omitted",
        "order",
        "length",
    ] {
        let mut changed = ip.clone();
        match mutation {
            "name" => changed.ip_refs[0].name = "other".into(),
            "namespace-empty" => changed.ip_refs[0].namespace = Some(String::new()),
            "namespace-explicit" => changed.ip_refs[0].namespace = Some("apps".into()),
            "namespace-omitted" => changed.ip_refs[1].namespace = None,
            "order" => changed.ip_refs.swap(0, 1),
            _ => {
                changed.ip_refs.pop();
            }
        }
        assert!(!prepared.matches(&changed), "{mutation}");
        assert!(PreparedIp::new(&snapshot, 0, None, &changed).matches(&changed));
    }
    prepared.slots[0].wire = FieldWire::Descriptor {
        name: "first".into(),
        key: String::new(),
    };
    assert!(!prepared.matches(&ip));
}
