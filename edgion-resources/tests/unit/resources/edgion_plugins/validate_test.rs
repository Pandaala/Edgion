use super::*;
use crate::resources::common::SecretObjectReference;
use crate::resources::edgion_plugins::plugin_configs::{
    CredentialInjectionTarget, CredentialInjectorConfig, DslConfig, FormJsonDirection, FormJsonTransformConfig,
    JsonSchemaValidationConfig, WasmConfig, WasmRawConfig, MAX_WASM_RAW_CONFIG_BYTES,
};
use crate::resources::edgion_plugins::{
    BodyReadFailureMode, BodyRequirement, EdgionPluginsSpec, RequestFilterEntry, UpstreamResponseFilterEntry,
    ABSOLUTE_INSTANCE_MAX_BYTES, DEFAULT_INSTANCE_MAX_BYTES,
};
use crate::resources::http_route::HTTPHeaderFilter;
use serde_json::json;

fn header_filter() -> HTTPHeaderFilter {
    HTTPHeaderFilter {
        set: None,
        add: None,
        remove: None,
    }
}

fn ai_guard_entry() -> RequestFilterEntry {
    let config: crate::resources::edgion_plugins::AiGuardConfig =
        serde_yaml::from_str("rules:\n  - name: r\n    match: { type: regex, pattern: \"x\" }\n    action: reject\n")
            .expect("configuration parses");
    RequestFilterEntry::new(EdgionPlugin::AiGuard(config))
}

fn ai_proxy_entry() -> RequestFilterEntry {
    let config: crate::resources::edgion_plugins::AiProxyConfig =
        serde_yaml::from_str("{}").expect("configuration parses");
    RequestFilterEntry::new(EdgionPlugin::AiProxy(config))
}

fn link_sys_credential_injector() -> EdgionPlugin {
    EdgionPlugin::CredentialInjector(CredentialInjectorConfig {
        credential_ref: SecretObjectReference {
            group: Some("edgion.io".to_string()),
            kind: Some("LinkSys".to_string()),
            name: "oauth-source".to_string(),
            namespace: None,
        },
        target: CredentialInjectionTarget::Bearer,
        resolved_credential: None,
    })
}

#[test]
fn multiple_active_link_sys_credential_injectors_across_stages_are_rejected() {
    let mut plugins = with_request_plugins(vec![RequestFilterEntry::new(link_sys_credential_injector())]);
    plugins.spec.upstream_response_filter_plugins =
        Some(vec![UpstreamResponseFilterEntry::new(link_sys_credential_injector())]);

    let errors = plugins.validate_plugins();
    assert!(errors
        .iter()
        .any(|error| error == "EdgionPlugins may contain at most one enabled LinkSys-backed CredentialInjector"));
}

#[test]
fn inactive_link_sys_credential_injectors_do_not_count_toward_resource_limit() {
    let active = RequestFilterEntry::new(link_sys_credential_injector());
    let disabled = RequestFilterEntry::with_enable(link_sys_credential_injector(), false);
    let plugins = with_request_plugins(vec![active, disabled]);

    assert!(!plugins
        .validate_plugins()
        .iter()
        .any(|error| error.contains("at most one enabled LinkSys-backed")));
}

#[test]
fn ai_guard_ordered_after_ai_proxy_is_reported() {
    // A guard that runs after AiProxy can never affect the provider body,
    // because the provider request is derived from the canonical body
    // AiProxy already captured.
    let mut errors = Vec::new();
    collect_ai_guard_ordering_errors(&[ai_proxy_entry(), ai_guard_entry()], &mut errors);
    assert_eq!(errors, vec![AI_GUARD_AFTER_AI_PROXY_ERROR.to_string()]);

    let mut ok = Vec::new();
    collect_ai_guard_ordering_errors(&[ai_guard_entry(), ai_proxy_entry()], &mut ok);
    assert!(ok.is_empty(), "guard before proxy is the supported order");
}

#[test]
fn ordering_rule_ignores_inert_entries_and_single_sided_lists() {
    // A disabled entry is inert and must not trigger a status condition.
    let mut disabled = ai_guard_entry();
    disabled.enable = false;
    let mut errors = Vec::new();
    collect_ai_guard_ordering_errors(&[ai_proxy_entry(), disabled], &mut errors);
    assert!(errors.is_empty());

    // Either plugin alone is a perfectly valid configuration.
    let mut errors = Vec::new();
    collect_ai_guard_ordering_errors(&[ai_guard_entry()], &mut errors);
    collect_ai_guard_ordering_errors(&[ai_proxy_entry()], &mut errors);
    assert!(errors.is_empty());
}

fn aliased_request_entry(alias: &str) -> RequestFilterEntry {
    let mut entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(header_filter()));
    entry.alias = Some(alias.to_string());
    entry
}

#[test]
fn duplicate_alias_in_one_stage_is_rejected() {
    let ep = with_request_plugins(vec![aliased_request_entry("dup"), aliased_request_entry("dup")]);
    let errors = ep.validate_plugins();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("requestPlugins[1] "), "{errors:?}");
    assert!(errors[0].contains("duplicate alias 'dup'"), "{errors:?}");
}

#[test]
fn distinct_aliases_in_one_stage_are_accepted() {
    let ep = with_request_plugins(vec![aliased_request_entry("a"), aliased_request_entry("b")]);
    assert!(ep.validate_plugins().is_empty());
}

#[test]
fn bounded_body_invalid_entry_does_not_claim_its_alias() {
    // Mirrors the gateway prepass: a bounded-structured-body error owns the
    // entry, so it never reaches the alias check and a later entry reusing
    // that alias is NOT a duplicate. Without the skip the two validation
    // layers would disagree on this input.
    use crate::resources::edgion_plugins::BodyRequirement;

    let mut first = RequestFilterEntry::new(EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
        schema: json!({"type": "object"}),
    }));
    first.alias = Some("dup".to_string());
    first.body = Some(BodyRequirement {
        max_body_size: Some("notasize".to_string()),
        ..Default::default()
    });

    let errors = with_request_plugins(vec![first, aliased_request_entry("dup")]).validate_plugins();

    assert!(
        errors.iter().any(|e| e.contains("maxBodySize")),
        "the bounded-body error must still be reported: {errors:?}"
    );
    assert!(
        !errors.iter().any(|e| e.contains("duplicate alias")),
        "a skipped entry must not claim its alias: {errors:?}"
    );
}

#[test]
fn duplicate_alias_on_a_disabled_entry_is_ignored() {
    // Inert entries are never built, so their alias claims nothing.
    let mut disabled = aliased_request_entry("dup");
    disabled.enable = false;
    let ep = with_request_plugins(vec![disabled, aliased_request_entry("dup")]);
    assert!(ep.validate_plugins().is_empty());
}

fn with_request_plugins(entries: Vec<RequestFilterEntry>) -> EdgionPlugins {
    EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(entries),
            ..Default::default()
        },
        status: None,
    }
}

#[test]
fn controller_validate_plugins_matches_wasm_raw_config_validation() {
    let mut config = WasmConfig::default();
    config.source.file = Some("./demo.wasm".to_string());
    config.plugin_config = Some(WasmRawConfig::base64("/wA="));
    let valid = RequestFilterEntry::new(EdgionPlugin::Wasm(config.clone()));
    assert!(with_request_plugins(vec![valid]).validate_plugins().is_empty());

    config.plugin_config = Some(WasmRawConfig::string("x".repeat(MAX_WASM_RAW_CONFIG_BYTES + 1)));
    let invalid = RequestFilterEntry::new(EdgionPlugin::Wasm(config));
    let errors = with_request_plugins(vec![invalid]).validate_plugins();
    assert!(
        errors
            .iter()
            .any(|error| error.contains("pluginConfig decoded value exceeds the 64KiB limit")),
        "{errors:?}"
    );
}

fn dsl_entry(source: &str) -> RequestFilterEntry {
    RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
        name: "d".to_string(),
        source: Some(source.to_string()),
        ..Default::default()
    }))
}

#[test]
fn body_on_non_consuming_plugin_is_rejected() {
    use crate::resources::edgion_plugins::BodyRequirement;
    let mut entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(header_filter()));
    entry.body = Some(BodyRequirement::fail_close());
    let errs = with_request_plugins(vec![entry]).validate_plugins();
    assert!(
        errs.iter().any(|e| e.contains("body: plugin does not consume a body")),
        "expected 'does not consume a body', got: {errs:?}"
    );
}

#[test]
fn bad_body_max_size_on_consuming_plugin_is_flagged() {
    use crate::resources::edgion_plugins::{BodyRequirement, HmacAuthConfig};
    let cfg = HmacAuthConfig {
        match_config: crate::resources::edgion_plugins::HmacMatchConfig {
            validate_request_body: true,
            ..Default::default()
        },
        anonymous: Some("anon".to_string()),
        ..Default::default()
    };
    let mut entry = RequestFilterEntry::new(EdgionPlugin::HmacAuth(cfg));
    entry.body = Some(BodyRequirement {
        max_body_size: Some("notasize".to_string()),
        ..Default::default()
    });
    let errs = with_request_plugins(vec![entry]).validate_plugins();
    assert!(
        errs.iter().any(|e| e.contains("body: invalid body.maxBodySize")),
        "expected a maxBodySize error, got: {errs:?}"
    );
}

#[test]
fn valid_body_on_consuming_plugin_has_no_error() {
    use crate::resources::edgion_plugins::{BodyRequirement, HmacAuthConfig};
    let cfg = HmacAuthConfig {
        match_config: crate::resources::edgion_plugins::HmacMatchConfig {
            validate_request_body: true,
            ..Default::default()
        },
        anonymous: Some("anon".to_string()),
        ..Default::default()
    };
    let mut entry = RequestFilterEntry::new(EdgionPlugin::HmacAuth(cfg));
    entry.body = Some(BodyRequirement {
        max_body_size: Some("1m".to_string()),
        ..Default::default()
    });
    assert!(with_request_plugins(vec![entry]).validate_plugins().is_empty());
}

fn json_schema_entry() -> RequestFilterEntry {
    RequestFilterEntry::new(EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
        schema: json!({"type": "object"}),
    }))
}

fn form_json_entry() -> RequestFilterEntry {
    RequestFilterEntry::new(EdgionPlugin::FormJsonTransform(FormJsonTransformConfig {
        direction: FormJsonDirection::FormToJson,
    }))
}

#[test]
fn form_json_transform_defaults_to_bounded_fail_close_request_body() {
    let entry = form_json_entry();
    let intrinsic = entry.plugin.body_requirement(BodyStage::Request).unwrap();
    assert_eq!(intrinsic.max_body_size_bytes(), Some(Ok(DEFAULT_INSTANCE_MAX_BYTES)));
    assert!(intrinsic.is_fail_close());
    assert!(intrinsic.conditions.is_none());
    assert!(entry.plugin.body_requirement(BodyStage::Response).is_none());
}

#[test]
fn form_json_transform_rejects_conditions_fail_open_and_oversized_limit() {
    let mut entry = form_json_entry();
    entry.body = Some(BodyRequirement {
        max_body_size: Some((ABSOLUTE_INSTANCE_MAX_BYTES + 1).to_string()),
        conditions: Some(Default::default()),
        on_read_failure: BodyReadFailureMode::FailOpen,
    });
    let errors = with_request_plugins(vec![entry]).validate_plugins();
    for expected in [
        "conditions are not supported",
        "body.onReadFailure must be failClose",
        "must not exceed 1MiB",
    ] {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "missing {expected:?}: {errors:?}"
        );
    }
}

#[test]
fn form_json_transform_is_request_stage_only() {
    let entry = UpstreamResponseFilterEntry::new(form_json_entry().plugin);
    let plugins = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            upstream_response_filter_plugins: Some(vec![entry]),
            ..Default::default()
        },
        status: None,
    };
    let errors = plugins.validate_plugins();
    assert!(
        errors
            .iter()
            .any(|error| error.contains("plugin is not valid for upstream response filter stage")),
        "{errors:?}"
    );
}

#[test]
fn json_schema_intrinsic_body_limit_defaults_to_64_kib() {
    let entry = json_schema_entry();
    let intrinsic = entry.plugin.body_requirement(BodyStage::Request).unwrap();
    assert_eq!(intrinsic.max_body_size_bytes(), Some(Ok(DEFAULT_INSTANCE_MAX_BYTES)));

    let operator = BodyRequirement {
        on_read_failure: BodyReadFailureMode::FailOpen,
        ..Default::default()
    };
    let resolved = BodyRequirement::resolve(Some(operator), Some(intrinsic)).unwrap();
    assert_eq!(resolved.max_body_size_bytes(), Some(Ok(DEFAULT_INSTANCE_MAX_BYTES)));
    assert!(resolved.is_fail_close());
}

#[test]
fn json_schema_body_policy_rejects_conditions_and_limit_above_1_mib() {
    let mut entry = json_schema_entry();
    entry.body = Some(BodyRequirement {
        max_body_size: Some((ABSOLUTE_INSTANCE_MAX_BYTES + 1).to_string()),
        conditions: Some(Default::default()),
        ..Default::default()
    });
    let errors = with_request_plugins(vec![entry]).validate_plugins();
    assert!(
        errors
            .iter()
            .any(|error| error.contains("conditions are not supported")),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|error| error.contains("must not exceed 1MiB")),
        "{errors:?}"
    );
}

#[test]
fn json_schema_body_policy_rejects_invalid_and_zero_limits() {
    for limit in ["notasize", "0"] {
        let mut entry = json_schema_entry();
        entry.body = Some(BodyRequirement {
            max_body_size: Some(limit.to_string()),
            ..Default::default()
        });
        let errors = with_request_plugins(vec![entry]).validate_plugins();
        assert!(
            errors.iter().any(|error| error.contains("body.maxBodySize")),
            "limit {limit:?} should fail: {errors:?}"
        );
    }
}

#[test]
fn json_schema_body_limit_allows_exactly_1_mib() {
    let mut entry = json_schema_entry();
    entry.body = Some(BodyRequirement {
        max_body_size: Some(ABSOLUTE_INSTANCE_MAX_BYTES.to_string()),
        ..Default::default()
    });
    assert!(with_request_plugins(vec![entry]).validate_plugins().is_empty());
}

#[test]
fn rejects_multiple_json_schema_plugins_in_one_request_list() {
    let errors = with_request_plugins(vec![json_schema_entry(), json_schema_entry()]).validate_plugins();
    assert!(
        errors
            .iter()
            .any(|error| error.contains("at most one JsonSchemaValidation")),
        "{errors:?}"
    );
}

#[test]
fn disabled_json_schema_plugin_is_inert_for_cardinality() {
    let enabled = json_schema_entry();
    let disabled = RequestFilterEntry::with_enable(json_schema_entry().plugin, false);
    let errors = with_request_plugins(vec![enabled, disabled]).validate_plugins();
    assert!(
        !errors
            .iter()
            .any(|error| error.contains("at most one JsonSchemaValidation")),
        "{errors:?}"
    );
}

#[test]
fn disabled_json_schema_plugin_is_inert_for_all_validation_passes() {
    let mut disabled = json_schema_entry();
    disabled.enable = false;
    disabled.alias = Some("invalid alias".to_string());
    disabled.conditions = Some(
        serde_json::from_value(json!({
            "run": {
                "allOf": [{
                    "type": "keyMatch",
                    "key": {"type": "header", "name": "X-Test"},
                    "regex": ["("]
                }]
            }
        }))
        .unwrap(),
    );
    disabled.body = Some(BodyRequirement {
        max_body_size: Some("invalid".to_string()),
        conditions: Some(Default::default()),
        ..Default::default()
    });
    disabled.plugin = EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
        schema: json!({"$schema": "http://json-schema.org/draft-07/schema#"}),
    });

    let errors = with_request_plugins(vec![disabled, json_schema_entry()]).validate_plugins();
    assert!(errors.is_empty(), "disabled entries must be inert: {errors:?}");
}

#[test]
fn json_schema_plugin_is_request_stage_only() {
    let entry = UpstreamResponseFilterEntry::new(json_schema_entry().plugin);
    let plugins = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            upstream_response_filter_plugins: Some(vec![entry]),
            ..Default::default()
        },
        status: None,
    };
    let errors = plugins.validate_plugins();
    assert!(
        errors
            .iter()
            .any(|error| error.contains("plugin is not valid for upstream response filter stage")),
        "{errors:?}"
    );
}

#[test]
fn dsl_bad_syntax_surfaces_at_schema_layer() {
    let ep = with_request_plugins(vec![dsl_entry("this is not valid dsl @@@")]);
    let errs = ep.validate_plugins();
    assert!(
        errs.iter().any(|e| e.contains("DSL compile errors")),
        "expected a DSL compile error, got: {errs:?}"
    );
}

#[test]
fn dsl_wrong_builtin_arity_surfaces_at_schema_layer() {
    // req.header takes exactly 1 argument; arity is validated at compile time.
    let ep = with_request_plugins(vec![dsl_entry("let h = req.header()")]);
    let errs = ep.validate_plugins();
    assert!(
        errs.iter()
            .any(|e| e.contains("DSL compile errors") && e.contains("expects 1 argument(s), got 0")),
        "expected an arity compile error, got: {errs:?}"
    );
}

#[test]
fn dsl_unknown_redis_link_surfaces_at_schema_layer() {
    let ep = with_request_plugins(vec![dsl_entry("let r = redis.get(\"mylink\", \"mykey\")")]);
    let errs = ep.validate_plugins();
    assert!(
        errs.iter().any(|e| e.contains("linkSysRefs")),
        "expected a redis link-name error, got: {errs:?}"
    );
}

#[test]
fn dsl_request_body_mutation_is_rejected_in_response_stage() {
    let entry = UpstreamResponseFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
        name: "late-mutation".to_string(),
        source: Some("req.body.clear()".to_string()),
        ..Default::default()
    }));
    let plugins = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            upstream_response_filter_plugins: Some(vec![entry]),
            ..Default::default()
        },
        status: None,
    };
    let errors = plugins.validate_plugins();
    assert!(
        errors
            .iter()
            .any(|error| error.contains("request-body mutation is only valid in request stage")),
        "{errors:?}"
    );
}

#[test]
fn dsl_request_body_read_is_allowed_in_response_stage() {
    let entry = UpstreamResponseFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
        name: "late-read".to_string(),
        source: Some("let found = req.body.contains(\"needle\")".to_string()),
        ..Default::default()
    }));
    let plugins = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            upstream_response_filter_plugins: Some(vec![entry]),
            ..Default::default()
        },
        status: None,
    };
    assert!(plugins.validate_plugins().is_empty());
}

#[test]
fn valid_request_plugin_produces_no_errors() {
    let ep = with_request_plugins(vec![RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
        header_filter(),
    ))]);
    assert!(ep.validate_plugins().is_empty());
}

#[test]
fn response_only_plugin_in_request_stage_is_wrong_stage() {
    // The header configuration is valid, so only placement is rejected.
    let ep = with_request_plugins(vec![RequestFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(
        header_filter(),
    ))]);
    assert_eq!(
        ep.validate_plugins(),
        vec!["requestPlugins[0] (ResponseHeaderModifier): plugin is not valid for request stage".to_string()]
    );
}

#[test]
fn disabled_entry_is_skipped() {
    let ep = with_request_plugins(vec![RequestFilterEntry::with_enable(
        EdgionPlugin::ResponseHeaderModifier(header_filter()),
        false,
    )]);
    assert!(ep.validate_plugins().is_empty());
}

#[test]
fn header_modifier_validation_reports_invalid_content_and_skips_disabled_entries() {
    let mut invalid = header_filter();
    invalid.remove = Some(vec!["invalid name".to_string()]);
    let mut ep = with_request_plugins(vec![RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
        invalid.clone(),
    ))]);
    let mut response = UpstreamResponseFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(invalid.clone()));
    ep.spec.upstream_response_filter_plugins = Some(vec![response.clone()]);
    let errors = ep.validate_plugins();
    assert_eq!(errors.len(), 2);
    assert!(errors[0].contains("RequestHeaderModifier"));
    assert!(errors[1].contains("ResponseHeaderModifier"));
    assert!(errors
        .iter()
        .all(|error| error.ends_with(invalid.validate().unwrap_err())));
    assert!(errors.iter().all(|error| !error.contains("invalid name")));

    ep.spec.request_plugins = Some(vec![RequestFilterEntry::with_enable(
        EdgionPlugin::RequestHeaderModifier(invalid),
        false,
    )]);
    response.enable = false;
    ep.spec.upstream_response_filter_plugins = Some(vec![response]);
    assert!(ep.validate_plugins().is_empty());
}

#[test]
fn invalid_alias_is_reported_and_does_not_skip_stage_check() {
    // A valid, correctly-placed plugin with a bad alias yields exactly the
    // alias error (and no spurious stage error).
    let mut entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(header_filter()));
    entry.alias = Some("bad alias".to_string());
    let ep = with_request_plugins(vec![entry]);
    assert_eq!(
        ep.validate_plugins(),
        vec![concat!(
            "requestPlugins[0] (RequestHeaderModifier): ",
            "alias must match [a-zA-Z0-9._-]{1,32} (contains disallowed character)"
        )
        .to_string()]
    );
}

#[test]
fn unsupported_types_decode_without_poisoning_a_resource_list_and_remain_invalid() {
    for stage in [
        "requestPlugins",
        "upstreamResponseFilterPlugins",
        "upstreamResponseBodyFilterPlugins",
        "upstreamResponsePlugins",
    ] {
        for enabled in [true, false] {
            let invalid = json!({"apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
            "metadata":{"name":"invalid"}, "spec":{stage:[
                {"type":"FuturePlugin", "enable":enabled, "config":{"token":"must-not-survive"}}
            ]}});
            let valid = json!({"apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
                "metadata":{"name":"valid"}, "spec":{}});
            let list: Vec<EdgionPlugins> = serde_json::from_value(json!([invalid, valid])).unwrap();
            assert_eq!(
                list[0].validate_plugins(),
                vec![format!("{stage}[0]: unsupported plugin type")]
            );
            assert!(list[1].validate_plugins().is_empty());
            let serialized = serde_json::to_string(&list[0]).unwrap();
            assert!(!serialized.contains("must-not-survive"));
            let roundtrip: EdgionPlugins = serde_json::from_str(&serialized).unwrap();
            assert_eq!(roundtrip.validate_plugins(), list[0].validate_plugins());
            let yaml = serde_yaml::to_string(&list[0]).unwrap();
            let from_yaml: EdgionPlugins = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(from_yaml.validate_plugins(), list[0].validate_plugins());
        }
    }
}

#[test]
fn unsupported_fallback_does_not_accept_malformed_known_configs_or_tags() {
    for input in [
        json!({"config":{}}),
        json!({"type":123, "config":{}}),
        json!({"type":"RequestHeaderModifier", "config":{"remove":123}}),
    ] {
        assert!(serde_json::from_value::<RequestFilterEntry>(input).is_err());
    }
    for input in [
        r#"{"config":{"nested":[1,2]},"type":"FuturePlugin"}"#,
        r#"{"type":"FuturePlugin"}"#,
        r#"{"type":"Unsupported","config":{}}"#,
    ] {
        let entry: RequestFilterEntry = serde_json::from_str(input).unwrap();
        assert!(matches!(entry.plugin, EdgionPlugin::Unsupported));
    }
}
