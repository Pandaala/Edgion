use super::*;

#[test]
fn failover_request_rejects_unknown_fields() {
    // Any unknown field must be rejected to keep the contract
    // "failover endpoint can only change failoverTo".
    let json = r#"{
            "regionName": "east",
            "failoverTo": "west",
            "myRegion": "evil"
        }"#;
    let result: Result<SettingFailoverRequest, _> = serde_json::from_str(json);
    assert!(result.is_err(), "unknown field 'myRegion' must be rejected");
}

#[test]
fn failover_request_accepts_canonical_payload() {
    let json = r#"{
            "regionName": "east",
            "failoverTo": "west"
        }"#;
    let result: Result<SettingFailoverRequest, _> = serde_json::from_str(json);
    let req = result.expect("canonical target payload must be accepted");
    assert_eq!(req.region_name, "east");
    assert_eq!(req.failover_to, "west");
}

#[test]
fn failover_request_accepts_empty_failover_to() {
    let json = r#"{
            "regionName": "east",
            "failoverTo": ""
        }"#;
    let result: Result<SettingFailoverRequest, _> = serde_json::from_str(json);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().failover_to, "");
}

#[test]
fn patch_region_failover_only_changes_target_failover_to() {
    let original = serde_json::json!([
        {
            "name": "east",
            "weight": 100,
            "endpoints": ["10.0.0.1", "10.0.0.2"],
            "failoverTo": "old-target"
        },
        {
            "name": "west",
            "weight": 50,
            "endpoints": ["10.0.1.1"],
            "failoverTo": "another"
        }
    ]);
    let mut regions = original.as_array().unwrap().clone();

    let modified = patch_region_failover(&mut regions, "east", "west");
    assert_eq!(
        modified,
        RegionPatch {
            matched: true,
            changed: true
        }
    );

    assert_eq!(regions[0]["failoverTo"], serde_json::Value::String("west".into()));
    assert_eq!(regions[0]["name"], original[0]["name"]);
    assert_eq!(regions[0]["weight"], original[0]["weight"]);
    assert_eq!(regions[0]["endpoints"], original[0]["endpoints"]);
    assert_eq!(regions[1], original[1]);
}

#[test]
fn patch_region_failover_clear_removes_field_only() {
    let original = serde_json::json!([
        {
            "name": "east",
            "weight": 100,
            "failoverTo": "west"
        },
        {
            "name": "west",
            "weight": 50,
            "failoverTo": "east"
        }
    ]);
    let mut regions = original.as_array().unwrap().clone();

    let modified = patch_region_failover(&mut regions, "east", "");
    assert_eq!(
        modified,
        RegionPatch {
            matched: true,
            changed: true
        }
    );

    assert!(regions[0].as_object().unwrap().get("failoverTo").is_none());
    assert_eq!(regions[0]["name"], original[0]["name"]);
    assert_eq!(regions[0]["weight"], original[0]["weight"]);
    assert_eq!(regions[1], original[1]);
}

#[test]
fn patch_region_failover_no_match_returns_false() {
    let mut regions = vec![serde_json::json!({"name": "east", "failoverTo": "x"})];
    let modified = patch_region_failover(&mut regions, "nonexistent", "y");
    assert_eq!(
        modified,
        RegionPatch {
            matched: false,
            changed: false
        }
    );
    assert_eq!(regions[0]["failoverTo"], serde_json::Value::String("x".into()));
}

#[test]
fn patch_region_failover_reports_already_applied_without_mutation() {
    let mut regions = vec![serde_json::json!({"name": "east", "failoverTo": "west"})];
    let before = regions.clone();
    assert_eq!(
        patch_region_failover(&mut regions, "east", "west"),
        RegionPatch {
            matched: true,
            changed: false
        }
    );
    assert_eq!(regions, before);

    let mut clear = vec![serde_json::json!({"name": "east"})];
    assert_eq!(
        patch_region_failover(&mut clear, "east", ""),
        RegionPatch {
            matched: true,
            changed: false
        }
    );
}

#[test]
fn route_usage_discovers_rule_and_backend_extension_refs() {
    let http_routes = vec![serde_json::json!({
        "metadata": {"namespace": "shop", "name": "checkout"},
        "spec": {"rules": [
            {
                "filters": [{"type": "ExtensionRef", "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "region"
                }}],
                "backendRefs": [{"name": "checkout-api", "port": 8080}]
            },
            {
                "backendRefs": [{
                    "name": "payments", "port": 9090,
                    "filters": [{"type": "ExtensionRef", "extensionRef": {
                        "group": "edgion.io", "kind": "EdgionPlugins", "name": "region"
                    }}]
                }]
            }
        ]}
    })];
    let usages = collect_route_usages("shop", "region", &http_routes, &[], &mut ProjectionBudget::new()).unwrap();
    assert_eq!(usages.len(), 2);
    assert_eq!(usages[0]["routeKind"], "HTTPRoute");
    assert_eq!(usages[0]["backendServices"][0]["name"], "checkout-api");
    assert_eq!(usages[1]["ruleIndex"], 1);
    assert_eq!(usages[1]["backendServices"][0]["name"], "payments");
}

#[test]
fn route_usage_rejects_other_namespaces_and_reference_kinds() {
    let routes = vec![serde_json::json!({
        "metadata": {"namespace": "other", "name": "ignored"},
        "spec": {"rules": [{
            "filters": [{"type": "ExtensionRef", "extensionRef": {
                "group": "example.io", "kind": "EdgionPlugins", "name": "region"
            }}]
        }]}
    })];
    assert!(
        collect_route_usages("shop", "region", &routes, &[], &mut ProjectionBudget::new())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn route_usage_reads_authored_rules_only() {
    let routes = vec![serde_json::json!({
        "metadata": {"namespace": "shop", "name": "parent"},
        "spec": {
            "rules": [{
                "filters": [{"type": "ExtensionRef", "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "region"
                }}],
                "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "child"}]
            }],
            // Injected legacy flattening output is not a field and is ignored.
            "resolvedRules": [{
                "filters": [{"type": "ExtensionRef", "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "region"
                }}],
                "backendRefs": [{"name": "child-service", "port": 8080}]
            }]
        }
    })];
    let usages = collect_route_usages("shop", "region", &routes, &[], &mut ProjectionBudget::new()).unwrap();
    assert_eq!(usages.len(), 1);
    assert_eq!(usages[0]["routeName"], "parent");
    assert!(!usages[0].to_string().contains("child-service"));
}

#[test]
fn setting_lookup_preserves_exact_source_and_consumer_validation() {
    let config: RegionRouteConfig = serde_json::from_value(serde_json::json!({
        "clusterRegionRef":{"namespace":"operations","name":"local"}
    }))
    .unwrap();
    let resource = serde_json::json!({
        "metadata":{"namespace":"operations","name":"local"},
        "spec":{"data":{"type":"ClusterRegionSetting","config":{
            "clusterName":"prod","regionName":"east","regions":[
                {"name":"east","hashRange":[0,999],"backendEndpoint":"east:80"}
            ]
        }}}
    });
    let setting = read_region_setting(&config, "shop", std::slice::from_ref(&resource)).unwrap();
    assert_eq!(setting.region_name, "east");
    let proof = setting.validation_proof().unwrap();
    assert!(proof.accepts_modulo(1000));
    assert!(!proof.accepts_modulo(100));
    assert!(read_region_setting(&config, "shop", &[]).is_none());
    for (pointer, value) in [
        ("/spec/enable", serde_json::json!(false)),
        ("/spec/data/type", serde_json::json!("Misc")),
        ("/metadata/name", serde_json::json!("other")),
    ] {
        let mut changed = resource.clone();
        if pointer == "/spec/enable" {
            changed["spec"]["enable"] = value;
        } else {
            *changed.pointer_mut(pointer).unwrap() = value;
        }
        assert!(read_region_setting(&config, "shop", &[changed]).is_none());
    }
    let mut invalid = resource;
    invalid["spec"]["data"]["config"]["regionName"] = serde_json::json!("missing");
    assert!(read_region_setting(&config, "shop", &[invalid])
        .unwrap()
        .validation_proof()
        .is_err());
}

#[test]
fn fresh_resource_version_skips_an_external_version_equal_to_next_allocation() {
    let mut next = 42;
    let version = distinct_resource_version(Some("42"), || {
        let value = next;
        next += 1;
        value
    });
    assert_eq!(version, "43");
    assert_eq!(next, 44);
    assert_eq!(distinct_resource_version(None, || 7), "7");
}

#[test]
fn usage_projection_shares_nested_row_budget_and_rejects_large_identity() {
    let backends = vec![serde_json::json!({"name":"backend","port":80}); 1001];
    assert_eq!(
        route_usage(
            "HTTPRoute",
            "ns",
            "route",
            0,
            backends.iter(),
            &mut ProjectionBudget::new()
        ),
        Err(StatusCode::PAYLOAD_TOO_LARGE)
    );
    let name = "x".repeat(8 * 1024 * 1024);
    assert_eq!(
        route_usage("HTTPRoute", "ns", &name, 0, [].iter(), &mut ProjectionBudget::new()),
        Err(StatusCode::PAYLOAD_TOO_LARGE)
    );
    // A second parent shares the same ledger rather than resetting child budgets.
    let mut budget = ProjectionBudget::new();
    let backends = vec![serde_json::json!({"name":"backend"}); 600];
    route_usage("HTTPRoute", "ns", "first", 0, backends.iter(), &mut budget).unwrap();
    assert_eq!(
        route_usage("HTTPRoute", "ns", "second", 1, backends.iter(), &mut budget),
        Err(StatusCode::PAYLOAD_TOO_LARGE)
    );
}
