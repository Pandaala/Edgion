use super::*;

fn base_quota() -> AiTokenQuotaConfig {
    AiTokenQuotaConfig {
        redis_ref: "default/ai-quota".to_string(),
        key: vec![KeyGet::RemoteIp],
        on_key_miss: OnKeyMiss::Deny,
        on_redis_failure: OnRedisFailure::Deny,
        limits: vec![AiTokenQuotaLimit {
            scope: AiTokenQuotaScope::Identity,
            tokens: 100,
            window: "1h".to_string(),
        }],
        on_exceeded: AiTokenQuotaOnExceeded::Reject,
        flush_tokens: 10,
        flush_interval: "100ms".to_string(),
        request_timeout: "500ms".to_string(),
    }
}

fn config(token_quota: Option<AiTokenQuotaConfig>) -> AiProxyConfig {
    AiProxyConfig {
        api: crate::resources::edgion_plugins::AiApi::OpenaiChat,
        conversion: None,
        usage_observation: crate::resources::edgion_plugins::AiUsageObservation::Provider,
        backend_selection: AiBackendSelectionConfig::default(),
        inject_usage_tracking: true,
        on_unknown_model: OnUnknownModel::Reject,
        model_routes: Vec::new(),
        token_quota,
    }
}

#[test]
fn ai_proxy_defaults_are_stable() {
    let config: AiProxyConfig = serde_json::from_str(r#"{"api":"openaiChat"}"#).unwrap();
    assert_eq!(config.backend_selection.selection_type, AiBackendSelection::Weighted);
    assert!(!config.inject_usage_tracking);
    assert_eq!(config.usage_observation, AiUsageObservation::Off);
    assert_eq!(config.on_unknown_model, OnUnknownModel::Reject);
    assert!(config.model_routes.is_empty());
    assert!(config.token_quota.is_none());

    let passthrough: AiProxyConfig = serde_json::from_str(
        r#"{"api":"openaiChat","backendSelection":{"type":"Priority"},"injectUsageTracking":false,"onUnknownModel":"passthrough"}"#,
    )
    .unwrap();
    assert!(!passthrough.inject_usage_tracking);
    assert_eq!(passthrough.on_unknown_model, OnUnknownModel::Passthrough);
    assert_eq!(
        passthrough.backend_selection.selection_type,
        AiBackendSelection::Priority
    );
    assert!(passthrough.validation_error().is_none());
}

#[test]
fn model_routes_validate_wildcards_headers_selectors_and_ambiguity() {
    let valid: AiProxyConfig = serde_yaml::from_str(
        r#"
api: openaiChat
modelRoutes:
  - match:
      model: gpt-4*
      headers:
        - name: X-Tier
          value: paid
    action:
      rewrite: gpt-4o
      candidates:
        - backend:
            group: edgion.io
            kind: EdgionBackend
            namespace: ai
            name: primary
            occurrence: 0
          weight: 0
"#,
    )
    .unwrap();
    assert!(valid.validation_error().is_none());

    for yaml in [
        r#"modelRoutes: [{match: {model: "*"}, action: {}}]"#,
        r#"modelRoutes: [{match: {model: "a*b"}, action: {}}]"#,
        r#"modelRoutes: [{match: {model: a, headers: [{name: X-Tier, value: a}, {name: x-tier, value: b}]}, action: {}}]"#,
        r#"modelRoutes: [{match: {model: a}, action: {candidates: []}}]"#,
        r#"modelRoutes: [{match: {model: a}, action: {rewrite: "alias*"}}]"#,
    ] {
        let value: AiProxyConfig = serde_yaml::from_str(&format!("api: openaiChat\n{yaml}")).unwrap();
        assert!(value.validation_error().is_some(), "accepted {yaml}");
    }

    let ambiguous: AiProxyConfig = serde_yaml::from_str(
        r#"
api: openaiChat
modelRoutes:
  - match: {model: gpt*}
    action: {rewrite: a}
  - match: {model: gpt*}
    action: {rewrite: b}
"#,
    )
    .unwrap();
    assert!(ambiguous
        .validation_error()
        .is_some_and(|error| error.contains("ambiguous")));
}

#[test]
fn model_route_headers_require_bounded_visible_ascii() {
    for yaml in [
        format!(
            "modelRoutes: [{{match: {{model: a, headers: [{{name: {}, value: ok}}]}}, action: {{}}}}]",
            "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize + 1)
        ),
        "modelRoutes: [{match: {model: a, headers: [{name: x-tier, value: \"bad\\nvalue\"}]}, action: {}}]".to_string(),
        "modelRoutes: [{match: {model: a, headers: [{name: x-tier, value: \"é\"}]}, action: {}}]".to_string(),
    ] {
        let value: AiProxyConfig = serde_yaml::from_str(&format!("api: openaiChat\n{yaml}")).unwrap();
        assert!(value.validation_error().is_some(), "accepted {yaml}");
    }

    let boundary = format!(
        "modelRoutes: [{{match: {{model: a, headers: [{{name: {}, value: '~'}}]}}, action: {{}}}}]",
        "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize)
    );
    assert!(
        serde_yaml::from_str::<AiProxyConfig>(&format!("api: openaiChat\n{boundary}"))
            .unwrap()
            .validation_error()
            .is_none()
    );
}

#[test]
fn backend_selection_has_no_inline_candidates_and_gates_passthrough() {
    let error = serde_yaml::from_str::<AiProxyConfig>(
        r#"
api: openaiChat
backendSelection:
  type: Weighted
  candidates:
    - backendRef: backend-a
"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown field `candidates`"));

    for selection_type in [
        AiBackendSelection::LowestLatency,
        AiBackendSelection::LowestUsage,
        AiBackendSelection::CostWeighted,
    ] {
        let invalid = AiProxyConfig {
            api: crate::resources::edgion_plugins::AiApi::OpenaiChat,
            conversion: None,
            usage_observation: crate::resources::edgion_plugins::AiUsageObservation::Provider,
            backend_selection: AiBackendSelectionConfig { selection_type },
            inject_usage_tracking: true,
            on_unknown_model: OnUnknownModel::Passthrough,
            model_routes: Vec::new(),
            token_quota: None,
        };
        let first = invalid.validation_error();
        let second = invalid.validation_error();
        assert_eq!(first, second, "validation must be stateless");
        assert!(
            first
                .as_deref()
                .is_some_and(|message| message.contains("Weighted or Priority")),
            "accepted passthrough with {selection_type:?}"
        );
    }
}

#[test]
fn token_quota_defaults_fail_closed_and_are_bounded() {
    let config: AiProxyConfig = serde_yaml::from_str(
        r#"
api: openaiChat
usageObservation: provider
tokenQuota:
  redisRef: default/ai-quota
  key:
    - type: ctx
      name: consumer_id
  limits:
    - tokens: 10000000
      window: 24h
"#,
    )
    .unwrap();
    let quota = config.token_quota.as_ref().unwrap();
    assert_eq!(quota.on_key_miss, OnKeyMiss::Deny);
    assert_eq!(quota.on_redis_failure, OnRedisFailure::Deny);
    assert_eq!(quota.on_exceeded, AiTokenQuotaOnExceeded::Reject);
    assert_eq!(quota.flush_tokens, 100);
    assert_eq!(quota.flush_interval, "1s");
    assert_eq!(quota.request_timeout, "500ms");
    assert!(config.validation_error().is_none());
}

#[test]
fn token_quota_rejects_unsafe_or_ambiguous_configuration() {
    let base = base_quota();

    let mut cases = Vec::new();
    let mut invalid_ref = base.clone();
    invalid_ref.redis_ref = "missing-namespace".to_string();
    cases.push(invalid_ref);
    let mut zero_limit = base.clone();
    zero_limit.limits[0].tokens = 0;
    cases.push(zero_limit);
    let mut unsafe_limit = base.clone();
    unsafe_limit.limits[0].tokens = AI_QUOTA_MAX_SAFE_INTEGER + 1;
    cases.push(unsafe_limit);
    let mut zero_timeout = base.clone();
    zero_timeout.request_timeout = "0s".to_string();
    cases.push(zero_timeout);

    for quota in cases {
        assert!(config(Some(quota)).validation_error().is_some());
    }
}

#[test]
fn token_quota_exceeded_actions_are_tagged_and_validated() {
    let mut throttle = base_quota();
    throttle.on_exceeded = AiTokenQuotaOnExceeded::Throttle {
        max_delay: "250ms".to_string(),
    };
    assert!(config(Some(throttle.clone())).validation_error().is_none());
    assert_eq!(
        throttle.on_exceeded.parsed_max_delay().unwrap(),
        Some(Duration::from_millis(250))
    );

    for max_delay in ["0s", "6s", "invalid"] {
        throttle.on_exceeded = AiTokenQuotaOnExceeded::Throttle {
            max_delay: max_delay.to_string(),
        };
        assert!(config(Some(throttle.clone())).validation_error().is_some());
    }

    let parsed: AiTokenQuotaOnExceeded = serde_yaml::from_str("type: Downgrade\nmodel: chat-economy\n").unwrap();
    assert_eq!(
        parsed,
        AiTokenQuotaOnExceeded::Downgrade {
            model: "chat-economy".to_string()
        }
    );

    let mut downgrade = base_quota();
    downgrade.limits[0].scope = AiTokenQuotaScope::IdentityModel;
    downgrade.on_exceeded = parsed;
    assert!(config(Some(downgrade.clone())).validation_error().is_none());

    downgrade.limits[0].scope = AiTokenQuotaScope::Identity;
    assert!(config(Some(downgrade.clone()))
        .validation_error()
        .is_some_and(|error| error.contains("identityModel")));

    downgrade.limits[0].scope = AiTokenQuotaScope::IdentityModel;
    for model in [" ".to_string(), "m".repeat(AI_MODEL_NAME_MAX_BYTES + 1)] {
        downgrade.on_exceeded = AiTokenQuotaOnExceeded::Downgrade { model };
        assert!(config(Some(downgrade.clone())).validation_error().is_some());
    }

    downgrade.on_exceeded = AiTokenQuotaOnExceeded::Downgrade {
        model: "economy\nmodel".to_string(),
    };
    assert!(config(Some(downgrade.clone()))
        .validation_error()
        .is_some_and(|error| error.contains("HTTP header value")));

    downgrade.on_exceeded = AiTokenQuotaOnExceeded::Downgrade {
        model: "économy".to_string(),
    };
    assert!(http::HeaderValue::try_from("économy").is_ok());
    assert!(config(Some(downgrade)).validation_error().is_none());
}

#[test]
fn api_is_required_and_observation_is_an_independent_explicit_contract() {
    assert!(serde_json::from_str::<AiProxyConfig>("{}").is_err());
    let schema = serde_json::to_value(schemars::schema_for!(AiProxyConfig)).unwrap();
    assert!(schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field == "api"));
    let unsupported: AiProxyConfig = serde_json::from_value(serde_json::json!({"api":"anthropicMessages"})).unwrap();
    assert!(unsupported.validation_error().is_none());
    assert!(unsupported.accepts_provider(AiProvider::Anthropic));
    assert!(!unsupported.accepts_provider(AiProvider::OpenAI));
    let mut native: AiProxyConfig = serde_json::from_str(r#"{"api":"openaiChat"}"#).unwrap();
    assert!(native.validation_error().is_none());
    native.inject_usage_tracking = true;
    assert!(native.validation_error().is_some());
    native.conversion = Some(AiConversion::ChatToAnthropic);
    assert!(native.validation_error().is_none());
    native.conversion = None;
    native.usage_observation = AiUsageObservation::Provider;
    assert!(native.validation_error().is_none());
    native.usage_observation = AiUsageObservation::Off;
    native.inject_usage_tracking = false;
    for selection in [
        AiBackendSelection::Weighted,
        AiBackendSelection::Priority,
        AiBackendSelection::LowestLatency,
        AiBackendSelection::CostWeighted,
    ] {
        native.backend_selection.selection_type = selection;
        assert!(native.validation_error().is_none());
    }
    native.backend_selection.selection_type = AiBackendSelection::LowestUsage;
    assert!(native.validation_error().is_some());
    native.backend_selection.selection_type = AiBackendSelection::Weighted;
    native.token_quota = Some(base_quota());
    assert!(native.validation_error().is_some());
    native.usage_observation = AiUsageObservation::Provider;
    assert!(native.validation_error().is_none());
}

#[test]
fn native_responses_has_an_explicit_openai_only_matrix() {
    let mut config: AiProxyConfig = serde_json::from_str(r#"{"api":"openaiResponses"}"#).unwrap();
    assert!(config.validation_error().is_none());
    assert!(config.accepts_provider(AiProvider::OpenAI));
    assert!(!config.accepts_provider(AiProvider::OpenAICompatible));
    assert!(!config.accepts_provider(AiProvider::Anthropic));
    config.usage_observation = AiUsageObservation::Provider;
    assert!(config.validation_error().is_none());
    config.inject_usage_tracking = true;
    assert!(config.validation_error().is_some());
    config.inject_usage_tracking = false;
    config.conversion = Some(AiConversion::ChatToAnthropic);
    assert!(config.validation_error().is_some());
    assert!(!config.accepts_provider(AiProvider::Anthropic));
}

#[test]
fn native_messages_has_explicit_anthropic_only_accounting_matrix() {
    let mut config: AiProxyConfig = serde_json::from_str(r#"{"api":"anthropicMessages"}"#).unwrap();
    assert!(config.validation_error().is_none());
    assert!(config.accepts_provider(AiProvider::Anthropic));
    assert!(!config.accepts_provider(AiProvider::OpenAI));
    assert!(!config.accepts_provider(AiProvider::OpenAICompatible));
    config.inject_usage_tracking = true;
    assert!(config.validation_error().is_some());
    config.inject_usage_tracking = false;
    config.conversion = Some(AiConversion::ChatToAnthropic);
    assert!(config.validation_error().is_some());
    config.conversion = None;
    config.backend_selection.selection_type = AiBackendSelection::LowestUsage;
    assert!(config.validation_error().is_some());
    config.usage_observation = AiUsageObservation::Provider;
    assert!(config.validation_error().is_none());
    config.token_quota = Some(base_quota());
    assert!(config.validation_error().is_none());
    config.usage_observation = AiUsageObservation::Off;
    assert!(config.validation_error().is_some());
}
