use super::*;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::edgion_backend::AI_MODEL_NAME_MAX_BYTES;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ai::runtime::backend_store::RuntimeWildcardModel;
use crate::ai::{ResolvedModel, RuntimeAiBackend, RuntimeAiCredentialPool, RuntimeAiEndpoint};
use crate::plugins::common::redis_rate_limiter::RedisRateLimiter;
use crate::plugins::PluginRuntime;
use crate::routes::http::match_unit::CompiledBackend;
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::edgion_backend::AiProvider;
use edgion_resources::resources::edgion_plugins::{
    AiBackendSelection, AiBackendSelectionConfig, AiModelRoute, AiModelRouteAction, AiModelRouteMatch,
};
use edgion_resources::HTTPBackendRef;

fn config() -> AiProxyConfig {
    AiProxyConfig {
        api: edgion_resources::resources::edgion_plugins::AiApi::OpenaiChat,
        conversion: None,
        usage_observation: edgion_resources::resources::edgion_plugins::AiUsageObservation::Provider,
        backend_selection: AiBackendSelectionConfig {
            selection_type: AiBackendSelection::Weighted,
        },
        inject_usage_tracking: true,
        on_unknown_model: OnUnknownModel::Reject,
        model_routes: Vec::new(),
        token_quota: None,
    }
}

fn compiled_ai_backend(ref_denied: Option<RefDenied>) -> CompiledBackend {
    CompiledBackend::compile(
        HTTPBackendRef {
            name: "provider".to_string(),
            namespace: Some("tenant-b".to_string()),
            port: None,
            weight: Some(1),
            group: Some("edgion.io".to_string()),
            kind: Some("EdgionBackend".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied,
        },
        "tenant-a",
        0,
        Arc::new(PluginRuntime::new()),
    )
}

fn compiled_service_backend(weight: i32) -> CompiledBackend {
    CompiledBackend::compile(
        HTTPBackendRef {
            name: "service".to_string(),
            namespace: None,
            port: Some(8080),
            weight: Some(weight),
            group: None,
            kind: Some("Service".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        },
        "tenant-a",
        1,
        Arc::new(PluginRuntime::new()),
    )
}

#[test]
fn zero_weight_service_slot_is_protocol_neutral_at_request_gate() {
    let ai = compiled_ai_backend(None);
    assert!(!has_positive_weight_service_backend(&[
        ai.clone(),
        compiled_service_backend(0),
    ]));
    assert!(has_positive_weight_service_backend(&[ai, compiled_service_backend(1),]));
}

#[test]
fn denied_ai_backend_is_invalid_and_never_materialized() {
    let denied = compiled_ai_backend(Some(RefDenied {
        target_namespace: "tenant-b".to_string(),
        target_name: "provider".to_string(),
        reason: Some("NoMatchingReferenceGrant".to_string()),
    }));
    let allowed = compiled_ai_backend(None);

    assert!(!denied.is_permitted_ai_backend());
    assert!(allowed.is_permitted_ai_backend());
    let gate_candidates = [gate_candidate(0, &denied, false).unwrap()];
    assert_eq!(invalid_weight_gate(&gate_candidates, 0), Ok(ConformanceGate::Invalid));
    assert!(ai_candidate_spec(&denied).is_none());
    let (slot, weight, key) = ai_candidate_spec(&allowed).unwrap();
    assert_eq!((slot, weight, key), (0, 1, "tenant-b/provider".to_string()));
    let generation = Arc::from(vec![allowed.clone()]);
    let logical = crate::backends::LogicalBackendHandle::new(generation, 0).unwrap();
    assert_eq!(logical.identity(), &allowed.identity);
}

#[test]
fn missing_runtime_backend_keeps_its_invalid_weight_share() {
    let declared = compiled_ai_backend(None);
    let gate_candidates = [gate_candidate(0, &declared, false).unwrap()];
    assert_eq!(invalid_weight_gate(&gate_candidates, 0), Ok(ConformanceGate::Invalid));
}

#[test]
fn constructor_contains_no_backend_membership() {
    let plugin = AiProxy::from_config(&config(), "tenant-a/plugins/#0").expect("valid plugin");
    assert_eq!(
        plugin.config.backend_selection.selection_type,
        AiBackendSelection::Weighted
    );
    assert!(plugin.quota.is_none());
}

#[test]
fn model_list_is_deterministic_public_and_requires_symbolic_wildcard_owner() {
    let public = ResolvedModel {
        canonical_name: "provider-public".to_string(),
        pricing: None,
        public: true,
    };
    let hidden = ResolvedModel {
        canonical_name: "provider-hidden".to_string(),
        pricing: None,
        public: false,
    };
    let runtime = Arc::new(RuntimeAiBackend {
        has_credential_tpm: false,
        uid: "backend-uid".to_string(),
        provider: AiProvider::OpenAI,
        endpoint: RuntimeAiEndpoint {
            domain: "example.test".to_string(),
            port: 443,
            use_tls: true,
            sni: "example.test".to_string(),
            host: "example.test".to_string(),
            path_prefix: String::new(),
        },
        streaming: true,
        max_output_tokens: None,
        credential_pool: RuntimeAiCredentialPool {
            redis_ref: None,
            on_redis_failure: Default::default(),
            credentials: Vec::new(),
            limiter_core: Arc::new(RedisRateLimiter::new()),
        },
        models: HashMap::from([("public".to_string(), public.clone()), ("hidden".to_string(), hidden)]),
        wildcard_models: vec![
            RuntimeWildcardModel {
                pattern: "owned*".to_string(),
                prefix: "owned".to_string(),
                model: public.clone(),
            },
            RuntimeWildcardModel {
                pattern: "n*".to_string(),
                prefix: "n".to_string(),
                model: public,
            },
        ],
        canonical_models: HashSet::from(["public".to_string(), "hidden".to_string()]),
        public_model_ids: BTreeSet::from(["public".to_string(), "owned*".to_string(), "n*".to_string()]),
    });
    let routes = ModelRouteTable::compile(&[
        AiModelRoute {
            match_: AiModelRouteMatch {
                model: "virtual*".to_string(),
                headers: Vec::new(),
            },
            action: AiModelRouteAction {
                rewrite: Some("hidden".to_string()),
                candidates: None,
            },
        },
        AiModelRoute {
            match_: AiModelRouteMatch {
                model: "owned*".to_string(),
                headers: Vec::new(),
            },
            action: AiModelRouteAction {
                rewrite: None,
                candidates: None,
            },
        },
        AiModelRoute {
            match_: AiModelRouteMatch {
                model: "narrow*".to_string(),
                headers: Vec::new(),
            },
            action: AiModelRouteAction {
                rewrite: None,
                candidates: None,
            },
        },
    ])
    .unwrap();
    let visible = visible_model_ids(&routes, &[compiled_ai_backend(None)], &http::HeaderMap::new(), |key| {
        (key == "tenant-b/provider").then(|| runtime.clone())
    });
    assert_eq!(visible, vec!["owned*", "public"]);
    assert!(!visible.iter().any(|id| id == "virtual*" || id == "narrow*"));
}

#[test]
fn canonical_request_preserves_logical_model_and_injects_usage_once() {
    let input =
        Bytes::from_static(br#"{"model":"fast","stream":true,"stream_options":{"custom":7,"include_usage":false}}"#);
    let witness = input.clone();
    let mut request = canonical_openai_request(input, true).unwrap();
    assert_eq!(request.input_chars, 0, "metadata capture alone must not count usage");
    request.input_chars = checked_utf8_chars(&request.body).unwrap();
    assert_eq!(request.logical_model, "fast");
    assert!(request.requested_streaming);
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["model"], "fast");
    assert_eq!(body["stream_options"]["custom"], 7);
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert_eq!(
        request.input_chars,
        u64::try_from(std::str::from_utf8(&request.body).unwrap().chars().count()).unwrap()
    );
    assert_ne!(request.body.as_ptr(), witness.as_ptr());
}

#[test]
fn canonical_request_reuses_nonempty_sliced_bytes_when_wire_body_is_unchanged() {
    let padded = Bytes::from_static(br#"xx{"model":"fast","stream":false}yy"#);
    let input = padded.slice(2..padded.len() - 2);
    let witness = input.clone();
    let expected_ptr = witness.as_ptr();
    let expected_len = witness.len();

    let request = canonical_openai_request(input, true).unwrap();

    assert_eq!(request.body, witness);
    assert_eq!(request.body.len(), expected_len);
    assert_eq!(request.body.as_ptr(), expected_ptr);
}

#[test]
fn canonical_request_rejects_malformed_or_unbounded_model() {
    assert!(matches!(
        canonical_openai_request(Bytes::from_static(br#"{"model":7}"#), true),
        Err(AiProxyError::MalformedBody)
    ));
    let body = format!(r#"{{"model":"{}"}}"#, "x".repeat(AI_MODEL_NAME_MAX_BYTES + 1));
    assert!(matches!(
        canonical_openai_request(Bytes::from(body.into_bytes()), true),
        Err(AiProxyError::ModelNotFound)
    ));
}

#[test]
fn canonical_request_invalid_utf8_remains_malformed_body() {
    assert!(matches!(
        canonical_openai_request(Bytes::from_static(b"{\"model\":\"fast\",\"content\":\"\xff\"}"), true),
        Err(AiProxyError::MalformedBody)
    ));
}

#[test]
fn canonical_request_replaces_only_the_logical_model_for_downgrade() {
    let mut request = canonical_openai_request(
        Bytes::from_static(br#"{"model":"premium","stream":true,"messages":[{"role":"user","content":"hello"}]}"#),
        true,
    )
    .unwrap();

    assert_eq!(request.input_chars, 0);
    request.input_chars = checked_utf8_chars(&request.body).unwrap();
    let premium_chars = request.input_chars;
    request.replace_logical_model("e").unwrap();
    let short_chars = request.input_chars;
    assert!(short_chars < premium_chars);

    request.replace_logical_model("economy-longer-than-premium").unwrap();

    assert_eq!(request.logical_model, "economy-longer-than-premium");
    assert!(request.input_chars > short_chars);
    assert_eq!(
        request.input_chars,
        u64::try_from(std::str::from_utf8(&request.body).unwrap().chars().count()).unwrap()
    );
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["model"], "economy-longer-than-premium");
    assert_eq!(body["messages"][0]["content"], "hello");
    assert_eq!(body["stream_options"]["include_usage"], true);
}

#[test]
fn canonical_input_chars_tracks_usage_injection_and_unicode_characters() {
    let original = Bytes::copy_from_slice(r#"{"model":"fast","stream":true,"messages":[{"content":"é€"}]}"#.as_bytes());
    let mut without_injection = canonical_openai_request(original.clone(), false).unwrap();
    let mut with_injection = canonical_openai_request(original, true).unwrap();
    assert_eq!(without_injection.input_chars, 0);
    assert_eq!(with_injection.input_chars, 0);
    without_injection.input_chars = checked_utf8_chars(&without_injection.body).unwrap();
    with_injection.input_chars = checked_utf8_chars(&with_injection.body).unwrap();

    assert_eq!(
        without_injection.input_chars,
        u64::try_from(std::str::from_utf8(&without_injection.body).unwrap().chars().count()).unwrap()
    );
    assert_eq!(
        with_injection.input_chars,
        u64::try_from(std::str::from_utf8(&with_injection.body).unwrap().chars().count()).unwrap()
    );
    assert!(with_injection.input_chars > without_injection.input_chars);
}

#[test]
fn initial_candidate_errors_preserve_model_streaming_and_availability_semantics() {
    assert_eq!(
        initial_candidate_error(false, true, false, false),
        Some(("model-not-found", AiProxyError::ModelNotFound))
    );
    assert_eq!(
        initial_candidate_error(true, true, true, false),
        Some(("streaming-not-supported", AiProxyError::StreamingDisabled))
    );
    assert_eq!(initial_candidate_error(true, false, false, false), None);
    assert_eq!(initial_candidate_error(true, true, true, true), None);
}

#[test]
fn downgrade_re_resolves_models_from_captured_candidates_without_passthrough() {
    let backend = Arc::new(RuntimeAiBackend {
        has_credential_tpm: false,
        uid: "backend-uid".to_string(),
        provider: AiProvider::OpenAI,
        endpoint: RuntimeAiEndpoint {
            domain: "example.test".to_string(),
            port: 443,
            use_tls: true,
            sni: "example.test".to_string(),
            host: "example.test".to_string(),
            path_prefix: String::new(),
        },
        streaming: true,
        max_output_tokens: None,
        credential_pool: RuntimeAiCredentialPool {
            redis_ref: None,
            on_redis_failure: Default::default(),
            credentials: Vec::new(),
            limiter_core: Arc::new(RedisRateLimiter::new()),
        },
        models: HashMap::from([
            (
                "premium".to_string(),
                ResolvedModel {
                    canonical_name: "provider-premium".to_string(),
                    pricing: None,
                    public: true,
                },
            ),
            (
                "economy".to_string(),
                ResolvedModel {
                    canonical_name: "provider-economy".to_string(),
                    pricing: None,
                    public: true,
                },
            ),
        ]),
        wildcard_models: Vec::new(),
        canonical_models: HashSet::from(["premium".to_string(), "economy".to_string()]),
        public_model_ids: BTreeSet::from(["premium".to_string(), "economy".to_string()]),
    });
    let compiled = compiled_ai_backend(None);
    let logical = crate::backends::LogicalBackendHandle::new(Arc::from(vec![compiled]), 0).unwrap();
    let mut candidates = vec![AiPlanCandidate {
        logical,
        slot: 0,
        order: 0,
        weight: 1,
        backend_key: "tenant/backend".to_string(),
        backend: Some(backend),
        model: None,
        effective_policy: None,
        backend_tls_policy: crate::backends::policy::BackendTlsPolicyLookup::Absent,
        adaptive: None,
        credential_outliers: Vec::new(),
    }];

    assert!(resolve_candidate_models(&mut candidates, "economy", true, false));
    assert_eq!(
        candidates[0].model.as_ref().map(|model| model.provider_model.as_str()),
        Some("provider-economy")
    );
    assert!(!resolve_candidate_models(&mut candidates, "unknown", true, false));
    assert!(candidates[0].model.is_none());

    // Simulate the passthrough result without adding the client-provided
    // name to the bounded backend catalog.
    candidates[0].model = Some(AiCandidateModel {
        provider_model: "client-provided".to_string(),
        pricing: None,
    });
    let adaptive = AdaptiveTelemetryStore::new();
    capture_adaptive_states(
        &adaptive,
        AiBackendSelection::Weighted,
        "route-uid",
        "rule-a",
        "client-provided",
        &mut candidates,
    );
    assert!(adaptive.is_empty());
    capture_adaptive_states(
        &adaptive,
        AiBackendSelection::LowestLatency,
        "route-uid",
        "rule-a",
        "client-provided",
        &mut candidates,
    );
    assert!(adaptive.is_empty());

    assert!(resolve_candidate_models(&mut candidates, "economy", true, false));
    adaptive.reconcile_keys(&std::collections::HashSet::from([AdaptiveTelemetryKey::new(
        "route-uid".to_string(),
        "rule-a".to_string(),
        PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "tenant".to_string(),
            name: "backend".to_string(),
        },
        "economy".to_string(),
    )
    .unwrap()]));
    capture_adaptive_states(
        &adaptive,
        AiBackendSelection::LowestLatency,
        "route-uid",
        "rule-a",
        "economy",
        &mut candidates,
    );
    assert_eq!(adaptive.count(), 1);
    assert!(candidates[0].adaptive.is_some());

    let passthrough_inputs = vec![((0, 1, "tenant/backend".to_string()), candidates[0].backend.clone())];
    assert!(unknown_model_passthrough_allowed(
        false,
        OnUnknownModel::Passthrough,
        &passthrough_inputs
    ));
    assert!(!unknown_model_passthrough_allowed(
        true,
        OnUnknownModel::Passthrough,
        &passthrough_inputs
    ));
    assert!(!unknown_model_passthrough_allowed(
        false,
        OnUnknownModel::Reject,
        &passthrough_inputs
    ));
    assert!(!unknown_model_passthrough_allowed(
        false,
        OnUnknownModel::Passthrough,
        &[((0, 1, "missing/backend".to_string()), None)]
    ));
}

#[test]
fn plan_debug_never_renders_canonical_body() {
    let plan = ReadyAiAttemptPlan {
        api: edgion_resources::resources::edgion_plugins::AiApi::OpenaiChat,
        operation: crate::ai::runtime::attempt_plan::AiOperation::ChatCompletions,
        conversion: None,
        usage_observation: edgion_resources::resources::edgion_plugins::AiUsageObservation::Provider,
        body_generation: None,
        body_is_current: false,
        request_max_output_tokens: None,
        request_body: Bytes::from_static(br#"{"prompt":"must-not-render"}"#),
        canonical_path: "/v1/chat/completions".to_string(),
        canonical_query: None,
        logical_model: "model-a".to_string(),
        requested_streaming: false,
        inject_usage_tracking: true,
        input_chars: 28,
        selection: AiBackendSelection::Weighted,
        selection_sequence: 0,
        candidates: Vec::new(),
        scheduler: AttemptScheduler::new(&[], 0).unwrap(),
        retry_cap_initialized: true,
        backend_snapshot: AiBackendStore::new().snapshot(),
        quota: None,
        quota_status: None,
        terminate_on_over_limit: false,
    };
    let rendered = format!("{plan:?}");
    assert!(!rendered.contains("must-not-render"));
    assert!(rendered.contains("request_body_bytes"));
}

#[test]
fn captured_backend_skew_cannot_hide_unresolved_tpm_or_provider_changes() {
    use edgion_resources::resources::edgion_backend::EdgionBackend;
    use k8s_openapi::ByteString;
    let store = AiBackendStore::new();
    let mut declared: EdgionBackend = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"provider","namespace":"tenant-b"},
        "spec":{"ai":{"provider":"OpenAICompatible","endpoint":"https://provider.example.com","credentialPool":{"credentials":[{"name":"usable","secretRef":{"name":"key"}}]},"models":[{"name":"model"}],"defaults":{"maxOutputTokens":64}}}
    })).unwrap();
    declared.spec.ai.credential_pool.credentials[0].credential.secret = Some(k8s_openapi::api::core::v1::Secret {
        data: Some(std::collections::BTreeMap::from([(
            "api-key".to_string(),
            ByteString(b"usable-key".to_vec()),
        )])),
        ..Default::default()
    });
    store.full_set(&HashMap::from([(
        "tenant-b/provider".to_string(),
        Arc::new(declared.clone()),
    )]));
    let before = store.snapshot();
    let mut config = config();
    config.usage_observation = AiUsageObservation::Off;
    config.inject_usage_tracking = false;
    let backend = compiled_ai_backend(None);
    assert!(captured_backend_contract_valid(
        &config,
        std::slice::from_ref(&backend),
        &before
    ));
    let mut missing = declared.spec.ai.credential_pool.credentials[0].clone();
    missing.name = "missing-tpm".into();
    missing.weight = 1;
    missing.credential.secret = None;
    missing.limits = Some(edgion_resources::resources::edgion_backend::AiCredentialLimits {
        requests_per_minute: None,
        tokens_per_minute: Some(1000),
    });
    declared.spec.ai.credential_pool.redis_ref = Some("default/accounting".into());
    declared.spec.ai.credential_pool.credentials.push(missing);
    store.full_set(&HashMap::from([(
        "tenant-b/provider".to_string(),
        Arc::new(declared.clone()),
    )]));
    let current = store.snapshot();
    assert!(captured_backend_contract_valid(
        &config,
        std::slice::from_ref(&backend),
        &before
    ));
    assert!(!captured_backend_contract_valid(
        &config,
        std::slice::from_ref(&backend),
        &current
    ));
    config.usage_observation = AiUsageObservation::Provider;
    assert!(captured_backend_contract_valid(
        &config,
        std::slice::from_ref(&backend),
        &current
    ));
    declared.spec.ai.provider = AiProvider::Anthropic;
    declared.spec.ai.endpoint = None;
    store.full_set(&HashMap::from([("tenant-b/provider".to_string(), Arc::new(declared))]));
    let current = store.snapshot();
    assert!(!captured_backend_contract_valid(
        &config,
        std::slice::from_ref(&backend),
        &current
    ));
    let mut zero = backend;
    zero.schema.weight = Some(0);
    assert!(captured_backend_contract_valid(&config, &[zero.clone()], &current));
    config.model_routes = serde_json::from_value(serde_json::json!([{"match":{"model":"virtual"},"action":{"candidates":[{"backend":{"group":"edgion.io","kind":"EdgionBackend","namespace":"tenant-b","name":"provider","occurrence":0},"weight":1}]}}])).unwrap();
    assert!(!captured_backend_contract_valid(&config, &[zero], &current));
}
