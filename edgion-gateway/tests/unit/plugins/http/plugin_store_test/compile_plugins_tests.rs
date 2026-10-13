use super::*;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::waf_session::{MockWafSessionCapability, WafSessionCapability};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::conditions::PluginConditions;
use edgion_resources::resources::edgion_plugins::plugin_configs::access_log_extern::{
    AccessLogExternField, LogExtSource,
};
use edgion_resources::resources::edgion_plugins::plugin_configs::DslConfig;
use edgion_resources::resources::edgion_plugins::{
    BodyRequirement, DyeOutcome, DyeRule, DyeSuite, EdgionPlugin, EdgionPluginsSpec, LimitHeaderNames,
    RateLimitLocalConfig, RateLimitRedisConfig, RequestFilterEntry, UpstreamResponseFilterEntry, WafConfig, WafMode,
};
use edgion_resources::resources::http_route::{HTTPHeader, HTTPHeaderFilter};
use edgion_resources::resources::plugin_policy::PluginPolicyAction;

fn policy_config(
    kind: &str,
    action: PluginPolicyAction,
) -> edgion_resources::resources::plugin_policy::PluginPolicyConfig {
    let rule = match action {
        PluginPolicyAction::Bypass => {
            serde_json::json!({"name":format!("http/{kind}"),"action":"bypass"})
        }
        PluginPolicyAction::Block { status } => {
            serde_json::json!({"name":format!("http/{kind}"),"blockStatus":status})
        }
    };
    serde_json::from_value(serde_json::json!({"deny":[rule]})).unwrap()
}

fn compile_with_policy(
    resource: &EdgionPlugins,
    kind: &str,
    action: Option<PluginPolicyAction>,
) -> Arc<CompiledPlugins> {
    let policy = action.map(|action| policy_config(kind, action).compile().unwrap());
    super::super::plugin_generation::PluginGeneration::new(resource.clone()).compile(policy.as_ref())
}

fn first_filter(runtime: &PluginRuntime) -> &Arc<dyn crate::plugins::runtime::RequestFilter> {
    runtime.request_plugins_iter().next().unwrap()
}

fn ai_ordering_resource(types: &[&str]) -> EdgionPlugins {
    let entries: Vec<serde_json::Value> = types
        .iter()
        .map(|kind| {
            let config = if *kind == "AiGuard" {
                serde_json::json!({"rules": [{"name": "guard-rule",
                        "match": {"type": "regex", "pattern": "private-prompt"}, "action": "reject"}]})
            } else if *kind == "AiProxy" {
                serde_json::json!({"api":"openaiChat"})
            } else {
                serde_json::json!({})
            };
            serde_json::json!({"type": kind, "config": config})
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"name": "ai-ordering", "namespace": "default"},
        "spec": {"requestPlugins": entries}
    }))
    .unwrap()
}

#[test]
fn request_mirror_standard_validation_has_controller_gateway_parity() {
    let resource: EdgionPlugins = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"name": "invalid-mirror", "namespace": "default"},
        "spec": {"requestPlugins": [{"type": "RequestMirror", "config": {
            "backendRef": {"name": "shadow", "port": 8080},
            "fraction": {"numerator": 1}, "percent": 50
        }}]}
    }))
    .unwrap();

    let controller_errors = resource.validate_plugins();
    let compiled = compile_plugins(&resource);
    assert_eq!(compiled.preparse_errors, controller_errors);
    assert_eq!(compiled.preparse_errors.len(), 1);
    assert!(compiled.preparse_errors[0].contains("only one of fraction or percent"));
    assert!(!compiled.is_activatable);
}

#[tokio::test]
async fn ai_ordering_invalid_generation_rejects_once_before_request_plugins() {
    for types in [
        vec!["AiProxy", "AiGuard"],
        vec!["AiProxy", "AiGuard", "AiGuard"],
        vec!["AiGuard", "AiProxy", "AiGuard"],
    ] {
        let mut resource = ai_ordering_resource(&types);
        // Even a guard that would skip this request cannot bypass the
        // unconditional resource-level configuration gate.
        resource
            .spec
            .request_plugins
            .as_mut()
            .unwrap()
            .last_mut()
            .unwrap()
            .conditions = Some(conditions(serde_json::json!({
                "skip": {"allOf": [{"type": "probability", "ratio": 1.0}]}
        })));
        let compiled = compile_plugins(&resource);
        assert_eq!(compiled.preparse_errors, resource.validate_plugins());
        assert_eq!(compiled.preparse_errors.len(), 1);
        assert!(!compiled.is_activatable);
        assert!(!compiled.retain_previous_on_invalid_update);
        assert_leading_reject(&compiled);
        assert_eq!(
            compiled
                .plugin_runtime
                .request_plugins_iter()
                .filter(|filter| filter.name() == "RejectInvalidConfig")
                .count(),
            1
        );

        let reject = compiled.plugin_runtime.request_plugins_iter().next().unwrap();
        // No session access is permitted: the reject must precede body
        // reads, condition evaluation, evaluator calls and AI planning.
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new(reject.name());
        let result = reject.run_request(&mut session, &mut log).await;
        match result {
            PluginRunningResult::ErrResponse { status, body } => {
                assert_eq!(status, 500);
                assert_eq!(body.unwrap().as_ref(), br#"{"message":"plugin configuration error"}"#);
            }
            other => panic!("expected fail-closed response, got {other:?}"),
        }
        let serialized = serde_json::to_value(&log).unwrap();
        let entries = serialized["log"].as_array().unwrap();
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.as_str() == Some("err: AiGuardOrdering"))
                .count(),
            1
        );
        assert!(!serialized.to_string().contains("private-prompt"));
    }
}

#[test]
fn ai_ordering_inert_entries_and_valid_orders_do_not_seal() {
    for types in [vec!["AiGuard", "AiProxy"], vec!["AiGuard"], vec!["AiProxy"]] {
        let resource = ai_ordering_resource(&types);
        let compiled = compile_plugins(&resource);
        assert_eq!(compiled.preparse_errors, resource.validate_plugins());
        assert!(compiled.preparse_errors.is_empty());
        assert!(compiled.is_activatable);
    }
    for index in 0..2 {
        for (enabled, policy) in [
            (false, None),
            (true, Some(PluginPolicyAction::Bypass)),
            (true, Some(PluginPolicyAction::Block { status: 403 })),
        ] {
            let mut resource = ai_ordering_resource(&["AiProxy", "AiGuard"]);
            let entry = &mut resource.spec.request_plugins.as_mut().unwrap()[index];
            entry.enable = enabled;
            let kind = entry.plugin.type_name();
            let compiled = compile_with_policy(&resource, kind, policy);
            if policy.is_none() {
                assert_eq!(compiled.preparse_errors, resource.validate_plugins());
            }
            assert!(compiled.preparse_errors.is_empty());
            assert!(compiled.is_activatable);
            assert!(compiled
                .plugin_runtime
                .request_plugins_iter()
                .all(|filter| filter.name() != "RejectInvalidConfig"));
        }
    }
}

#[test]
fn ai_ordering_invalid_publications_replace_and_recover() {
    let valid = compile_plugins(&ai_ordering_resource(&["AiGuard", "AiProxy"]));
    let invalid = compile_plugins(&ai_ordering_resource(&["AiProxy", "AiGuard"]));
    let key = "default/ai-ordering".to_string();
    for mode in ["full", "update", "add"] {
        let store = PluginStore::new();
        // Include invalid first publication, recovery, valid-to-invalid,
        // repeated invalid publication, and final recovery.
        for incoming in [&invalid, &valid, &invalid, &invalid, &valid] {
            let map = HashMap::from([(key.clone(), Arc::clone(incoming))]);
            match mode {
                "full" => store.replace_all(map),
                "update" => store.apply_delta(HashMap::new(), map, &HashSet::new()),
                "add" => store.apply_delta(map, HashMap::new(), &HashSet::new()),
                _ => unreachable!(),
            }
            let snapshot = store.scope();
            let published = snapshot.http(&key).unwrap();
            assert_eq!(published.preparse_errors, incoming.preparse_errors, "{mode}");
            assert_eq!(published.is_activatable, Arc::ptr_eq(incoming, &valid));
            if !published.is_activatable {
                assert_leading_reject(&published);
            }
        }
    }
}

#[tokio::test]
async fn ai_ordering_invalid_generation_preserves_policy_block_precedence() {
    let mut resource = ai_ordering_resource(&["AiProxy", "AiGuard"]);
    resource
        .spec
        .request_plugins
        .as_mut()
        .unwrap()
        .push(RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
            HTTPHeaderFilter {
                set: None,
                add: None,
                remove: None,
            },
        )));
    let compiled = compile_with_policy(
        &resource,
        "RequestHeaderModifier",
        Some(PluginPolicyAction::Block { status: 403 }),
    );
    assert_eq!(compiled.preparse_errors, resource.validate_plugins());
    assert_eq!(compiled.preparse_errors.len(), 1);
    assert!(!compiled.is_activatable);
    assert!(!compiled.retain_previous_on_invalid_update);
    let mut filters = compiled.plugin_runtime.request_plugins_iter();
    let policy = filters.next().unwrap();
    assert_eq!(policy.name(), "PluginPolicyBlock");
    assert_eq!(filters.next().unwrap().name(), "RejectInvalidConfig");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new(policy.name());
    assert!(matches!(
        policy.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse { status: 403, .. }
    ));
}

#[test]
fn ai_ordering_failure_overrides_dsl_and_waf_retention() {
    for program in ["dsl", "waf"] {
        let mut resource = ai_ordering_resource(&["AiGuard", "AiProxy"]);
        if program == "dsl" {
            resource
                .spec
                .request_plugins
                .as_mut()
                .unwrap()
                .push(RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
                    source: Some("invalid @@@".to_string()),
                    ..Default::default()
                })));
        } else {
            resource.spec.waf = waf_resource("SecRule invalid-(").spec.waf;
        }
        let program_only = compile_plugins(&resource);
        assert!(!program_only.is_activatable);
        assert!(program_only.retain_previous_on_invalid_update);
        resource.spec.request_plugins.as_mut().unwrap().swap(0, 1);
        let mixed = compile_plugins(&resource);
        let mut expected = resource.validate_plugins();
        expected.sort();
        let mut actual = mixed.preparse_errors.clone();
        actual.sort();
        assert_eq!(actual, expected);
        assert!(!mixed.is_activatable);
        assert!(!mixed.retain_previous_on_invalid_update);
        assert_leading_reject(&mixed);
        let valid = compile_plugins(&ai_ordering_resource(&["AiGuard", "AiProxy"]));
        assert!(!mixed.can_retain_previous(&valid));
    }
}

#[test]
fn dye_stage_policy_and_inert_entries_have_controller_gateway_parity() {
    use edgion_resources::resources::edgion_plugins::{
        DyeStage, UpstreamResponseBodyFilterEntry, UpstreamResponseEntry,
    };
    let file = tempfile::Builder::new().suffix(".wasm").tempfile().unwrap();
    std::fs::write(file.path(), wat::parse_str("(module)").unwrap()).unwrap();
    for stage in [
        DyeStage::Request,
        DyeStage::UpstreamResponseFilter,
        DyeStage::UpstreamResponseBodyFilter,
        DyeStage::UpstreamResponse,
    ] {
        for fail_open in [false, true] {
            for outcome in [DyeOutcome::FailOpen, DyeOutcome::FailClose] {
                // Disabled, bypassed and blocked entries remain inert for
                // dye validation; a policy block still owns its existing denial.
                for (enable, policy) in [
                    (true, None),
                    (false, None),
                    (true, Some(PluginPolicyAction::Bypass)),
                    (true, Some(PluginPolicyAction::Block { status: 403 })),
                ] {
                    let plugin: EdgionPlugin = serde_json::from_value(serde_json::json!({
                        "type": "Wasm", "config": { "source": { "file": file.path() }, "failOpen": fail_open }
                    }))
                    .unwrap();
                    let dye = Some(DyeSuite {
                        request: vec![],
                        response: vec![DyeRule {
                            name: "X-Dye".into(),
                            on: vec![outcome],
                            value: None,
                            conditions: None,
                        }],
                    });
                    let active = enable && policy.is_none();
                    let supported = matches!(stage, DyeStage::Request | DyeStage::UpstreamResponseFilter)
                        && (outcome == DyeOutcome::FailOpen) == fail_open;
                    let mut resource = EdgionPlugins::new("dye", EdgionPluginsSpec::default());
                    macro_rules! entry {
                        ($ty:ident, $field:ident) => {{
                            let mut entry = $ty::new(plugin);
                            entry.enable = enable;
                            entry.dye = dye;
                            resource.spec.$field = Some(vec![entry]);
                        }};
                    }
                    match stage {
                        DyeStage::Request => entry!(RequestFilterEntry, request_plugins),
                        DyeStage::UpstreamResponseFilter => {
                            entry!(UpstreamResponseFilterEntry, upstream_response_filter_plugins)
                        }
                        DyeStage::UpstreamResponseBodyFilter => {
                            entry!(UpstreamResponseBodyFilterEntry, upstream_response_body_filter_plugins)
                        }
                        DyeStage::UpstreamResponse => {
                            entry!(UpstreamResponseEntry, upstream_response_plugins)
                        }
                    }
                    let source_errors = resource.validate_plugins();
                    let compiled = compile_with_policy(&resource, "Wasm", policy);
                    if policy.is_none() {
                        assert_eq!(
                            source_errors, compiled.preparse_errors,
                            "{stage:?} {outcome:?} {fail_open}"
                        );
                    }
                    let schema = &compiled.preparse_errors;
                    assert_eq!(
                        schema
                            .iter()
                            .any(|e| e.contains("cannot render in this stage and configuration")),
                        active && !supported
                    );
                    let has_plan = match stage {
                        DyeStage::Request => compiled
                            .plugin_runtime
                            .request_plugins_iter()
                            .any(|p| p.dye_plan().is_some()),
                        DyeStage::UpstreamResponseFilter => compiled
                            .plugin_runtime
                            .upstream_response_plugins_iter()
                            .any(|p| p.dye_plan().is_some()),
                        DyeStage::UpstreamResponseBodyFilter => compiled
                            .plugin_runtime
                            .upstream_response_body_plugins_iter()
                            .any(|p| p.dye_plan().is_some()),
                        DyeStage::UpstreamResponse => compiled
                            .plugin_runtime
                            .upstream_response_async_plugins_iter()
                            .any(|p| p.dye_plan().is_some()),
                    };
                    assert_eq!(has_plan, active && supported);
                    if active && stage != DyeStage::UpstreamResponse {
                        assert!(
                            compiled.is_activatable,
                            "a dye error must not disable the plugin itself"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn dye_policy_update_replaces_plan_and_recovers_without_retaining_old_rules() {
    use edgion_resources::resources::edgion_plugins::plugin_configs::DslErrorPolicy;
    let mut entry = RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
        name: "dye".into(),
        source: Some("return next()".into()),
        error_policy: Some(DslErrorPolicy::Deny),
        ..Default::default()
    }));
    entry.dye = Some(DyeSuite {
        request: vec![],
        response: vec![DyeRule {
            name: "X-Dye".into(),
            on: vec![DyeOutcome::FailClose],
            value: None,
            conditions: None,
        }],
    });
    let mut resource = dsl_resource(entry);
    let store = PluginStore::new();
    let key = "default/dye".to_string();
    for outcome in [DyeOutcome::FailClose, DyeOutcome::FailOpen, DyeOutcome::FailClose] {
        resource.spec.request_plugins.as_mut().unwrap()[0]
            .dye
            .as_mut()
            .unwrap()
            .response[0]
            .on = vec![outcome];
        let compiled = compile_plugins(&resource);
        assert_eq!(resource.validate_plugins(), compiled.preparse_errors);
        assert!(compiled.is_activatable, "{:?}", compiled.preparse_errors);
        assert!(!compiled.retain_previous_on_invalid_update);
        store.apply_delta(
            HashMap::new(),
            HashMap::from([(key.clone(), compiled)]),
            &HashSet::new(),
        );
        assert_eq!(
            store.with_plugin(&key, |p| p
                .plugin_runtime
                .request_plugins_iter()
                .any(|f| f.dye_plan().is_some())),
            Some(outcome == DyeOutcome::FailClose)
        );
    }
}

#[test]
fn dye_canary_internal_kill_switch_keeps_anti_spoof_plan() {
    let mut resource: EdgionPlugins = serde_yaml::from_str(include_str!(
            "../../../../../../edgion-tests/integration/conf/EdgionPlugins/Canary/20_EdgionPlugins_default_canary-kill-switch-strip.yaml"
        )).unwrap();
    let compiled = compile_plugins(&resource);
    assert!(resource.validate_plugins().is_empty());
    assert!(compiled.preparse_errors.is_empty());
    let plan = compiled
        .plugin_runtime
        .request_plugins_iter()
        .next()
        .unwrap()
        .dye_plan()
        .unwrap();
    assert_eq!(plan.request[0].name, "X-Canary-Active");
    // The outer switch is different: no entry, no plan and no stripping.
    resource.spec.request_plugins.as_mut().unwrap()[0].enable = false;
    let compiled = compile_plugins(&resource);
    assert_eq!(compiled.plugin_runtime.request_plugins_count(), 0);
}

#[test]
fn dye_named_jump_fixtures_reject_unreachable_subscriptions_consistently() {
    // The integration manifests now subscribe only to supported outcomes.
    // Restore broad subscriptions in memory to retain D0083 negative coverage.
    let fixtures = [
            (false, include_str!("../../../../../../edgion-tests/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-canary-fallback.yaml")),
            (false, include_str!("../../../../../../edgion-tests/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-canary-reject.yaml")),
            (false, include_str!("../../../../../../edgion-tests/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-diu-fallback.yaml")),
            (true, include_str!("../../../../../../edgion-tests/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-diu-reject.yaml")),
        ];
    for (valid, yaml) in fixtures {
        let mut resource: EdgionPlugins = serde_yaml::from_str(yaml).unwrap();
        let fixture = compile_plugins(&resource);
        assert!(resource.validate_plugins().is_empty());
        assert!(fixture.preparse_errors.is_empty());
        assert!(fixture.is_activatable);
        assert!(fixture
            .plugin_runtime
            .request_plugins_iter()
            .any(|plugin| plugin.dye_plan().is_some()));
        if !valid {
            resource.spec.request_plugins.as_mut().unwrap()[0]
                .dye
                .as_mut()
                .unwrap()
                .response[0]
                .on = vec![DyeOutcome::Reject, DyeOutcome::Fallback];
        }
        let errors = resource.validate_plugins();
        let compiled = compile_plugins(&resource);
        assert_eq!(errors, compiled.preparse_errors);
        assert_eq!(errors.is_empty(), valid, "{:?}: {errors:?}", resource.metadata.name);
        if !valid {
            assert!(errors
                .iter()
                .any(|e| e.contains("cannot render in this stage and configuration")));
        }
        assert!(compiled.is_activatable, "only dye is rejected");
        assert_eq!(
            compiled
                .plugin_runtime
                .request_plugins_iter()
                .any(|p| p.dye_plan().is_some()),
            valid
        );
    }
}

fn header_modifier() -> EdgionPlugin {
    EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
        set: Some(vec![HTTPHeader {
            name: "X-Test".into(),
            value: "test-value".into(),
        }]),
        add: None,
        remove: None,
    })
}

fn conditions(value: serde_json::Value) -> PluginConditions {
    serde_json::from_value(value).expect("condition fixture must deserialize")
}

fn resource_with_ip_conditions(count: usize) -> EdgionPlugins {
    use edgion_resources::resources::edgion_plugins::conditions::{Condition, ConditionSet, IpAccessMatch};

    let mut entry = RequestFilterEntry::new(header_modifier());
    entry.conditions = Some(PluginConditions {
        run: Some(ConditionSet::all_of(vec![
            Condition::DirectPeerIpMatch(IpAccessMatch {
                refs: edgion_resources::resources::edgion_plugins::ip_reference_set::IpReferenceSet {
                    ip_refs: vec![edgion_resources::resources::edgion_config_data::EdgionConfigDataRef {
                        name: "test-networks".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            });
            count
        ])),
        ..Default::default()
    });
    EdgionPlugins {
        metadata: kube::api::ObjectMeta {
            name: Some("ip-conditions".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![entry]),
            ..Default::default()
        },
        status: None,
    }
}

#[test]
fn ip_condition_count_does_not_limit_validation_or_publication() {
    let key = "default/ip-conditions".to_string();
    let store = PluginStore::new();
    let initial = compile_plugins(&resource_with_ip_conditions(1));
    store.replace_all(HashMap::from([(key.clone(), initial)]));

    // Keep literal historical boundaries independent of consumer-side limits.
    for count in [10_000, 10_001] {
        let resource = resource_with_ip_conditions(count);
        let errors = resource.validate_plugins();
        assert!(errors.is_empty(), "{count}: {errors:?}");
        let compiled = compile_plugins(&resource);
        assert!(compiled.preparse_errors.is_empty(), "{:?}", compiled.preparse_errors);
        assert!(compiled.is_activatable);
        assert!(!compiled.retain_previous_on_invalid_update);
        assert_eq!(compiled.plugin_runtime.request_plugins_count(), 1);
        assert!(compiled
            .plugin_runtime
            .request_plugins_iter()
            .all(|filter| filter.name() != "RejectInvalidConfig"));

        store.apply_delta(
            HashMap::new(),
            HashMap::from([(key.clone(), Arc::clone(&compiled))]),
            &HashSet::new(),
        );
        assert!(store
            .with_plugin(&key, |published| published.is_activatable
                && published.preparse_errors.is_empty()
                && published.plugin_runtime.request_plugins_count() == 1)
            .unwrap());
        store.replace_all(HashMap::from([(key.clone(), Arc::clone(&compiled))]));
        assert!(store
            .with_plugin(&key, |published| published.is_activatable
                && published.preparse_errors.is_empty()
                && published.plugin_runtime.request_plugins_count() == 1)
            .unwrap());
    }
}

#[test]
fn large_ip_condition_resource_still_rejects_invalid_conditions() {
    let mut resource = resource_with_ip_conditions(10_001);
    resource.spec.request_plugins.as_mut().unwrap()[0]
        .conditions
        .as_mut()
        .unwrap()
        .skip = malformed_time_range_conditions().run;
    let errors = resource.validate_plugins();
    assert!(!errors.is_empty());
    let compiled = compile_plugins(&resource);
    assert_eq!(compiled.preparse_errors, errors);
    assert!(!compiled.is_activatable);
    assert!(!compiled.retain_previous_on_invalid_update);
    assert_leading_reject(&compiled);
}

fn malformed_time_range_conditions() -> PluginConditions {
    conditions(serde_json::json!({
        "run": { "allOf": [{ "type": "timeRange", "after": "not-rfc3339" }] }
    }))
}

fn bad_regex_conditions() -> PluginConditions {
    conditions(serde_json::json!({
        "run": { "allOf": [{
            "type": "keyMatch",
            "key": { "type": "header", "name": "X-Test" },
            "regex": ["("]
        }] }
    }))
}

fn unresolved_conditions() -> PluginConditions {
    conditions(serde_json::json!({
        "run": { "allOf": [{
            "type": "secretMatch",
            "key": {"type":"header","name":"x-token"},
            "secretKey": "token",
            "secretRef": {"name":"missing"},
            "negate": true
        }] }
    }))
}

fn body_dsl_entry() -> RequestFilterEntry {
    RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
        name: "body-condition-test".to_string(),
        source: Some("let found = req.body.contains(\"needle\")".to_string()),
        ..Default::default()
    }))
}

fn dsl_resource(entry: RequestFilterEntry) -> EdgionPlugins {
    EdgionPlugins {
        metadata: kube::api::ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("condition-test".to_string()),
            ..Default::default()
        },
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![entry]),
            ..Default::default()
        },
        status: None,
    }
}

fn response_dye(conditions: Vec<PluginConditions>) -> DyeSuite {
    DyeSuite {
        request: Vec::new(),
        response: conditions
            .into_iter()
            .enumerate()
            .map(|(index, conditions)| DyeRule {
                name: format!("X-Condition-Dye-{index}"),
                on: vec![DyeOutcome::FailOpen, DyeOutcome::FailClose],
                value: None,
                conditions: Some(conditions),
            })
            .collect(),
    }
}

fn entry_condition_resource(condition: PluginConditions) -> EdgionPlugins {
    let mut entry = body_dsl_entry();
    entry.conditions = Some(condition);
    dsl_resource(entry)
}

fn body_condition_resource(condition: PluginConditions) -> EdgionPlugins {
    let mut entry = body_dsl_entry();
    entry.body = Some(BodyRequirement {
        conditions: Some(condition),
        ..Default::default()
    });
    dsl_resource(entry)
}

fn dye_condition_resource(condition: PluginConditions) -> EdgionPlugins {
    let mut entry = body_dsl_entry();
    entry.dye = Some(response_dye(vec![condition]));
    dsl_resource(entry)
}

fn assert_leading_reject(compiled: &CompiledPlugins) {
    assert_eq!(
        compiled
            .plugin_runtime
            .request_plugins_iter()
            .next()
            .map(|filter| filter.name()),
        Some("RejectInvalidConfig")
    );
}

#[test]
fn active_condition_error_matrix_seals_fail_closed() {
    let cases = [
        (
            "entry timeRange",
            entry_condition_resource(malformed_time_range_conditions()),
            "conditions: timeRange.after",
        ),
        (
            "entry regex",
            entry_condition_resource(bad_regex_conditions()),
            "conditions regex:",
        ),
        (
            "body timeRange",
            body_condition_resource(malformed_time_range_conditions()),
            "body conditions: timeRange.after",
        ),
        (
            "body regex",
            body_condition_resource(bad_regex_conditions()),
            "body conditions regex:",
        ),
        (
            "dye timeRange",
            dye_condition_resource(malformed_time_range_conditions()),
            "dye conditions: timeRange.after",
        ),
        (
            "dye regex",
            dye_condition_resource(bad_regex_conditions()),
            "dye conditions regex:",
        ),
    ];

    for (name, resource, expected_error) in cases {
        let mut controller_errors = resource.validate_plugins();
        controller_errors.sort();
        let compiled = compile_plugins(&resource);
        let mut gateway_errors = compiled.preparse_errors.clone();
        gateway_errors.sort();

        assert_eq!(gateway_errors, controller_errors, "static validation parity: {name}");
        assert!(
            compiled
                .preparse_errors
                .iter()
                .any(|error| error.contains(expected_error)),
            "missing {expected_error:?} for {name}: {:?}",
            compiled.preparse_errors
        );
        assert!(!compiled.is_activatable, "{name}");
        assert!(!compiled.retain_previous_on_invalid_update, "{name}");
        assert_leading_reject(&compiled);
    }
}

#[test]
fn unresolved_body_condition_is_nonactivatable_nonretainable_and_sealed() {
    let compiled = compile_plugins(&body_condition_resource(unresolved_conditions()));

    assert!(compiled
        .preparse_errors
        .iter()
        .any(|error| { error == "requestPlugins[0] body conditions contain unresolved references" }));
    assert!(!compiled.is_activatable);
    assert!(!compiled.retain_previous_on_invalid_update);
    assert_leading_reject(&compiled);
}

#[test]
fn unresolved_dye_condition_remains_observational_while_malformed_dye_is_invalid() {
    let unresolved = compile_plugins(&dye_condition_resource(unresolved_conditions()));
    assert!(unresolved.is_activatable);
    assert!(unresolved.preparse_errors.is_empty());
    assert!(unresolved
        .plugin_runtime
        .request_plugins_iter()
        .all(|filter| filter.name() != "RejectInvalidConfig"));

    for resource in [
        dye_condition_resource(malformed_time_range_conditions()),
        dye_condition_resource(bad_regex_conditions()),
    ] {
        let compiled = compile_plugins(&resource);
        assert!(!compiled.is_activatable);
        assert!(!compiled.retain_previous_on_invalid_update);
        assert_leading_reject(&compiled);
    }
}

#[test]
fn dsl_condition_failure_replaces_last_known_good_generation() {
    let key = "default/condition-test".to_string();
    let valid_entry = body_dsl_entry();
    let mut invalid_entry = valid_entry.clone();
    invalid_entry.conditions = Some(malformed_time_range_conditions());
    let valid = compile_plugins(&dsl_resource(valid_entry));
    let invalid = compile_plugins(&dsl_resource(invalid_entry));
    assert!(valid.is_activatable);
    assert!(!invalid.is_activatable);
    assert!(!invalid.retain_previous_on_invalid_update);
    assert_leading_reject(&invalid);

    let store = PluginStore::new();
    store.apply_delta(
        HashMap::from([(key.clone(), Arc::clone(&valid))]),
        HashMap::new(),
        &HashSet::new(),
    );
    store.apply_delta(
        HashMap::new(),
        HashMap::from([(key.clone(), Arc::clone(&invalid))]),
        &HashSet::new(),
    );

    let snapshot = store.scope();
    let published = snapshot.http(&key).expect("updated generation is published");
    assert!(!published.is_activatable);
    assert_eq!(published.preparse_errors, invalid.preparse_errors);
    assert_leading_reject(&published);
}

#[tokio::test]
async fn condition_invalid_entry_is_logged_once_across_all_condition_sites() {
    let mut entry = body_dsl_entry();
    entry.conditions = Some(malformed_time_range_conditions());
    entry.body = Some(BodyRequirement {
        conditions: Some(bad_regex_conditions()),
        ..Default::default()
    });
    entry.dye = Some(response_dye(vec![
        malformed_time_range_conditions(),
        bad_regex_conditions(),
    ]));
    let compiled = compile_plugins(&dsl_resource(entry));

    for expected in [
        "conditions: timeRange.after",
        "body conditions regex:",
        "dye conditions: timeRange.after",
        "dye conditions regex:",
    ] {
        assert!(
            compiled.preparse_errors.iter().any(|error| error.contains(expected)),
            "missing {expected:?}: {:?}",
            compiled.preparse_errors
        );
    }
    assert_leading_reject(&compiled);

    let reject = compiled
        .plugin_runtime
        .request_plugins_iter()
        .next()
        .expect("reject gate is installed");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new(reject.name());
    let result = reject.run_request(&mut session, &mut log).await;
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));

    let serialized = serde_json::to_value(&log).expect("PluginLog serializes");
    let entries = serialized["log"].as_array().expect("reject log entries");
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.as_str() == Some("err: invalid plugin config"))
            .count(),
        1
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.as_str() == Some("err: Dsl"))
            .count(),
        1
    );
}

#[test]
fn disabled_and_scoped_bypass_malformed_conditions_remain_inert() {
    for (name, mut entry) in [("disabled", body_dsl_entry()), ("policy bypass", body_dsl_entry())] {
        entry.conditions = Some(malformed_time_range_conditions());
        entry.body = Some(BodyRequirement {
            conditions: Some(bad_regex_conditions()),
            ..Default::default()
        });
        entry.dye = Some(response_dye(vec![malformed_time_range_conditions()]));
        if name == "disabled" {
            entry.enable = false;
        }

        let resource = dsl_resource(entry);
        let action = (name != "disabled").then_some(PluginPolicyAction::Bypass);
        assert_eq!(resource.validate_plugins().is_empty(), name == "disabled");
        let compiled = compile_with_policy(&resource, "Dsl", action);
        assert!(
            compiled.preparse_errors.is_empty(),
            "{name}: {:?}",
            compiled.preparse_errors
        );
        assert!(compiled.is_activatable, "{name}");
        assert!(compiled
            .plugin_runtime
            .request_plugins_iter()
            .all(|filter| filter.name() != "RejectInvalidConfig"));
    }
}

#[test]
fn auth_realm_invalid_configs_match_controller_validation_and_fail_closed() {
    for kind in [
        "BasicAuth",
        "KeyAuth",
        "JwtAuth",
        "HmacAuth",
        "LdapAuth",
        "OpenidConnect",
    ] {
        let mut config = match kind {
            "LdapAuth" => serde_json::json!({
                "ldapHost": "ldap.example.com", "baseDn": "dc=example,dc=com", "attribute": "uid"
            }),
            "OpenidConnect" => serde_json::json!({
                "discovery": "https://idp.example.com/.well-known/openid-configuration",
                "clientId": "test-client", "bearerOnly": true
            }),
            _ => serde_json::json!({"anonymous": "guest"}),
        };
        config["realm"] = serde_json::json!("private\r\nrealm");
        let ep: EdgionPlugins = serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
            "metadata": {"name": "realm-test", "namespace": "default"},
            "spec": {"requestPlugins": [{"type": kind, "config": config}]}
        }))
        .unwrap();
        let controller_errors = ep.validate_plugins();
        assert!(!controller_errors.is_empty(), "{kind}");
        let compiled = compile_plugins(&ep);
        assert_eq!(compiled.preparse_errors, controller_errors, "{kind}");
        assert!(!compiled.is_activatable, "{kind}");
        assert!(!compiled.retain_previous_on_invalid_update, "{kind}");
        assert_eq!(
            compiled.plugin_runtime.request_plugins_count(),
            1,
            "fail-closed gate: {kind}"
        );
        assert!(!compiled.preparse_errors.join(";").contains("private"));
    }
}

#[test]
fn auth_realm_invalid_reload_replaces_valid_generation_and_recovers() {
    let resource = |realm: &str, enabled: bool| -> EdgionPlugins {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
            "metadata": {"name": "realm-test", "namespace": "default"},
            "spec": {"requestPlugins": [{"type": "BasicAuth", "enable": enabled,
                "config": {"anonymous": "guest", "realm": realm}}]}
        }))
        .unwrap()
    };
    let store = PluginStore::new();
    let key = "default/realm-test".to_string();
    let valid = compile_plugins(&resource("initial", true));
    assert!(valid.is_activatable);
    store.replace_all(HashMap::from([(key.clone(), valid)]));
    for (realm, expected_active) in [("bad\0realm", false), ("Partner \"API\"\\", true)] {
        let next = compile_plugins(&resource(realm, true));
        assert_eq!(next.is_activatable, expected_active);
        store.apply_delta(
            HashMap::new(),
            HashMap::from([(key.clone(), Arc::clone(&next))]),
            &HashSet::new(),
        );
        let published = store.scope().http(&key).unwrap();
        assert_eq!(published.is_activatable, expected_active);
        assert_eq!(published.preparse_errors, next.preparse_errors);
    }
    let disabled = resource("bad\0realm", false);
    assert!(disabled.validate_plugins().is_empty());
    assert!(compile_plugins(&disabled).is_activatable);
}

#[test]
fn header_modifier_invalid_reload_replaces_valid_generation_and_recovers() {
    for (stage, kind) in [
        ("requestPlugins", "RequestHeaderModifier"),
        ("upstreamResponseFilterPlugins", "ResponseHeaderModifier"),
    ] {
        let resource = |value: &str| -> EdgionPlugins {
            serde_json::from_value(serde_json::json!({
                "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
                "metadata": {"name": "headers", "namespace": "default"},
                "spec": {stage: [{"type": kind, "config": {
                    "set": [{"name": "x-test", "value": value}]
                }}]}
            }))
            .unwrap()
        };
        let store = PluginStore::new();
        let key = "default/headers".to_string();
        let initial = compile_plugins(&resource("initial"));
        assert!(initial.is_activatable);
        store.replace_all(HashMap::from([(key.clone(), initial)]));
        let captured = store.scope().http(&key).unwrap();
        for (value, expected_active) in [("private\r\nvalue", false), ("recovered", true)] {
            let source = resource(value);
            let next = compile_plugins(&source);
            assert_eq!(next.preparse_errors, source.validate_plugins());
            assert_eq!(next.is_activatable, expected_active);
            assert!(!next.retain_previous_on_invalid_update);
            store.apply_delta(HashMap::new(), HashMap::from([(key.clone(), next)]), &HashSet::new());
            let published = store.scope().http(&key).unwrap();
            assert_eq!(published.is_activatable, expected_active);
            if !expected_active {
                assert_leading_reject(&published);
                assert!(!published.preparse_errors.join(";").contains("private"));
            }
            // An already captured generation remains valid through reload.
            assert!(captured.is_activatable);
        }
    }
}

fn waf_resource(rules: &str) -> EdgionPlugins {
    EdgionPlugins {
        metadata: kube::api::ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("native-waf".to_string()),
            ..Default::default()
        },
        spec: EdgionPluginsSpec {
            current_status: None,
            waf: Some(WafConfig {
                mode: WafMode::On,
                ..crate::plugins::http::waf::fixture::config(rules)
            }),
            ..Default::default()
        },
        status: None,
    }
}

#[test]
fn valid_waf_builds_one_logical_filter_for_each_supported_gateway_hook() {
    let compiled = compile_plugins(&waf_resource(
        "SecRule REQUEST_URI \"@contains /blocked\" \"id:1,phase:1,deny,status:403\"\n",
    ));

    assert!(compiled.is_activatable);
    assert!(compiled.preparse_errors.is_empty());
    assert_eq!(compiled.plugin_runtime.request_plugins_count(), 1);
    assert_eq!(compiled.plugin_runtime.upstream_response_plugins_count(), 1);
    assert_eq!(compiled.plugin_runtime.upstream_response_body_plugins_count(), 0);
    assert_eq!(compiled.plugin_runtime.upstream_response_async_plugins_count(), 1);
}

#[test]
fn invalid_waf_update_retains_the_last_known_good_generation() {
    let valid = compile_source(&waf_resource(
        "SecRule REQUEST_URI \"@contains /blocked\" \"id:1,phase:1,deny\"\n",
    ));
    let invalid = compile_source(&waf_resource("SecRule invalid-("));
    assert!(!invalid.is_activatable);
    assert!(invalid.retain_previous_on_invalid_update);
    assert!(invalid.preparse_errors.iter().any(|error| error.starts_with("waf: ")));

    let store = PluginStore::new();
    store.apply_delta(
        HashMap::from([("default/native-waf".to_string(), Arc::clone(&valid))]),
        HashMap::new(),
        &HashSet::new(),
    );
    let previous_runtime = store.get_plugin_runtime("default/native-waf").unwrap();
    store.apply_delta(
        HashMap::new(),
        HashMap::from([("default/native-waf".to_string(), Arc::clone(&invalid))]),
        &HashSet::new(),
    );

    let snapshot = store.scope();
    let retained = snapshot
        .http("default/native-waf")
        .expect("valid WAF generation remains published");
    assert!(retained.is_activatable);
    assert!(Arc::ptr_eq(
        first_filter(&retained.plugin_runtime),
        first_filter(&previous_runtime)
    ));
}

/// Compile a resource while keeping its source spec as store input. Published
/// views drop resolved WAF bundles, so feeding `compiled.spec` back into a
/// store would recompile an unresolved generation.
fn compile_source(resource: &EdgionPlugins) -> Arc<CompiledPlugins> {
    let compiled = compile_plugins(resource);
    Arc::new(CompiledPlugins {
        spec: Arc::new(resource.clone()),
        plugin_runtime: Arc::clone(&compiled.plugin_runtime),
        preparse_errors: compiled.preparse_errors.clone(),
        is_activatable: compiled.is_activatable,
        retain_previous_on_invalid_update: compiled.retain_previous_on_invalid_update,
    })
}

fn bundled_waf_resource(id: u64) -> EdgionPlugins {
    waf_resource(&format!(
        "SecRule REQUEST_URI \"@contains /blocked\" \"id:{id},phase:1,deny\"\n"
    ))
}

#[test]
fn bundled_waf_reloads_retain_invalid_update_and_pin_in_flight_generation() {
    let key = "default/native-waf".to_string();
    let first = compile_source(&bundled_waf_resource(100));
    let second = compile_source(&bundled_waf_resource(101));
    assert!(first.is_activatable, "{:?}", first.preparse_errors);
    assert!(second.is_activatable, "{:?}", second.preparse_errors);

    let mut unavailable = bundled_waf_resource(102);
    unavailable.spec.waf.as_mut().unwrap().resolved_bundles = None;
    let invalid = compile_source(&unavailable);
    assert!(!invalid.is_activatable);
    assert!(invalid.retain_previous_on_invalid_update);

    let store = PluginStore::new();
    store.replace_all(HashMap::from([(key.clone(), first)]));
    let in_flight = store.get_plugin_runtime(&key).unwrap();
    store.apply_delta(HashMap::new(), HashMap::from([(key.clone(), second)]), &HashSet::new());
    let current = store.get_plugin_runtime(&key).unwrap();
    assert!(!Arc::ptr_eq(first_filter(&in_flight), first_filter(&current)));

    store.apply_delta(HashMap::new(), HashMap::from([(key.clone(), invalid)]), &HashSet::new());
    let retained = store.get_plugin_runtime(&key).unwrap();
    assert!(Arc::ptr_eq(first_filter(&current), first_filter(&retained)));

    let rollback = compile_source(&bundled_waf_resource(100));
    assert!(rollback.is_activatable);
    store.apply_delta(
        HashMap::new(),
        HashMap::from([(key.clone(), rollback)]),
        &HashSet::new(),
    );
    let rolled_back = store.get_plugin_runtime(&key).unwrap();
    assert!(!Arc::ptr_eq(first_filter(&current), first_filter(&rolled_back)));
    assert_eq!(in_flight.request_plugins_count(), 1);
}

fn mirror_program_resource(program: &str, valid: bool, mirror: Option<serde_json::Value>) -> EdgionPlugins {
    let mut resource = waf_resource(if valid {
        "SecRule REQUEST_URI \"@contains /blocked\" \"id:1,phase:1,deny\"\n"
    } else {
        "SecRule invalid-("
    });
    let mut entries = Vec::new();
    if let Some(config) = mirror {
        entries.push(RequestFilterEntry::new(EdgionPlugin::RequestMirror(
            serde_json::from_value(config).expect("mirror fixture parses"),
        )));
    }
    if program == "dsl" {
        resource.spec.waf = None;
        entries.push(RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
            name: "mirror-lkg".to_string(),
            source: Some(if valid { "let a = 1" } else { "invalid @@@" }.to_string()),
            ..Default::default()
        })));
    }
    resource.spec.request_plugins = Some(entries);
    resource
}

fn mirror_config(namespace: &str, denied: bool) -> serde_json::Value {
    let mut config = serde_json::json!({
        "backendRef": { "name": "mirror-lkg", "namespace": namespace, "port": 8080 }
    });
    if denied {
        config["backendRef"]["refDenied"] = serde_json::json!({
            "targetNamespace": namespace, "targetName": "mirror-lkg", "reason": "NoMatchingReferenceGrant"
        });
    }
    config
}

#[tokio::test]
async fn policy_restriction_survives_invalid_program_publication() {
    for program in ["dsl", "waf"] {
        for full_snapshot in [false, true] {
            for case in [
                "current-block",
                "new-current-block",
                "retained-only-block",
                "bypass",
                "removal",
            ] {
                let entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
                    set: None,
                    add: None,
                    remove: None,
                }));
                let mut old = mirror_program_resource(program, true, None);
                if case != "new-current-block" {
                    old.spec.request_plugins.as_mut().unwrap().push(entry.clone());
                }
                let mut incoming = mirror_program_resource(program, false, None);
                if case != "retained-only-block" {
                    incoming.spec.request_plugins.as_mut().unwrap().push(entry);
                }
                let key = old.key_name();
                let store = PluginStore::new();
                store.replace_all(HashMap::from([(key.clone(), compile_source(&old))]));
                let in_flight = store.get_plugin_runtime(&key).unwrap();
                let action = if case == "bypass" {
                    PluginPolicyAction::Bypass
                } else {
                    PluginPolicyAction::Block { status: 451 }
                };
                store.set_test_policy(Some(policy_config("RequestHeaderModifier", action)));
                let update = HashMap::from([(key.clone(), compile_source(&incoming))]);
                if full_snapshot {
                    store.replace_all(update);
                } else {
                    store.apply_delta(HashMap::new(), update, &HashSet::new());
                }
                if case == "removal" {
                    store.set_test_policy(None);
                }
                let scope = store.scope();
                let published = scope.http(&key).unwrap();
                assert_eq!(
                    published.is_activatable,
                    case != "removal",
                    "{program}/{full_snapshot}/{case}"
                );
                let names: Vec<_> = published
                    .plugin_runtime
                    .request_plugins_iter()
                    .map(|p| p.name())
                    .collect();
                assert_eq!(
                    names.contains(&"RejectInvalidConfig"),
                    case == "removal",
                    "{case}: {names:?}"
                );
                assert_eq!(
                    in_flight
                        .request_plugins_iter()
                        .any(|p| p.name() == "RequestHeaderModifier"),
                    case != "new-current-block"
                );
                if case == "removal" {
                    assert_eq!(names[0], "RejectInvalidConfig");
                    assert!(!names.contains(&"PluginPolicyBlock"));
                    let mut session = MockPluginSession::new();
                    let mut log = PluginLog::new("RejectInvalidConfig");
                    assert!(matches!(
                        first_filter(&published.plugin_runtime)
                            .run_request(&mut session, &mut log)
                            .await,
                        PluginRunningResult::ErrResponse { status: 500, .. }
                    ));
                } else if case == "bypass" {
                    assert!(names.contains(&"PluginPolicyBypass"));
                    assert!(!names.contains(&"RequestHeaderModifier"));
                } else {
                    assert_eq!(names[0], "PluginPolicyBlock");
                    let mut session = MockPluginSession::new();
                    let mut log = PluginLog::new("PluginPolicyBlock");
                    assert!(matches!(
                        first_filter(&published.plugin_runtime)
                            .run_request(&mut session, &mut log)
                            .await,
                        PluginRunningResult::ErrResponse { status: 451, .. }
                    ));
                }
            }
        }
    }
}

#[tokio::test]
async fn mirror_authorization_updates_never_retain_cross_namespace_lkg() {
    for program in ["dsl", "waf"] {
        for full_snapshot in [false, true] {
            for incoming_mirror in [
                Some(mirror_config("other", true)),
                None,
                Some(mirror_config("default", false)),
                Some(mirror_config("retargeted", false)),
            ] {
                let previous = compile_source(&mirror_program_resource(
                    program,
                    true,
                    Some(mirror_config("other", false)),
                ));
                let incoming = compile_source(&mirror_program_resource(program, false, incoming_mirror));
                assert!(previous.is_activatable);
                assert!(!incoming.is_activatable);
                assert!(incoming.retain_previous_on_invalid_update);
                let key = previous.spec.key_name();
                let store = PluginStore::new();
                store.replace_all(HashMap::from([(key.clone(), Arc::clone(&previous))]));
                let in_flight = store.get_plugin_runtime(&key).expect("previous runtime is pinned");
                let replacement = HashMap::from([(key.clone(), Arc::clone(&incoming))]);
                if full_snapshot {
                    store.replace_all(replacement);
                } else {
                    store.apply_delta(HashMap::new(), replacement, &HashSet::new());
                }
                let active = store.get_plugin_runtime(&key).expect("sealed replacement is published");
                assert_eq!(first_filter(&active).name(), "RejectInvalidConfig");
                assert!(in_flight.request_plugins_iter().any(|p| p.name() == "RequestMirror"));
                assert!(!Arc::ptr_eq(&active, &in_flight));
                assert_leading_reject(&incoming);
                let reject = active.request_plugins_iter().next().expect("reject precedes mirror");
                let mut session = MockPluginSession::new();
                let mut log = PluginLog::new(reject.name());
                assert!(matches!(
                    reject.run_request(&mut session, &mut log).await,
                    PluginRunningResult::ErrResponse { status: 500, .. }
                ));

                let recovered = compile_source(&mirror_program_resource(
                    program,
                    true,
                    Some(mirror_config("other", true)),
                ));
                assert!(recovered.is_activatable);
                store.apply_delta(
                    HashMap::new(),
                    HashMap::from([(key.clone(), Arc::clone(&recovered))]),
                    &HashSet::new(),
                );
                let recovered_runtime = store.get_plugin_runtime(&key).expect("valid update recovers");
                assert!(!Arc::ptr_eq(&active, &recovered_runtime));
                let mirror = recovered_runtime
                    .request_plugins_iter()
                    .find(|filter| filter.name() == "RequestMirror")
                    .expect("valid denied mirror remains executable");
                let mut session = MockPluginSession::new();
                let mut log = PluginLog::new(mirror.name());
                assert!(matches!(
                    mirror.run_request(&mut session, &mut log).await,
                    PluginRunningResult::GoodNext
                ));
            }
        }
    }
}

#[test]
fn mirror_lkg_boundary_checks_incoming_and_preserves_inert_or_local_mirrors() {
    for program in ["dsl", "waf"] {
        for full_snapshot in [false, true] {
            for case in ["added", "local", "implicit", "disabled", "bypassed", "absent"] {
                let config = match case {
                    "absent" | "added" => None,
                    "implicit" => Some(serde_json::json!({"backendRef": {"name": "mirror-lkg", "port": 8080}})),
                    "local" => Some(mirror_config("default", false)),
                    _ => Some(mirror_config("other", false)),
                };
                let mut old_resource = mirror_program_resource(program, true, config.clone());
                let mut new_resource = mirror_program_resource(
                    program,
                    false,
                    if case == "added" {
                        Some(mirror_config("other", true))
                    } else {
                        config
                    },
                );
                for resource in [&mut old_resource, &mut new_resource] {
                    if case == "implicit" {
                        resource.metadata.namespace = None;
                    }
                    if let Some(entry) = resource
                        .spec
                        .request_plugins
                        .as_mut()
                        .and_then(|entries| entries.first_mut())
                    {
                        if case == "disabled" {
                            entry.enable = false;
                        }
                    }
                }
                let action = (case == "bypassed").then_some(PluginPolicyAction::Bypass);
                let previous = compile_with_policy(&old_resource, "RequestMirror", action);
                let incoming = compile_with_policy(&new_resource, "RequestMirror", action);
                assert!(previous.is_activatable, "{case}");
                assert!(!incoming.is_activatable, "{case}");
                assert!(incoming.retain_previous_on_invalid_update, "{case}");
                // Global references always use a namespace; the source still has
                // no namespace in the implicit-local mirror case.
                let key = "default/native-waf".to_string();
                let store = PluginStore::new();
                store.set_test_policy(action.map(|action| policy_config("RequestMirror", action)));
                store.replace_all(HashMap::from([(key.clone(), compile_source(&old_resource))]));
                let previous_runtime = store.get_plugin_runtime(&key).unwrap();
                let replacement = HashMap::from([(key.clone(), compile_source(&new_resource))]);
                if full_snapshot {
                    store.replace_all(replacement);
                } else {
                    store.apply_delta(HashMap::new(), replacement, &HashSet::new());
                }
                let active = store.get_plugin_runtime(&key).expect("runtime remains published");
                if case == "added" {
                    assert_eq!(first_filter(&active).name(), "RejectInvalidConfig");
                } else {
                    // Compare an executable entry, excluding freshly assembled policy gates.
                    let active_entry = active
                        .request_plugins_iter()
                        .find(|p| !p.name().starts_with("PluginPolicy"))
                        .unwrap();
                    let previous_entry = previous_runtime
                        .request_plugins_iter()
                        .find(|p| !p.name().starts_with("PluginPolicy"))
                        .unwrap();
                    assert!(
                        Arc::ptr_eq(active_entry, previous_entry),
                        "{program}/{full_snapshot}/{case}"
                    );
                }
            }
        }
    }
}

#[test]
fn invalid_waf_same_key_add_replaces_last_known_good_and_fails_closed() {
    let key = "default/native-waf".to_string();
    let valid = compile_source(&waf_resource(
        "SecRule REQUEST_URI \"@contains /blocked\" \"id:1,phase:1,deny\"\n",
    ));
    let invalid = compile_source(&waf_resource("SecRule invalid-("));
    let store = PluginStore::new();

    store.apply_delta(
        HashMap::from([(key.clone(), Arc::clone(&valid))]),
        HashMap::new(),
        &HashSet::new(),
    );
    store.apply_delta(
        HashMap::from([(key.clone(), Arc::clone(&invalid))]),
        HashMap::new(),
        &HashSet::new(),
    );

    let snapshot = store.scope();
    let replacement = snapshot.http(&key).expect("invalid add remains stored");
    assert_eq!(replacement.preparse_errors, invalid.preparse_errors);
    assert_leading_reject(&replacement);
    assert!(!replacement.is_activatable);
}

#[test]
fn waf_store_publishes_atomically_and_pins_in_flight_generations() {
    let key = "default/native-waf".to_string();
    let first = compile_source(&waf_resource(
        "SecRule REQUEST_URI \"@contains /first\" \"id:1,phase:1,deny\"\n",
    ));
    let second = compile_source(&waf_resource(
        "SecRule REQUEST_URI \"@contains /second\" \"id:2,phase:1,deny\"\n",
    ));
    let invalid = compile_source(&waf_resource("SecRule invalid-("));
    let recovered = compile_source(&waf_resource(
        "SecRule REQUEST_URI \"@contains /third\" \"id:3,phase:1,deny\"\n",
    ));
    let store = PluginStore::new();

    store.replace_all(HashMap::from([(key.clone(), Arc::clone(&first))]));
    let in_flight_first = store.get_plugin_runtime(&key).expect("first generation is published");

    store.apply_delta(
        HashMap::new(),
        HashMap::from([(key.clone(), Arc::clone(&second))]),
        &HashSet::new(),
    );
    let in_flight_second = store.get_plugin_runtime(&key).expect("second generation is published");
    assert_eq!(in_flight_first.request_plugins_count(), 1);
    assert_eq!(in_flight_second.request_plugins_count(), 1);
    assert!(!Arc::ptr_eq(
        first_filter(&in_flight_first),
        first_filter(&in_flight_second)
    ));
    assert!(!Arc::ptr_eq(&in_flight_first, &in_flight_second));

    store.replace_all(HashMap::from([(key.clone(), invalid)]));
    let retained = store
        .get_plugin_runtime(&key)
        .expect("invalid full snapshot retains the second generation");
    assert!(Arc::ptr_eq(first_filter(&retained), first_filter(&in_flight_second)));

    store.apply_delta(
        HashMap::new(),
        HashMap::from([(key.clone(), Arc::clone(&recovered))]),
        &HashSet::new(),
    );
    let active_recovered = store
        .get_plugin_runtime(&key)
        .expect("recovered generation is published");
    assert!(!Arc::ptr_eq(
        first_filter(&active_recovered),
        first_filter(&in_flight_second)
    ));

    store.apply_delta(HashMap::new(), HashMap::new(), &HashSet::from([key.clone()]));
    assert!(store.get_plugin_runtime(&key).is_none());
    assert_eq!(in_flight_first.request_plugins_count(), 1);
    assert_eq!(in_flight_second.request_plugins_count(), 1);
    assert_eq!(active_recovered.request_plugins_count(), 1);
}

#[test]
fn waf_reload_storm_never_publishes_an_invalid_or_partial_runtime() {
    let key = "default/native-waf".to_string();
    let first = compile_source(&waf_resource(
        "SecRule REQUEST_URI \"@contains /first\" \"id:1,phase:1,deny\"\n",
    ));
    let second = compile_source(&waf_resource(
        "SecRule REQUEST_URI \"@contains /second\" \"id:2,phase:1,deny\"\n",
    ));
    let invalid = compile_source(&waf_resource("SecRule invalid-("));
    let store = Arc::new(PluginStore::new());
    store.replace_all(HashMap::from([(key.clone(), Arc::clone(&first))]));

    std::thread::scope(|scope| {
        for _ in 0..4 {
            let store = Arc::clone(&store);
            let key = key.clone();
            scope.spawn(move || {
                for _ in 0..10_000 {
                    let published = store.scope().http(&key).expect("reload never removes the active WAF");
                    let runtime = &published.plugin_runtime;
                    // Published views drop bundle sources; a clean activatable
                    // view can only come from the first or second generation.
                    assert!(published.is_activatable);
                    assert!(published.preparse_errors.is_empty());
                    assert!(published.spec.spec.waf.as_ref().unwrap().resolved_bundles.is_none());
                    assert_eq!(runtime.request_plugins_count(), 1);
                    assert_eq!(first_filter(runtime).name(), "Waf");
                    assert_eq!(runtime.upstream_response_plugins_count(), 1);
                    assert_eq!(runtime.upstream_response_async_plugins_count(), 1);
                }
            });
        }

        let store = Arc::clone(&store);
        let key = key.clone();
        let first = Arc::clone(&first);
        let second = Arc::clone(&second);
        let invalid = Arc::clone(&invalid);
        scope.spawn(move || {
            for iteration in 0..2_000 {
                let generation = match iteration % 3 {
                    0 => Arc::clone(&first),
                    1 => Arc::clone(&invalid),
                    _ => Arc::clone(&second),
                };
                store.apply_delta(
                    HashMap::new(),
                    HashMap::from([(key.clone(), generation)]),
                    &HashSet::new(),
                );
            }
        });
    });

    let active = store
        .get_plugin_runtime(&key)
        .expect("a complete generation remains after the storm");
    assert_eq!(active.request_plugins_count(), 1);
    assert_eq!(first_filter(&active).name(), "Waf");
}

/// A `ResponseHeaderModifier` plugin placed under `requestPlugins` is a
/// response-stage plugin mis-configured into the request stage. `compile_plugins`
/// must surface the mismatch in `preparse_errors` so the Controller writes it
/// into `status.conditions` — silently dropping it would leave `Accepted: True`
/// even though the plugin never ran (auth-bypass class bug).
#[test]
fn compile_plugins_flags_stage_mismatch() {
    // ResponseHeaderModifier → request stage (wrong stage)
    let request_plugins = vec![RequestFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(
        HTTPHeaderFilter {
            set: None,
            add: None,
            remove: Some(vec!["X-Remove".into()]),
        },
    ))];
    // RequestHeaderModifier → upstream response filter stage (wrong stage)
    let response_filter_plugins = vec![UpstreamResponseFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
        HTTPHeaderFilter {
            set: None,
            add: None,
            remove: Some(vec!["X-Remove".into()]),
        },
    ))];

    let ep = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(request_plugins),
            upstream_response_filter_plugins: Some(response_filter_plugins),
            ..Default::default()
        },
        status: None,
    };

    let compiled = compile_plugins(&ep);
    let errors = &compiled.preparse_errors;
    assert!(
        errors
            .iter()
            .any(|e| e.contains("requestPlugins[0]") && e.contains("not valid for request stage")),
        "expected request-stage mismatch error, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| e.contains("upstreamResponseFilterPlugins[0]")
            && e.contains("not valid for upstream response filter stage")),
        "expected upstream-response-filter-stage mismatch error, got: {:?}",
        errors
    );
}

/// Regression test: valid entries must NOT produce stage-mismatch errors.
#[test]
fn compile_plugins_no_false_positive_on_valid_stages() {
    let request_plugins = vec![RequestFilterEntry::new(header_modifier())];
    let response_filter_plugins = vec![UpstreamResponseFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(
        HTTPHeaderFilter {
            set: None,
            add: None,
            remove: Some(vec!["X-Remove".into()]),
        },
    ))];

    let ep = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(request_plugins),
            upstream_response_filter_plugins: Some(response_filter_plugins),
            ..Default::default()
        },
        status: None,
    };

    let compiled = compile_plugins(&ep);
    assert!(
        !compiled.preparse_errors.iter().any(|e| e.contains("not valid for")),
        "valid stage placement should not produce mismatch errors, got: {:?}",
        compiled.preparse_errors
    );
}

#[test]
fn unresolved_entry_condition_seals_resource_fail_closed() {
    let mut entry = RequestFilterEntry::new(header_modifier());
    entry.conditions = Some(
        serde_json::from_value(serde_json::json!({
            "run": { "allOf": [{
                    "type": "secretMatch",
                    "key": {"type":"header","name":"x-token"},
                    "secretKey": "token",
                    "secretRef": {"name":"missing"},
                    "negate": true
            }] }
        }))
        .unwrap(),
    );
    let ep = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![entry]),
            ..Default::default()
        },
        status: None,
    };

    let compiled = compile_plugins(&ep);
    assert!(!compiled.is_activatable);
    assert!(compiled
        .preparse_errors
        .iter()
        .any(|error| error.contains("conditions contain unresolved references")));
    assert_eq!(compiled.plugin_runtime.request_plugins_count(), 2);
}

/// C1 regression test: a hard-blocklisted `accessLogExtern` declaration must
/// be dropped from the *stored* spec by `compile_plugins`, not merely reported.
/// Without the `retain` in `compile_plugins`, `authorization` would survive
/// into `compiled.spec` and `apply_declarations` would log it in clear
/// whenever `access_log_sensitive.mode` is `off` or `wrap`.
#[test]
fn compile_plugins_drops_blocked_access_log_extern_fields() {
    let ep = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            access_log_extern: Some(vec![
                AccessLogExternField {
                    key: "leak".to_string(),
                    from: LogExtSource::Header,
                    name: "authorization".to_string(),
                },
                AccessLogExternField {
                    key: "tenant".to_string(),
                    from: LogExtSource::Ctx,
                    name: "tenant_id".to_string(),
                },
            ]),
            ..Default::default()
        },
        status: None,
    };

    let compiled = compile_plugins(&ep);

    let stored = compiled.spec.access_log_extern_fields();
    assert_eq!(
        stored.len(),
        1,
        "the blocked field must not survive into the stored spec"
    );
    assert_eq!(stored[0].key, "tenant", "the valid field must be retained");

    assert!(
        compiled
            .preparse_errors
            .iter()
            .any(|e| e.contains("accessLogExtern") && e.contains("authorization") && e.contains("blocked")),
        "expected a preparse error naming the dropped 'authorization' field, got: {:?}",
        compiled.preparse_errors
    );
}

/// Drift guard between the two independent validation implementations.
///
/// Since the controller no longer depends on `edgion-gateway`, it collects
/// the EdgionPlugins K8s-status preparse errors via the schema-layer
/// `EdgionPlugins::validate_plugins` (in `edgion-resources`), while the data
/// plane still produces them as a side effect of `compile_plugins` building
/// the runtime. These are SEPARATE code paths that must stay in lockstep —
/// if `validate_plugins` ever drops an error that `compile_plugins` still
/// reports, the controller would mark a genuinely-broken EdgionPlugins
/// `Accepted: True` (e.g. an auth plugin in the wrong stage that never runs).
///
/// Compared as sorted sets: exact message text and ordering parity are
/// covered by the unit tests in
/// `edgion_resources::resources::edgion_plugins::validate`; this guard's job
/// is the stronger safety property that neither path has an error the other
/// lacks. DSL entries are included: since the edgion-dsl extraction, both
/// paths call the same `DslConfig::validation_error()` pipeline, so they
/// must match exactly.
///
/// **Deliberate exclusion**: `validate_plugins`'s `accessLogExtern` Pass 3
/// also enforces a `LOG_LABEL_MAX_COUNT` cap and a duplicate-`key` check
/// (`edgion-resources/src/resources/edgion_plugins/validate.rs`), which
/// `compile_plugins`'s `retain()` does not mirror and which this guard does
/// not probe. That is intentional, not a gap to close: the data plane is
/// already structurally immune to both without needing to reject anything —
/// `LogExt::insert` caps stored fields at 16 regardless of how many were
/// declared, and a duplicate key resolves last-write-wins in place. So
/// `validate_plugins` over-reports relative to `compile_plugins` on these
/// two rules specifically (controller status flags something the gateway
/// was never going to mis-render), which is the safe direction for the
/// asymmetry to run. Do not add these two rules to `compile_plugins` to
/// "fix" this guard — there is no safety gain, only duplicated logic.
#[test]
fn validate_plugins_matches_compile_plugins_error_set() {
    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }
    fn hf() -> HTTPHeaderFilter {
        HTTPHeaderFilter {
            set: None,
            add: None,
            remove: None,
        }
    }
    // RequestHeaderModifier: request-stage-only. ResponseHeaderModifier:
    // upstream-response-filter-stage-only. Neither has a `validation_error`,
    // so any error comes purely from alias/condition/stage checks.
    fn rhm() -> EdgionPlugin {
        EdgionPlugin::RequestHeaderModifier(hf())
    }
    fn respm() -> EdgionPlugin {
        EdgionPlugin::ResponseHeaderModifier(hf())
    }
    fn dsl(source: &str) -> EdgionPlugin {
        EdgionPlugin::Dsl(DslConfig {
            name: "d".to_string(),
            source: Some(source.to_string()),
            ..Default::default()
        })
    }
    fn rate_limit_local_with_invalid_header() -> EdgionPlugin {
        EdgionPlugin::RateLimitLocal(RateLimitLocalConfig {
            header_names: Some(LimitHeaderNames {
                limit: Some("content-length".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
    fn rate_limit_redis_with_invalid_header() -> EdgionPlugin {
        use edgion_resources::resources::edgion_plugins::{RateLimitAlgorithm, RateLimitPolicy};

        EdgionPlugin::RateLimitRedis(RateLimitRedisConfig {
            quotas: vec![edgion_resources::resources::edgion_plugins::RateLimitQuota {
                policies: vec![RateLimitPolicy {
                    rate: 100,
                    interval: "1s".to_string(),
                    algorithm: RateLimitAlgorithm::default(),
                    interval_duration: None,
                }],
                ..Default::default()
            }],
            redis_ref: "default/shared-redis".to_string(),
            header_names: Some(LimitHeaderNames {
                remaining: Some("transfer-encoding".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
    fn request_body_buffer() -> EdgionPlugin {
        EdgionPlugin::RequestBodyBuffer(Default::default())
    }
    fn json_schema() -> EdgionPlugin {
        use edgion_resources::resources::edgion_plugins::JsonSchemaValidationConfig;
        EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
            schema: serde_json::json!({"type": "object"}),
        })
    }
    fn json_schema_entry_with_body(max_body_size: Option<&str>, with_conditions: bool) -> RequestFilterEntry {
        use edgion_resources::resources::edgion_plugins::BodyRequirement;
        let mut entry = RequestFilterEntry::new(json_schema());
        entry.body = Some(BodyRequirement {
            max_body_size: max_body_size.map(str::to_string),
            conditions: with_conditions.then(Default::default),
            ..Default::default()
        });
        entry
    }
    fn invalid_json_schema_with_invalid_body() -> RequestFilterEntry {
        use edgion_resources::resources::edgion_plugins::JsonSchemaValidationConfig;

        let mut entry = json_schema_entry_with_body(Some("notasize"), false);
        entry.plugin = EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
            schema: serde_json::json!({"$schema": "http://json-schema.org/draft-07/schema#"}),
        });
        entry
    }
    fn invalid_proxy_rewrite_with_invalid_body() -> RequestFilterEntry {
        use edgion_resources::resources::edgion_plugins::{JsonBodyRewrite, ProxyRewriteConfig, RegexUri};

        let mut entry = RequestFilterEntry::new(EdgionPlugin::ProxyRewrite(ProxyRewriteConfig {
            regex_uri: Some(RegexUri {
                pattern: "(".to_string(),
                replacement: "/invalid".to_string(),
            }),
            json_body: Some(JsonBodyRewrite::default()),
            ..Default::default()
        }));
        entry.body = Some(edgion_resources::resources::edgion_plugins::BodyRequirement {
            max_body_size: Some("notasize".to_string()),
            ..Default::default()
        });
        entry
    }
    fn aliased(mut e: RequestFilterEntry, alias: &str) -> RequestFilterEntry {
        e.alias = Some(alias.to_string());
        e
    }
    // Disabled entry carrying a malformed regex condition. Both
    // `compile_plugin_conditions` (runtime) and `validate_plugins` Pass 1
    // (schema) must skip it, so the error set stays empty on both sides.
    fn disabled_with_bad_condition() -> RequestFilterEntry {
        let mut e = RequestFilterEntry::with_enable(rhm(), false);
        e.conditions = Some(
            serde_json::from_value(serde_json::json!({
                "run": { "allOf": [{
                        "type": "keyMatch",
                        "key": { "type": "header", "name": "X-Test" },
                        "regex": ["("]
                }] }
            }))
            .expect("build malformed conditions"),
        );
        e
    }
    fn with_bad_body(mut e: RequestFilterEntry) -> RequestFilterEntry {
        e.body = Some(edgion_resources::resources::edgion_plugins::BodyRequirement {
            max_body_size: Some("notasize".to_string()),
            ..Default::default()
        });
        e
    }
    // A body-consuming plugin (HMAC with validateRequestBody) whose operator
    // `body.maxBodySize` is malformed — exercises the maxBodySize validation parity,
    // distinct from the "plugin does not consume a body" path.
    fn hmac_body_consumer() -> EdgionPlugin {
        use edgion_resources::resources::edgion_plugins::HmacAuthConfig;
        EdgionPlugin::HmacAuth(HmacAuthConfig {
            match_config: edgion_resources::resources::edgion_plugins::HmacMatchConfig {
                validate_request_body: true,
                ..Default::default()
            },
            anonymous: Some("anon".to_string()),
            ..Default::default()
        })
    }
    // A bad regex inside the secondary body-gate conditions — exercises the
    // `body conditions regex` compilation parity (F3).
    fn with_bad_body_regex(mut e: RequestFilterEntry) -> RequestFilterEntry {
        use edgion_resources::resources::edgion_plugins::conditions::{
            Condition, ConditionSet, KeyMatchCondition, PluginConditions,
        };
        e.body = Some(edgion_resources::resources::edgion_plugins::BodyRequirement {
            conditions: Some(PluginConditions {
                skip: None,
                run: Some(ConditionSet::all_of(vec![Condition::KeyMatch(KeyMatchCondition {
                    key: edgion_resources::common::KeyGet::Header { name: "x".to_string() },
                    core: edgion_resources::resources::edgion_plugins::conditions::MatchCore {
                        regex: Some(vec!["[".to_string()]),
                        ..Default::default()
                    },
                    negate: false,
                    descriptor: Box::from(""),
                })])),
            }),
            ..Default::default()
        });
        e
    }
    fn disabled_invalid_json_schema() -> RequestFilterEntry {
        use edgion_resources::resources::edgion_plugins::JsonSchemaValidationConfig;

        let mut entry = json_schema_entry_with_body(Some("notasize"), true);
        entry.plugin = EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
            schema: serde_json::json!({"$schema": "http://json-schema.org/draft-07/schema#"}),
        });
        entry.alias = Some("invalid alias".to_string());
        entry.conditions = disabled_with_bad_condition().conditions;
        entry.enable = false;
        entry
    }
    fn mk(request: Option<Vec<RequestFilterEntry>>, urf: Option<Vec<UpstreamResponseFilterEntry>>) -> EdgionPlugins {
        EdgionPlugins {
            metadata: kube::api::ObjectMeta {
                name: Some("validation-owner".into()),
                ..Default::default()
            },
            spec: EdgionPluginsSpec {
                current_status: None,
                request_plugins: request,
                upstream_response_filter_plugins: urf,
                ..Default::default()
            },
            status: None,
        }
    }
    // C1 case: a hard-blocklisted `accessLogExtern` field. Before the C1 fix,
    // `compile_plugins` never looked at `access_log_extern` at all, so this
    // case failed with `from_runtime` empty and `from_schema` carrying the
    // blocked-name error — exactly the drift class this guard exists to catch.
    fn ep_with_blocked_access_log_extern() -> EdgionPlugins {
        EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                current_status: None,
                access_log_extern: Some(vec![AccessLogExternField {
                    key: "leak".to_string(),
                    from: LogExtSource::Header,
                    name: "authorization".to_string(),
                }]),
                ..Default::default()
            },
            status: None,
        }
    }

    let ai_guard = || {
        let config: edgion_resources::resources::edgion_plugins::AiGuardConfig = serde_yaml::from_str(
            "rules:\n  - name: r\n    match: { type: regex, pattern: \"x\" }\n    action: reject\n",
        )
        .expect("configuration parses");
        RequestFilterEntry::new(EdgionPlugin::AiGuard(config))
    };
    let ai_proxy = || {
        let config: edgion_resources::resources::edgion_plugins::AiProxyConfig =
            serde_yaml::from_str("api: openaiChat").expect("configuration parses");
        RequestFilterEntry::new(EdgionPlugin::AiProxy(config))
    };

    let cases: Vec<(&str, EdgionPlugins)> = vec![
        ("valid_request", mk(Some(vec![RequestFilterEntry::new(rhm())]), None)),
        (
            // A guard ordered after AiProxy can never affect the provider
            // body. Both paths must report the identical message; separate
            // publication tests verify the unconditional fail-closed gate.
            "ai_guard_after_ai_proxy",
            mk(Some(vec![ai_proxy(), ai_guard()]), None),
        ),
        (
            // Two entries of one stage array claiming the same alias → the
            // second is rejected. The alias is part of the entry identity
            // that namespaces external state (RateLimitRedis buckets).
            "duplicate_alias_in_stage",
            mk(
                Some(vec![
                    aliased(RequestFilterEntry::new(rhm()), "dup"),
                    aliased(RequestFilterEntry::new(rhm()), "dup"),
                ]),
                None,
            ),
        ),
        (
            // A bounded-structured-body error owns the entry in BOTH layers, so
            // the entry never claims its alias and the later duplicate is NOT
            // reported. Pins the skip-set parity the alias accumulator depends on.
            //
            // The first entry must have a VALID plugin config (a config error
            // would short-circuit both layers earlier, before the bounded-body
            // skip, making this case vacuous): valid `{"type":"object"}` schema
            // with a malformed `body.maxBodySize`.
            "duplicate_alias_behind_invalid_bounded_body",
            mk(
                Some(vec![
                    aliased(json_schema_entry_with_body(Some("notasize"), false), "dup"),
                    aliased(RequestFilterEntry::new(rhm()), "dup"),
                ]),
                None,
            ),
        ),
        (
            // The same alias in two different stage arrays is fine: the
            // accumulator is per stage.
            "same_alias_across_stages",
            mk(
                Some(vec![aliased(RequestFilterEntry::new(rhm()), "shared")]),
                Some(vec![{
                    let mut e = UpstreamResponseFilterEntry::new(respm());
                    e.alias = Some("shared".to_string());
                    e
                }]),
            ),
        ),
        (
            // `body:` on a plugin that does not consume a body → rejected.
            "body_on_non_consuming_plugin",
            mk(Some(vec![with_bad_body(RequestFilterEntry::new(rhm()))]), None),
        ),
        (
            // `body.maxBodySize` malformed on a body-consuming plugin → maxBodySize error.
            "bad_body_max_body_size_on_consumer",
            mk(
                Some(vec![with_bad_body(RequestFilterEntry::new(hmac_body_consumer()))]),
                None,
            ),
        ),
        (
            // Bad regex in the secondary body-gate conditions → compile error.
            "bad_body_condition_regex",
            mk(Some(vec![with_bad_body_regex(RequestFilterEntry::new(rhm()))]), None),
        ),
        (
            // Disabled entry with a bad body-condition regex → inert, both
            // sides skip it (empty error set), same as disabled_with_bad_condition.
            "disabled_with_bad_body_regex",
            mk(
                Some(vec![with_bad_body_regex(RequestFilterEntry::with_enable(rhm(), false))]),
                None,
            ),
        ),
        (
            "valid_urf",
            mk(None, Some(vec![UpstreamResponseFilterEntry::new(respm())])),
        ),
        (
            "wrong_stage_request",
            mk(Some(vec![RequestFilterEntry::new(respm())]), None),
        ),
        (
            "wrong_stage_urf",
            mk(None, Some(vec![UpstreamResponseFilterEntry::new(rhm())])),
        ),
        (
            "bad_alias_valid_stage",
            mk(Some(vec![aliased(RequestFilterEntry::new(rhm()), "bad alias")]), None),
        ),
        (
            "bad_alias_wrong_stage",
            mk(Some(vec![aliased(RequestFilterEntry::new(respm()), "bad alias")]), None),
        ),
        (
            "disabled_wrong_stage",
            mk(Some(vec![RequestFilterEntry::with_enable(respm(), false)]), None),
        ),
        (
            "disabled_bad_condition",
            mk(Some(vec![disabled_with_bad_condition()]), None),
        ),
        (
            "bad_entry_time_range_condition",
            entry_condition_resource(malformed_time_range_conditions()),
        ),
        (
            "bad_entry_regex_condition",
            entry_condition_resource(bad_regex_conditions()),
        ),
        (
            "bad_body_time_range_condition",
            body_condition_resource(malformed_time_range_conditions()),
        ),
        (
            "bad_body_regex_condition",
            body_condition_resource(bad_regex_conditions()),
        ),
        (
            "bad_dye_time_range_condition",
            dye_condition_resource(malformed_time_range_conditions()),
        ),
        (
            "bad_dye_regex_condition",
            dye_condition_resource(bad_regex_conditions()),
        ),
        (
            "mixed_multi_stage",
            mk(
                Some(vec![RequestFilterEntry::new(rhm()), RequestFilterEntry::new(respm())]),
                Some(vec![
                    UpstreamResponseFilterEntry::new(respm()),
                    UpstreamResponseFilterEntry::new(rhm()),
                ]),
            ),
        ),
        (
            "dsl_valid",
            mk(Some(vec![RequestFilterEntry::new(dsl("let a = 1"))]), None),
        ),
        (
            "request_body_buffer_valid",
            mk(Some(vec![RequestFilterEntry::new(request_body_buffer())]), None),
        ),
        (
            "request_body_buffer_wrong_stage",
            mk(
                None,
                Some(vec![UpstreamResponseFilterEntry::new(request_body_buffer())]),
            ),
        ),
        (
            "json_schema_invalid_body_limit",
            mk(Some(vec![json_schema_entry_with_body(Some("notasize"), false)]), None),
        ),
        (
            "json_schema_zero_body_limit",
            mk(Some(vec![json_schema_entry_with_body(Some("0"), false)]), None),
        ),
        (
            "json_schema_body_limit_above_absolute_max",
            mk(Some(vec![json_schema_entry_with_body(Some("1048577"), false)]), None),
        ),
        (
            "json_schema_body_conditions",
            mk(Some(vec![json_schema_entry_with_body(None, true)]), None),
        ),
        (
            "json_schema_invalid_config_and_body",
            mk(Some(vec![invalid_json_schema_with_invalid_body()]), None),
        ),
        (
            "proxy_rewrite_invalid_config_and_body",
            mk(Some(vec![invalid_proxy_rewrite_with_invalid_body()]), None),
        ),
        (
            "json_schema_two_enabled",
            mk(
                Some(vec![
                    RequestFilterEntry::new(json_schema()),
                    RequestFilterEntry::new(json_schema()),
                ]),
                None,
            ),
        ),
        (
            "json_schema_enabled_and_disabled",
            mk(
                Some(vec![
                    RequestFilterEntry::new(json_schema()),
                    RequestFilterEntry::with_enable(json_schema(), false),
                ]),
                None,
            ),
        ),
        (
            "disabled_invalid_json_schema_is_inert",
            mk(
                Some(vec![
                    disabled_invalid_json_schema(),
                    RequestFilterEntry::new(json_schema()),
                ]),
                None,
            ),
        ),
        (
            "dsl_bad_syntax",
            mk(
                Some(vec![RequestFilterEntry::new(dsl("this is not valid dsl @@@"))]),
                None,
            ),
        ),
        (
            "dsl_unknown_redis_link",
            mk(
                Some(vec![RequestFilterEntry::new(dsl(
                    "let r = redis.get(\"mylink\", \"k\")",
                ))]),
                None,
            ),
        ),
        (
            "rate_limit_local_invalid_response_header",
            mk(
                Some(vec![RequestFilterEntry::new(rate_limit_local_with_invalid_header())]),
                None,
            ),
        ),
        (
            "rate_limit_redis_invalid_response_header",
            mk(
                Some(vec![RequestFilterEntry::new(rate_limit_redis_with_invalid_header())]),
                None,
            ),
        ),
        ("access_log_extern_blocked", ep_with_blocked_access_log_extern()),
        ("waf_blank_rules", waf_resource(" \n\t")),
        ("waf_bad_syntax", waf_resource("SecRule invalid-(")),
    ];

    for (name, ep) in cases {
        let from_runtime = sorted(compile_plugins(&ep).preparse_errors.clone());
        let from_schema = sorted(ep.validate_plugins());
        assert_eq!(
                from_runtime, from_schema,
                "preparse error-set drift between compile_plugins (runtime) and validate_plugins (schema) for case `{name}`"
            );
    }
}

#[test]
fn request_local_reply_status_validation_matches_controller_and_gateway() {
    for status in [100, 101, 199, 200, 599] {
        let cases = vec![
            ("FaultInjection", serde_json::json!({"abort": {"statusCode": status}})),
            ("Mock", serde_json::json!({"statusCode": status})),
            (
                "RequestAccessPolicy",
                serde_json::json!({
                    "defaultProfile": "default",
                    "profiles": {
                        "default": {
                            "requiredRuleGroups": [{
                                "name": "source",
                                "anyOfRules": [{
                                    "name": "loopback",
                                    "type": "directPeerIp",
                                    "config": {"ipRefs": [{"name":"local-peers"}]}
                                }]
                            }]
                        }
                    },
                    "status": status
                }),
            ),
            ("RateLimitLocal", serde_json::json!({"rate": 1, "rejectStatus": status})),
            (
                "RateLimitRedis",
                serde_json::json!({"quotas":[{"name":"default",
                    "policies": [{"rate": 1, "interval": "1s"}],}],
                    "redisRef": "default/redis",
                    "rejectStatus": status
                }),
            ),
            (
                "IpRestriction",
                serde_json::json!({
                    "allowRefs": [{"name": "loopback"}],
                    "status": status
                }),
            ),
            (
                "RequestRestriction",
                serde_json::json!({
                    "mode": "Deny",
                    "conditions": {
                        "allOf": [{
                            "type": "keyMatch",
                            "key": {"type": "method"},
                            "values": ["POST"]
                        }]
                    },
                    "status": status
                }),
            ),
        ];

        for (kind, config) in cases {
            let plugin: EdgionPlugin = serde_json::from_value(serde_json::json!({
                "type": kind,
                "config": config
            }))
            .unwrap();
            let resource = EdgionPlugins {
                metadata: kube::api::ObjectMeta {
                    name: Some("validation-owner".into()),
                    ..Default::default()
                },
                spec: EdgionPluginsSpec {
                    current_status: None,
                    request_plugins: Some(vec![RequestFilterEntry::new(plugin)]),
                    ..Default::default()
                },
                status: None,
            };

            let controller_errors = resource.validate_plugins();
            let gateway_errors = compile_plugins(&resource).preparse_errors.clone();
            assert_eq!(gateway_errors, controller_errors, "{kind} status {status}");
            assert_eq!(
                controller_errors.is_empty(),
                edgion_resources::http_status::is_valid_final_http_status(status),
                "{kind} status {status}: {controller_errors:?}"
            );
        }
    }
}

#[test]
fn responses_guard_capability_rejection_has_shared_validation_and_policy_parity() {
    for order in [vec!["AiGuard", "AiProxy"], vec!["AiProxy", "AiGuard"]] {
        let mut resource = ai_ordering_resource(&order);
        for entry in resource.spec.request_plugins.as_mut().unwrap() {
            if let EdgionPlugin::AiProxy(config) = &mut entry.plugin {
                config.api = edgion_resources::resources::edgion_plugins::AiApi::OpenaiResponses;
            }
        }
        let compiled = compile_plugins(&resource);
        assert_eq!(compiled.preparse_errors, resource.validate_plugins());
        assert!(compiled
            .preparse_errors
            .iter()
            .any(|error| error.contains("native Responses/Messages does not support")));
        assert!(!compiled.is_activatable);
        let bypassed = compile_with_policy(&resource, "AiGuard", Some(PluginPolicyAction::Bypass));
        assert!(bypassed.preparse_errors.is_empty());
        assert!(bypassed.is_activatable);
    }
}

#[test]
fn mirror_invalid_actual_owner_cannot_reuse_cached_entry() {
    let mut source: EdgionPlugins = serde_json::from_value(serde_json::json!({"metadata":{"name":"mirrors","namespace":"app"},"spec":{"requestPlugins":[{"type":"RequestMirror","config":{"backendRef":{"name":"copy","port":80}}}]}})).unwrap();
    let mut cache = crate::plugins::runtime::entry_bundle::EntryCache::default();
    let valid = compile_plugins_with_cache(&source, &mut cache, &[], Vec::new());
    assert!(valid.is_activatable);
    assert_eq!(cache.len(), 1);
    source.metadata.name = Some("invalid/name".into());
    let invalid = compile_plugins_with_cache(&source, &mut cache, &[], Vec::new());
    assert!(!invalid.is_activatable);
    assert!(invalid
        .plugin_runtime
        .request_plugins_iter()
        .all(|filter| filter.name() != "RequestMirror"));
}

fn descriptor_publication_owner(stage: &str, bytes: Option<Vec<u8>>) -> EdgionPlugins {
    let mut wire = serde_json::json!({"metadata":{"name":"descriptor-publication","namespace":"apps"},"spec":{}});
    wire["spec"][stage] = serde_json::json!([{"type":"GrpcJsonTranscoder","config":{
        "descriptor":{"type":"configMapRef","name":"schema","key":"test.pb"},
        "services":["test.TestService"],"resolvedDescriptor":bytes
    }}]);
    serde_json::from_value(wire).unwrap()
}

#[tokio::test]
async fn descriptor_publication_rejects_valid_material_decoys_and_missing_material() {
    let bytes = include_bytes!("../../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb").to_vec();
    let valid = descriptor_publication_owner("requestPlugins", Some(bytes));
    assert!(compile_plugins(&valid).is_activatable);
    for case in ["owner", "namespace", "name", "key", "missing"] {
        let mut owner = valid.clone();
        let EdgionPlugin::GrpcJsonTranscoder(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin
        else {
            panic!()
        };
        let edgion_resources::grpc_transcoding::GrpcDescriptorSource::ConfigMapRef { name, key } =
            &mut config.descriptor
        else {
            panic!()
        };
        match case {
            "owner" => owner.metadata.name = Some("bad/name".into()),
            "namespace" => owner.metadata.namespace = Some("Bad_Namespace".into()),
            "name" => *name = "bad/name".into(),
            "key" => key.clear(),
            _ => config.resolved_descriptor = None,
        }
        let compiled = compile_plugins(&owner);
        assert!(!compiled.is_activatable, "{case}");
        assert!(!compiled.preparse_errors.is_empty());
        let reject = compiled.plugin_runtime.request_plugins_iter().next().unwrap();
        assert_eq!(reject.name(), "RejectInvalidConfig");
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("RejectInvalidConfig");
        assert!(matches!(
            reject.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse { status: 500, .. }
        ));
    }
}

#[test]
fn unsupported_descriptor_stages_preserve_intrinsic_alias_body_and_stage_error_parity() {
    for stage in [
        "upstreamResponseFilterPlugins",
        "upstreamResponseBodyFilterPlugins",
        "upstreamResponsePlugins",
    ] {
        for bytes in [
            None,
            Some(vec![0xff]),
            Some(include_bytes!("../../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb").to_vec()),
        ] {
            let owner = descriptor_publication_owner(stage, bytes);
            let expected = owner.validate_plugins();
            assert!(
                expected.iter().any(|error| error.contains("plugin is not valid for")),
                "{expected:?}"
            );
            assert!(
                !expected.iter().any(|error| error.contains("descriptor")),
                "{expected:?}"
            );
            let compiled = compile_plugins(&owner);
            assert_eq!(compiled.preparse_errors, expected);
            assert!(!compiled.is_activatable);
        }
        let mut wire = serde_json::to_value(descriptor_publication_owner(stage, Some(vec![0xff]))).unwrap();
        wire["spec"][stage][0]["config"]["descriptor"]["name"] = serde_json::json!("bad/name");
        let owner: EdgionPlugins = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(compile_plugins(&owner).preparse_errors, owner.validate_plugins());
        assert!(owner.validate_plugins()[0].ends_with("invalid descriptor ConfigMap reference"));
        wire["spec"][stage][0]["config"]["maxMessageDepth"] = serde_json::json!(0);
        let owner: EdgionPlugins = serde_json::from_value(wire).unwrap();
        assert_eq!(compile_plugins(&owner).preparse_errors, owner.validate_plugins());
        assert!(owner.validate_plugins()[0].contains("message complexity"));
    }
    // Exercise the secondary response body-validation pass without consuming invalid bytes.
    let mut wire = serde_json::to_value(descriptor_publication_owner(
        "upstreamResponsePlugins",
        Some(vec![0xff]),
    ))
    .unwrap();
    wire["spec"]["upstreamResponsePlugins"][0]["body"] = serde_json::json!({"maxBodySize":"1MiB"});
    wire["spec"]["upstreamResponsePlugins"][0]["alias"] = serde_json::json!("bad/name");
    let owner: EdgionPlugins = serde_json::from_value(wire).unwrap();
    let compiled = compile_plugins(&owner);
    let mut expected = owner.validate_plugins();
    let mut actual = compiled.preparse_errors.clone();
    expected.sort();
    actual.sort();
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|error| error.contains("body:")));
    assert!(actual.iter().any(|error| error.contains("alias")));
}

#[test]
fn unsupported_configmap_descriptor_intrinsic_errors_precede_alias_dye_and_stage() {
    for stage in [
        "upstreamResponseFilterPlugins",
        "upstreamResponseBodyFilterPlugins",
        "upstreamResponsePlugins",
    ] {
        for invalid in ["limits", "name", "key", "none"] {
            let mut wire = serde_json::to_value(descriptor_publication_owner(stage, Some(vec![0xff]))).unwrap();
            let entry = &mut wire["spec"][stage][0];
            entry["alias"] = serde_json::json!("bad/name");
            entry["dye"] = serde_json::json!({"response":[{"name":"X-Descriptor","on":["fail-close"]}]});
            match invalid {
                "limits" => entry["config"]["maxMessageDepth"] = serde_json::json!(0),
                "name" => entry["config"]["descriptor"]["name"] = serde_json::json!("bad/name"),
                "key" => entry["config"]["descriptor"]["key"] = serde_json::json!(""),
                _ => {}
            }
            let owner: EdgionPlugins = serde_json::from_value(wire).unwrap();
            let expected = owner.validate_plugins();
            assert_eq!(compile_plugins(&owner).preparse_errors, expected, "{stage}/{invalid}");
            if invalid == "none" {
                assert!(expected.iter().any(|error| error.contains("alias")), "{expected:?}");
                assert!(expected.iter().any(|error| error.contains("dye")), "{expected:?}");
                assert!(
                    expected.iter().any(|error| error.contains("plugin is not valid for")),
                    "{expected:?}"
                );
            } else {
                assert_eq!(expected.len(), 1, "{expected:?}");
                assert!(expected[0].contains(if invalid == "limits" {
                    "message complexity"
                } else {
                    "invalid descriptor ConfigMap reference"
                }));
            }
        }
    }
}

#[test]
fn descriptor_cold_copy_observes_only_authorized_payload_and_preserves_all_entry_fields() {
    let bytes = include_bytes!("../../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb").to_vec();
    let mut owner = descriptor_publication_owner("requestPlugins", Some(bytes.clone()));
    owner.metadata.uid = Some("copy-owner".into());
    owner.metadata.generation = Some(7);
    owner.spec.unknown_fields = vec!["retained-unknown-field".into()];
    let mut invalid = owner.spec.request_plugins.as_ref().unwrap()[0].clone();
    invalid.alias = Some("invalid-descriptor".into());
    let EdgionPlugin::GrpcJsonTranscoder(config) = &mut invalid.plugin else {
        panic!()
    };
    config.descriptor = edgion_resources::grpc_transcoding::GrpcDescriptorSource::ConfigMapRef {
        name: "bad/name".into(),
        key: "test.pb".into(),
    };
    config.descriptor_error = Some("retained diagnostic".into());
    let mut inactive = owner.spec.request_plugins.as_ref().unwrap()[0].clone();
    inactive.enable = false;
    let sibling: RequestFilterEntry = serde_json::from_value(serde_json::json!({
        "type":"RequestHeaderModifier","alias":"sibling","config":{"set":[{"name":"X-Test","value":"kept"}]},
        "conditions":{"skip":{"allOf":[{"type":"keyExist","key":{"type":"header","name":"x-copy-test"}}]}}
    }))
    .unwrap();
    owner
        .spec
        .request_plugins
        .as_mut()
        .unwrap()
        .extend([invalid, inactive, sibling]);
    let response = serde_json::to_value(descriptor_publication_owner(
        "upstreamResponsePlugins",
        Some(bytes.clone()),
    ))
    .unwrap();
    let mut wire = serde_json::to_value(&owner).unwrap();
    wire["spec"]["upstreamResponsePlugins"] = response["spec"]["upstreamResponsePlugins"].clone();
    wire["spec"]["upstreamResponseFilterPlugins"] = response["spec"]["upstreamResponsePlugins"].clone();
    wire["spec"]["upstreamResponseBodyFilterPlugins"] = response["spec"]["upstreamResponsePlugins"].clone();
    owner = serde_json::from_value(wire).unwrap();
    owner.spec.unknown_fields = vec!["retained-unknown-field".into()];
    for demanded in [false, true] {
        let mut copies = Vec::new();
        let copied = clone_for_compilation_with(
            &owner,
            |_| demanded,
            |material| {
                copies.push(material.to_vec());
                material.to_vec()
            },
        );
        assert_eq!(copies, if demanded { vec![bytes.clone()] } else { vec![] });
        let mut expected = serde_json::to_value(&owner).unwrap();
        for stage in [
            "requestPlugins",
            "upstreamResponseFilterPlugins",
            "upstreamResponseBodyFilterPlugins",
            "upstreamResponsePlugins",
        ] {
            for (index, entry) in expected["spec"][stage].as_array_mut().unwrap().iter_mut().enumerate() {
                if entry["type"] == "GrpcJsonTranscoder" && !(stage == "requestPlugins" && index == 0 && demanded) {
                    entry["config"].as_object_mut().unwrap().remove("resolvedDescriptor");
                }
            }
        }
        assert_eq!(serde_json::to_value(&copied).unwrap(), expected);
        assert_eq!(copied.spec.unknown_fields, owner.spec.unknown_fields);
    }
    for invalid_owner in ["name", "namespace"] {
        let mut invalid = owner.clone();
        if invalid_owner == "name" {
            invalid.metadata.name = Some("bad/name".into());
        } else {
            invalid.metadata.namespace = Some("Bad_Namespace".into());
        }
        let mut copies = 0;
        let copied = clone_for_compilation_with(
            &invalid,
            |_| true,
            |_| {
                copies += 1;
                panic!("invalid owner reached payload copy")
            },
        );
        assert_eq!(copies, 0);
        let EdgionPlugin::GrpcJsonTranscoder(config) = &copied.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!()
        };
        assert!(config.resolved_descriptor.is_none());
    }
}

#[test]
fn waf_resource_policy_identity_is_checked_before_cached_construction() {
    let mut resource = EdgionPlugins::new("waf-owner", EdgionPluginsSpec::default());
    resource.metadata.namespace = Some("default".into());
    resource.spec.waf = Some(crate::plugins::http::waf::fixture::config_in_namespace(
        "SecAction \"id:9861,phase:1,pass\"\n",
        "default",
    ));
    let mut cache = EntryCache::default();
    let valid = compile_plugins_with_cache(&resource, &mut cache, &[], Vec::new());
    assert!(valid.plugin_runtime.waf_candidate().is_some());
    for namespace in ["", "bad/ns", "Bad"] {
        resource.spec.waf.as_mut().unwrap().policy_ref.namespace = Some(namespace.into());
        let invalid = compile_plugins_with_cache(&resource, &mut cache, &[], Vec::new());
        assert!(invalid.plugin_runtime.waf_candidate().is_none());
        assert!(invalid.can_retain_previous(&valid));
    }
}

#[tokio::test]
async fn waf_selection_only_update_keeps_sources_and_in_flight_filter_behavior() {
    use crate::ctx::WafPhaseClaim;
    use crate::EdgionHttpContext;
    let mut before = waf_resource(concat!(
        "SecRule REQUEST_URI \"@contains /probe\" \"id:100,phase:1,deny,status:403,tag:company/a\"\n",
        "SecRule REQUEST_URI \"@contains /probe\" \"id:101,phase:1,deny,status:409,tag:company/b\"\n",
    ));
    let set_selection = |resource: &mut EdgionPlugins, tag: &str| {
        resource
            .spec
            .waf
            .as_mut()
            .unwrap()
            .resolved_policy
            .as_mut()
            .unwrap()
            .profiles
            .get_mut("default")
            .unwrap()
            .rule_selection = serde_json::from_value(serde_json::json!({"include": {"tag": tag}})).unwrap();
    };
    set_selection(&mut before, "company/a");
    let mut after = before.clone();
    set_selection(&mut after, "company/b");
    let before_bundle = &before.spec.waf.as_ref().unwrap().resolved_bundles.as_ref().unwrap()[0];
    let after_bundle = &after.spec.waf.as_ref().unwrap().resolved_bundles.as_ref().unwrap()[0];
    assert_eq!(before_bundle.digest, after_bundle.digest);
    assert_eq!(before_bundle.data.rules[0].content, after_bundle.data.rules[0].content);
    let key = "default/native-waf".to_string();
    let first = compile_source(&before);
    let second = compile_source(&after);
    assert!(first.is_activatable, "{:?}", first.preparse_errors);
    assert!(second.is_activatable, "{:?}", second.preparse_errors);
    let store = PluginStore::new();
    store.replace_all(HashMap::from([(key.clone(), first)]));
    let old = store.get_plugin_runtime(&key).unwrap();
    store.apply_delta(HashMap::new(), HashMap::from([(key.clone(), second)]), &HashSet::new());
    let new = store.get_plugin_runtime(&key).unwrap();
    assert!(!Arc::ptr_eq(first_filter(&old), first_filter(&new)));
    // Both filters run after replacement, making immutable old-generation behavior observable.
    for (runtime, expected) in [(&old, 403), (&new, 409)] {
        let mut session = MockPluginSession::new();
        let mut waf = MockWafSessionCapability::new();
        session.expect_ctx().times(1).return_const(EdgionHttpContext::default());
        waf.expect_claim_phase().returning(|_, _| WafPhaseClaim::Execute);
        waf.expect_record_phase_outcome().returning(|_, _, _, _| ());
        waf.expect_record_terminal_error().returning(|_, _, _, _| ());
        session.expect_request_header_map().return_const(http::HeaderMap::new());
        session.expect_get_method().times(1).return_const("POST".to_string());
        session.expect_get_uri().return_const("/probe".to_string());
        session.expect_get_request_uri().return_const("/probe".to_string());
        session.expect_get_http_protocol().return_const("HTTP/1.1".to_string());
        session.expect_remote_ip().return_const("192.0.2.1".to_string());
        session.expect_get_request_id().return_const("request-1".to_string());
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
        let result = first_filter(runtime)
            .run_request(&mut session, &mut PluginLog::new("Waf"))
            .await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: expected,
                body: None
            }
        );
    }
    let mut invalid = after.clone();
    invalid.spec.waf.as_mut().unwrap().resolved_bundles = None;
    store.apply_delta(
        HashMap::new(),
        HashMap::from([(key.clone(), compile_source(&invalid))]),
        &HashSet::new(),
    );
    assert!(Arc::ptr_eq(
        first_filter(&new),
        first_filter(&store.get_plugin_runtime(&key).unwrap())
    ));
}

fn fixed_link_sys_resource(kind: &str, response: bool) -> EdgionPlugins {
    let config = match kind {
        "Dsl" => {
            serde_json::json!({"name":"program","source":"return next()","linkSysRefs":[{"namespace":"raw/ns","name":"redis_alias"}]})
        }
        "RateLimitRedis" => {
            serde_json::json!({"redisRef":"apps/cache","quotas":[{"name":"default","key":[],"policies":[{"rate":100,"interval":"1s"}]}]})
        }
        "GraphqlRateLimit" => {
            serde_json::json!({"schema":"type Query { name: String }","redisRef":"apps/cache","policies":[{"rate":100,"interval":"1s"}]})
        }
        "WebhookKeyGet" => {
            serde_json::json!({"vars":[{"name":"result","webhookRef":"apps/hook","extract":{"source":"bodyText"}}]})
        }
        "AiProxy" => {
            serde_json::json!({"api":"openaiChat","usageObservation":"provider","tokenQuota":{"redisRef":"apps/cache","key":[],"limits":[{"tokens":100,"window":"1h"}]}})
        }
        _ => unreachable!(),
    };
    let stage = if response {
        "upstreamResponseFilterPlugins"
    } else {
        "requestPlugins"
    };
    serde_json::from_value(serde_json::json!({"metadata":{"namespace":"apps","name":"fixed-links"},"spec":{stage:[{"type":kind,"config":config}]}})).unwrap()
}

#[tokio::test]
async fn fixed_link_sys_actual_sources_gate_all_factories_but_preserve_masks() {
    for (kind, response) in [
        ("Dsl", false),
        ("Dsl", true),
        ("RateLimitRedis", false),
        ("GraphqlRateLimit", false),
        ("WebhookKeyGet", false),
        ("AiProxy", false),
    ] {
        let owner = fixed_link_sys_resource(kind, response);
        let valid = compile_plugins(&owner);
        assert!(valid.is_activatable, "{kind}/{response}: {:?}", valid.preparse_errors);
        let mut native = owner.clone();
        native.metadata.namespace = None;
        assert!(
            compile_plugins(&native).is_activatable,
            "omitted native namespace: {kind}"
        );
        for namespace in ["", "bad/ns"] {
            let mut invalid = owner.clone();
            invalid.metadata.namespace = Some(namespace.into());
            let compiled = compile_plugins(&invalid);
            assert!(!compiled.is_activatable, "{kind}/{response}");
            assert!(!compiled.retain_previous_on_invalid_update);
            assert!(compiled
                .preparse_errors
                .iter()
                .any(|error| error.contains("invalid owning LinkSys reference")));
            let reject = compiled.plugin_runtime.request_plugins_iter().next().unwrap();
            let mut session = MockPluginSession::new();
            let mut log = PluginLog::new(reject.name());
            assert!(matches!(
                reject.run_request(&mut session, &mut log).await,
                PluginRunningResult::ErrResponse { status: 500, .. }
            ));
            let blocked = compile_with_policy(&invalid, kind, Some(PluginPolicyAction::Block { status: 451 }));
            assert_eq!(
                blocked.plugin_runtime.request_plugins_iter().next().unwrap().name(),
                "PluginPolicyBlock"
            );
            let bypass = compile_with_policy(&invalid, kind, Some(PluginPolicyAction::Bypass));
            assert!(
                bypass.is_activatable,
                "masked {kind}/{response}: {:?}",
                bypass.preparse_errors
            );
            if response {
                invalid.spec.upstream_response_filter_plugins.as_mut().unwrap()[0].enable = false;
            } else {
                invalid.spec.request_plugins.as_mut().unwrap()[0].enable = false;
            }
            assert!(compile_plugins(&invalid).is_activatable);
        }
        let mut invalid = owner;
        invalid.metadata.name = None;
        assert!(!compile_plugins(&invalid).is_activatable);
    }
}

#[tokio::test]
async fn fixed_link_sys_target_grammars_remain_consumer_specific() {
    for kind in ["RateLimitRedis", "GraphqlRateLimit", "WebhookKeyGet", "AiProxy"] {
        let mut owner = fixed_link_sys_resource(kind, false);
        let entry = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin;
        let key = match entry {
            EdgionPlugin::RateLimitRedis(config) => &mut config.redis_ref,
            EdgionPlugin::GraphqlRateLimit(config) => &mut config.redis_ref,
            EdgionPlugin::WebhookKeyGet(config) => &mut config.vars[0].webhook_ref,
            EdgionPlugin::AiProxy(config) => &mut config.token_quota.as_mut().unwrap().redis_ref,
            _ => unreachable!(),
        };
        *key = "ns_name/cache_key".into();
        assert!(compile_plugins(&owner).is_activatable, "underscore {kind}");
        let mut value = serde_json::to_value(&owner).unwrap();
        let config = &mut value["spec"]["requestPlugins"][0]["config"];
        if kind == "WebhookKeyGet" {
            config["vars"][0]["webhookRef"] = serde_json::json!("café/cache");
        } else if kind == "AiProxy" {
            config["tokenQuota"]["redisRef"] = serde_json::json!("café/cache");
        } else {
            config["redisRef"] = serde_json::json!("café/cache");
        }
        let unicode: EdgionPlugins = serde_json::from_value(value).unwrap();
        assert_eq!(compile_plugins(&unicode).is_activatable, kind != "AiProxy", "{kind}");
    }
    for response in [false, true] {
        assert!(
            compile_plugins(&fixed_link_sys_resource("Dsl", response)).is_activatable,
            "DSL namespace grammar stays unrestricted"
        );
    }
}

#[tokio::test]
async fn fixed_link_sys_dsl_invalid_owner_prevents_retention_without_changing_script_policy() {
    for response in [false, true] {
        let owner = fixed_link_sys_resource("Dsl", response);
        let valid = compile_source(&owner);
        let mut bad_script = owner.clone();
        let plugin = if response {
            &mut bad_script.spec.upstream_response_filter_plugins.as_mut().unwrap()[0].plugin
        } else {
            &mut bad_script.spec.request_plugins.as_mut().unwrap()[0].plugin
        };
        let EdgionPlugin::Dsl(config) = plugin else {
            unreachable!()
        };
        config.source = Some("invalid @@@".into());
        let invalid_script = compile_source(&bad_script);
        assert!(invalid_script.retain_previous_on_invalid_update);
        assert!(invalid_script.can_retain_previous(&valid));
        bad_script.metadata.namespace = Some(String::new());
        let invalid_source = compile_source(&bad_script);
        assert!(!invalid_source.retain_previous_on_invalid_update);
        assert!(!invalid_source.can_retain_previous(&valid));
        let store = PluginStore::new();
        let key = "apps/fixed-links".to_string();
        store.apply_delta(HashMap::from([(key.clone(), valid)]), HashMap::new(), &HashSet::new());
        let old = store.get_plugin_runtime(&key).unwrap();
        store.apply_delta(
            HashMap::new(),
            HashMap::from([(key.clone(), invalid_script)]),
            &HashSet::new(),
        );
        let retained = store.get_plugin_runtime(&key).unwrap();
        assert!(store.scope().http(&key).unwrap().is_activatable);
        if response {
            assert!(Arc::ptr_eq(
                old.upstream_response_plugins_iter().next().unwrap(),
                retained.upstream_response_plugins_iter().next().unwrap()
            ));
        } else {
            assert!(Arc::ptr_eq(
                old.request_plugins_iter().next().unwrap(),
                retained.request_plugins_iter().next().unwrap()
            ));
        }
        store.apply_delta(
            HashMap::new(),
            HashMap::from([(key.clone(), invalid_source)]),
            &HashSet::new(),
        );
        let latest = store.scope();
        assert!(!latest.http(&key).unwrap().is_activatable);
        assert!(!Arc::ptr_eq(&old, &store.get_plugin_runtime(&key).unwrap()));
        let closed = store.get_plugin_runtime(&key).unwrap();
        let reject = closed.request_plugins_iter().next().unwrap();
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new(reject.name());
        assert!(matches!(
            reject.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse { status: 500, .. }
        ));
    }
}

#[test]
fn all_endpoint_method_validation_has_ordered_controller_gateway_parity() {
    let mut resource: EdgionPlugins = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"name": "probe-methods", "namespace": "default"},
        "spec": {"requestPlugins": [
            {"type": "AllEndpointStatus", "enable": false, "config": {"methodOverride": "BAD METHOD"}},
            {"type": "AllEndpointStatus", "config": {"methodOverride": "BAD METHOD"}},
            {"type": "AllEndpointStatus", "config": {"methodOverride": "", "timeoutMs": 0}},
            {"type": "AllEndpointStatus", "config": {"methodOverride": "post"}}
        ]}
    }))
    .unwrap();
    let errors = resource.validate_plugins();
    let compiled = compile_plugins(&resource);
    assert_eq!(compiled.preparse_errors, errors);
    assert_eq!(errors.len(), 2);
    assert!(errors[0].starts_with("requestPlugins[1]"));
    assert!(errors[0].contains("methodOverride must be a valid HTTP method"));
    assert!(errors[1].starts_with("requestPlugins[2]"));
    assert!(errors[1].contains("timeoutMs must be > 0"));
    assert!(!compiled.is_activatable);
    assert_leading_reject(&compiled);
    resource.spec.request_plugins.as_mut().unwrap().drain(1..3);
    assert!(resource.validate_plugins().is_empty());
    let compiled = compile_plugins(&resource);
    assert!(compiled.preparse_errors.is_empty());
    assert!(compiled.is_activatable);
}

#[test]
fn wasm_callout_rejection_is_redacted_and_fails_closed() {
    for raw in [
        "https://private-user:private-password@auth.example",
        "https://auth.example?token=private-token",
        "https://auth.example/private-path#private-fragment",
        "private-invalid-url",
        "ftp://auth.example/private-path",
    ] {
        let resource: EdgionPlugins = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "wasm-redaction", "namespace": "default"},
            "spec": {"requestPlugins": [{"type": "Wasm", "config": {
                "source": {"file": "./demo.wasm"}, "calloutAllowlist": [raw]
            }}]}
        }))
        .unwrap();
        let errors = resource.validate_plugins();
        let compiled = compile_plugins(&resource);
        assert_eq!(errors.len(), 1);
        assert_eq!(compiled.preparse_errors, errors);
        assert!(errors[0].starts_with("requestPlugins[0] (Wasm): calloutAllowlist:"));
        assert!(!errors[0].contains("private-"));
        assert!(!compiled.is_activatable);
        assert_leading_reject(&compiled);
    }
}

#[tokio::test]
async fn authentication_header_names_match_transport_admission_and_fail_closed() {
    for name in [
        "private name",
        "private\tname",
        "private:name",
        "private-é",
        "X-Mixed!#$%&'*+-.^_`|~09",
    ] {
        let valid = name.starts_with("X-Mixed");
        let mut request = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
        assert_eq!(request.insert_header(name.to_string(), "identity").is_ok(), valid);
        for (kind, config) in [
            (
                "HeaderCertAuth",
                serde_json::json!({"mode": "Connection", "upstreamHeaders": {"consumerHeader": name}}),
            ),
            (
                "HmacAuth",
                serde_json::json!({"anonymous": "anon", "upstreamHeaderFields": [name]}),
            ),
        ] {
            let mut resource: EdgionPlugins = serde_json::from_value(serde_json::json!({
                "metadata": {"name": "auth-header-names", "namespace": "default"},
                "spec": {"requestPlugins": [
                    {"type": kind, "enable": false, "config": config},
                    {"type": kind, "config": config}
                ]}
            }))
            .unwrap();
            let errors = resource.validate_plugins();
            let compiled = compile_plugins(&resource);
            assert_eq!(compiled.preparse_errors, errors);
            assert_eq!(compiled.is_activatable, valid);
            if !valid {
                assert_eq!(errors.len(), 1);
                assert!(errors[0].starts_with(&format!("requestPlugins[1] ({kind}):")));
                assert!(!errors[0].contains("private"));
                assert_leading_reject(&compiled);
                let reject = first_filter(&compiled.plugin_runtime);
                let mut session = MockPluginSession::new();
                let mut log = PluginLog::new(reject.name());
                assert!(matches!(
                    reject.run_request(&mut session, &mut log).await,
                    PluginRunningResult::ErrResponse { status: 500, .. }
                ));
            }
            resource.spec.request_plugins.as_mut().unwrap()[1].enable = false;
            assert!(resource.validate_plugins().is_empty());
            assert!(compile_plugins(&resource).is_activatable);
        }
    }
}

#[test]
fn local_rate_limit_extreme_estimator_capacity_publishes_real_runtime() {
    use edgion_resources::rate_limit_local_config::SLOTS_K;
    for slots_k in [usize::MAX / SLOTS_K + 1, usize::MAX] {
        let resource: EdgionPlugins = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "local-capacity", "namespace": "default"},
            "spec": {"requestPlugins": [{"type": "RateLimitLocal", "config": {
                "rate": 10, "estimatorSlotsK": slots_k
            }}]}
        }))
        .unwrap();
        assert!(resource.validate_plugins().is_empty());
        let compiled = compile_plugins(&resource);
        assert!(compiled.preparse_errors.is_empty());
        assert!(compiled.is_activatable);
        assert_eq!(first_filter(&compiled.plugin_runtime).name(), "RateLimitLocal");
    }
}
