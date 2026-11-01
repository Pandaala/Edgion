use super::*;

#[test]
fn dye_producer_inventory_matches_declared_capability_union() {
    use edgion_resources::resources::edgion_plugins::{DyeStage, EdgionPlugin};
    use std::collections::BTreeSet;
    // Actual producer source is independent of the schema capability table.
    // Policy/stage cases and executable producer tests supply the branch
    // evidence; this guard catches newly orphaned/undeclared outcome wiring.
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "Canary",
            &[
                r#"{"enable":true,"activeProfile":"s","profiles":{"s":{"name":"app"}},"onInvalid":"reject"}"#,
                r#"{"enable":true,"activeProfile":"s","profiles":{"s":{"name":"app"}},"onInvalid":"fallback"}"#,
            ],
            &[include_str!("../../../../src/plugins/http/canary/plugin.rs")],
        ),
        (
            "DynamicInternalUpstream",
            &[r#"{"from":{"type":"header","name":"X-Key"}}"#],
            &[include_str!(
                "../../../../src/plugins/http/dynamic_internal_upstream/plugin.rs"
            )],
        ),
        (
            "DynamicExternalUpstream",
            &[
                r#"{"from":{"type":"header","name":"X-Key"},"domainMap":{"a":{"domain":"example.com"}},"onMissing":"reject"}"#,
            ],
            &[include_str!(
                "../../../../src/plugins/http/dynamic_external_upstream/plugin.rs"
            )],
        ),
        (
            "DirectEndpoint",
            &[r#"{"from":{"type":"header","name":"X-Key"}}"#],
            &[include_str!("../../../../src/plugins/http/direct_endpoint/plugin.rs")],
        ),
        (
            "RegionRoute",
            &[
                r#"{"routeRules":[{"type":"RouteByDirectRegion","keyFrom":[{"type":"header","name":"X-Region"}]}],"clusterRegionRef":{"name":"regions"}}"#,
                r#"{"routeRules":[{"type":"RouteByDirectRegion","keyFrom":[{"type":"header","name":"X-Region"}]}],"failOpen":false,"clusterRegionRef":{"name":"regions"}}"#,
            ],
            &[include_str!("../../../../src/plugins/http/region_route/plugin.rs")],
        ),
        (
            "RateLimitRedis",
            &[
                r#"{"redisRef":"default/redis","quotas":[{"name":"default","policies":[{"rate":10,"interval":"1s"}]}],"onRedisFailure":"Allow"}"#,
                r#"{"redisRef":"default/redis","quotas":[{"name":"default","policies":[{"rate":10,"interval":"1s"}]}],"onRedisFailure":"Deny"}"#,
            ],
            &[include_str!("../../../../src/plugins/http/rate_limit_redis/plugin.rs")],
        ),
        (
            "ExtProc",
            &[
                r#"{"grpcService":{"target":"http://example.com:9000"},"failureModeAllow":true}"#,
                r#"{"grpcService":{"target":"http://example.com:9000"},"failureModeAllow":false}"#,
            ],
            &[include_str!("../../../../src/plugins/http/ext_proc/plugin.rs")],
        ),
        (
            "Wasm",
            &[
                r#"{"source":{"file":"plugin.wasm"},"failOpen":true}"#,
                r#"{"source":{"file":"plugin.wasm"},"failOpen":false}"#,
            ],
            &[include_str!("../../../../src/plugins/http/wasm/plugin.rs")],
        ),
        (
            "Dsl",
            &[r#"{"name":"d","source":"return next()"}"#],
            &[include_str!("../../../../src/plugins/http/dsl/lang/vm.rs")],
        ),
        (
            "OpenidConnect",
            &[r#"{"discovery":"https://example.com/discovery","clientId":"client"}"#],
            &[include_str!(
                "../../../../src/plugins/http/openid_connect/discovery_jwks.rs"
            )],
        ),
        (
            "ForwardAuth",
            &[r#"{"target":{"name":"auth","port":80},"decision":{}}"#],
            &[include_str!("../../../../src/plugins/http/forward_auth/plugin.rs")],
        ),
        (
            "LdapAuth",
            &[r#"{"ldapHost":"ldap.example.com","baseDn":"dc=example,dc=com","attribute":"uid"}"#],
            &[include_str!("../../../../src/plugins/http/ldap_auth/plugin.rs")],
        ),
        (
            "AiProxy",
            &[r#"{"api":"openaiChat"}"#],
            &[include_str!("../../../../src/ai/http/proxy.rs")],
        ),
    ];
    let producer = regex::Regex::new(r"\.record_outcome\(\s*DyeOutcome::(\w+)").unwrap();
    for (kind, configs, sources) in cases {
        let mut declared = BTreeSet::new();
        for config in *configs {
            let config: serde_json::Value = serde_json::from_str(config).unwrap();
            let plugin: EdgionPlugin = serde_json::from_value(serde_json::json!({"type":kind,"config":config}))
                .unwrap_or_else(|e| panic!("{kind}: {e}"));
            for stage in [
                DyeStage::Request,
                DyeStage::UpstreamResponseFilter,
                DyeStage::UpstreamResponseBodyFilter,
                DyeStage::UpstreamResponse,
            ] {
                for outcome in plugin.dye_capabilities(stage) {
                    declared.insert(format!("{outcome:?}"));
                }
            }
        }
        let recorded: BTreeSet<_> = sources
            .iter()
            .flat_map(|source| {
                let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
                producer
                    .captures_iter(production)
                    .map(|c| c[1].to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(declared, recorded, "{kind}: declared/producer dye drift");
    }
    for source in [
        include_str!("../../../../src/plugins/http/ext_proc/response_streamed.rs"),
        include_str!("../../../../src/plugins/http/ext_proc/response_trailer.rs"),
    ] {
        assert!(
            !source
                .split("#[cfg(test)]\nmod tests")
                .next()
                .unwrap()
                .contains(".record_outcome("),
            "committed response paths cannot produce header dye"
        );
    }
}

#[test]
fn compile_plan_splits_directions_and_compiles_templates() {
    let suite: DyeSuite = serde_yaml::from_str(
        "request:\n  - name: X-T\nresponse:\n  - name: X-O\n    on: [fail-open]\n    value: 'v ${header:X-Id}'\n",
    )
    .unwrap();
    let plan = CompiledDyePlan::compile(&suite, "canary").unwrap();
    assert_eq!(plan.request.len(), 1);
    assert_eq!(plan.response.len(), 1);
    assert!(plan.response[0].value.is_some());
    assert_eq!(plan.type_prefix, "canary");
}

#[test]
fn compile_plan_leaves_omitted_value_as_recorded_value() {
    let suite: DyeSuite = serde_yaml::from_str("response:\n  - name: X-O\n").unwrap();
    let plan = CompiledDyePlan::compile(&suite, "canary").unwrap();
    assert_eq!(plan.response.len(), 1);
    assert!(plan.response[0].value.is_none());
    assert!(plan.response[0].matches_outcome(DyeOutcome::Success));
    assert!(!plan.response[0].matches_outcome(DyeOutcome::FailClose));
}

#[test]
fn compile_plan_carries_rule_conditions() {
    let suite: DyeSuite = serde_yaml::from_str(
            "response:\n  - name: X-O\n    conditions:\n      run:\n        allOf:\n          - type: keyExist\n            key:\n              type: header\n              name: X-Debug\n",
        )
        .unwrap();
    let plan = CompiledDyePlan::compile(&suite, "canary").unwrap();
    assert!(plan.response[0].conditions.is_some());
}

#[test]
fn compiled_outcome_membership_matches_every_subset_and_duplicate_order() {
    let outcomes = [
        DyeOutcome::Success,
        DyeOutcome::Fallback,
        DyeOutcome::Reject,
        DyeOutcome::FailOpen,
        DyeOutcome::FailClose,
    ];
    for bits in 0..(1usize << outcomes.len()) {
        let selected: Vec<_> = outcomes
            .iter()
            .enumerate()
            .filter_map(|(i, outcome)| (bits & (1 << i) != 0).then_some(*outcome))
            .collect();
        let mut repeated = selected.clone();
        repeated.reverse();
        repeated.extend_from_slice(&selected);
        for on in [selected, repeated] {
            let rule = DyeRule {
                name: "x-dye".into(),
                on: on.clone(),
                value: None,
                conditions: None,
            };
            let compiled = CompiledDyeRule::compile(&rule, None).unwrap();
            for outcome in outcomes {
                assert_eq!(compiled.matches_outcome(outcome), on.contains(&outcome));
            }
            assert_eq!(rule.on, on);
        }
    }
}
