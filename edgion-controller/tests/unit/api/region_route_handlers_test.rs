use super::*;

#[test]
fn failover_request_rejects_unknown_fields() {
    // Any unknown field must be rejected to keep the contract
    // "failover endpoint can only change failoverTo".
    let json = r#"{
            "namespace": "default",
            "name": "test-override-route",
            "regionName": "east",
            "failoverTo": "west",
            "myRegion": "evil"
        }"#;
    let result: Result<EdgionConfigDataFailoverRequest, _> = serde_json::from_str(json);
    assert!(result.is_err(), "unknown field 'myRegion' must be rejected");
}

#[test]
fn failover_request_accepts_canonical_payload() {
    let json = r#"{
            "namespace": "default",
            "name": "test-override-route",
            "regionName": "east",
            "failoverTo": "west"
        }"#;
    let result: Result<EdgionConfigDataFailoverRequest, _> = serde_json::from_str(json);
    let req = result.expect("canonical 4-field payload must be accepted");
    assert_eq!(req.namespace, "default");
    assert_eq!(req.name, "test-override-route");
    assert_eq!(req.region_name, "east");
    assert_eq!(req.failover_to, "west");
}

#[test]
fn failover_request_accepts_empty_failover_to() {
    let json = r#"{
            "namespace": "default",
            "name": "test-override-route",
            "regionName": "east",
            "failoverTo": ""
        }"#;
    let result: Result<EdgionConfigDataFailoverRequest, _> = serde_json::from_str(json);
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
    let usages = collect_route_usages("shop", "region", &http_routes, &[]);
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
    assert!(collect_route_usages("shop", "region", &routes, &[]).is_empty());
}

#[test]
fn route_usage_uses_delegation_resolved_rules() {
    let routes = vec![serde_json::json!({
        "metadata": {"namespace": "shop", "name": "parent"},
        "spec": {
            "rules": [{
                "filters": [{"type": "ExtensionRef", "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "region"
                }}],
                "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "child"}]
            }],
            "resolvedRules": [{
                "filters": [{"type": "ExtensionRef", "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "region"
                }}],
                "backendRefs": [{"name": "child-service", "port": 8080}]
            }]
        }
    })];
    let usages = collect_route_usages("shop", "region", &routes, &[]);
    assert_eq!(usages.len(), 1);
    assert_eq!(usages[0]["backendServices"][0]["name"], "child-service");
}

#[test]
fn override_ref_keeps_namespace_and_applies_effective_regions() {
    let reference = parse_override_ref(
        &serde_json::json!({"namespace": "operations", "name": "region-overlay"}),
        "shop",
    )
    .expect("structured reference");
    assert_eq!(reference["namespace"], "operations");
    let base = serde_json::json!([{
        "name": "base", "hashRange": [0, 999], "backendEndpoint": "base.example.test"
    }]);
    let config = RegionRouteConfig {
        regions: serde_json::from_value(base.clone()).unwrap(),
        ..Default::default()
    };
    let resources = vec![serde_json::json!({
        "metadata": {"namespace": "operations", "name": "region-overlay"},
        "spec": {"enable": true, "data": {"type": "RegionRouteOverride", "config": {
            "regions": [{
                "name": "effective",
                "hashRange": [0, 499],
                "backendEndpoint": "effective.example.test:443",
                "tls": true,
                "failoverTo": "backup"
            }, {
                "name": "backup",
                "hashRange": [500, 999],
                "backendEndpoint": "backup.example.test"
            }]
        }}}
    })];
    let (regions, applied) = apply_region_override(&config, &base, Some(&reference), &resources);
    assert!(applied);
    assert_eq!(regions[0]["name"], "effective");
    assert_eq!(regions[0]["failoverTo"], "backup");
}

#[test]
fn invalid_region_override_falls_back_to_base() {
    let reference = serde_json::json!({
        "namespace": "operations",
        "name": "region-overlay",
        "permitted": true
    });
    let base = serde_json::json!([{
        "name": "base",
        "hashRange": [0, 999],
        "backendEndpoint": "base.example.test:443",
        "tls": true
    }]);
    let resources = vec![serde_json::json!({
        "metadata": {"namespace": "operations", "name": "region-overlay"},
        "spec": {"enable": true, "data": {"type": "RegionRouteOverride", "config": {
            "regions": [{
                "name": "broken",
                "hashRange": [999, 0],
                "backendEndpoint": "broken.example.test:443",
                "tls": true
            }]
        }}}
    })];
    let config = RegionRouteConfig {
        regions: serde_json::from_value(base.clone()).unwrap(),
        ..Default::default()
    };
    let (regions, applied) = apply_region_override(&config, &base, Some(&reference), &resources);
    assert!(!applied);
    assert_eq!(regions, base);
}

#[test]
fn region_override_is_validated_for_each_consumers_modulo() {
    use edgion_resources::resources::edgion_plugins::plugin_configs::HashCalcConfig;

    let reference = serde_json::json!({"namespace": "default", "name": "overlay"});
    let base = serde_json::json!([
        {"name": "base", "hashRange": [0, 99], "backendEndpoint": "base.example.test"}
    ]);
    let resources = vec![serde_json::json!({
        "metadata": {"namespace": "default", "name": "overlay"},
        "spec": {"data": {"type": "RegionRouteOverride", "config": {"regions": [
            {"name": "overlay", "hashRange": [0, 999], "backendEndpoint": "overlay.example.test"}
        ]}}}
    })];
    for (modulo, expected_applied) in [(100, false), (1000, true)] {
        let config = RegionRouteConfig {
            regions: serde_json::from_value(base.clone()).unwrap(),
            hash_calc: Some(HashCalcConfig {
                modulo,
                algorithm: Default::default(),
                write_back: Vec::new(),
            }),
            ..Default::default()
        };
        let (effective, applied) = apply_region_override(&config, &base, Some(&reference), &resources);
        assert_eq!(applied, expected_applied);
        assert_eq!(effective[0]["name"], if expected_applied { "overlay" } else { "base" });
    }
}

#[test]
fn disabled_and_absent_region_overrides_inherit_base() {
    let reference = serde_json::json!({"namespace": "default", "name": "overlay"});
    let base = serde_json::json!([
        {"name": "base", "hashRange": [0, 999], "backendEndpoint": "base.example.test"}
    ]);
    let config = RegionRouteConfig {
        regions: serde_json::from_value(base.clone()).unwrap(),
        ..Default::default()
    };
    for overlay in [
        serde_json::json!({}),
        serde_json::json!({"regions": null}),
        serde_json::json!({"regions": []}),
    ] {
        let resources = vec![serde_json::json!({
            "metadata": {"namespace": "default", "name": "overlay"},
            "spec": {"data": {"type": "RegionRouteOverride", "config": overlay}}
        })];
        let (effective, applied) = apply_region_override(&config, &base, Some(&reference), &resources);
        assert!(!applied);
        assert_eq!(effective, base);
    }
}
