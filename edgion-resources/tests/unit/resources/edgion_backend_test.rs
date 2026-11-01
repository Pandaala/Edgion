use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::CustomResourceExt;

use super::*;
use crate::redact::AdminRedactGuard;
use crate::resources::common::SecretObjectReference;

fn ai_spec(provider: AiProvider) -> AiBackendSpec {
    AiBackendSpec {
        provider,
        endpoint: None,
        allow_insecure_http: false,
        credential_pool: AiCredentialPool {
            redis_ref: None,
            on_redis_failure: OnRedisFailure::Deny,
            credentials: vec![AiCredential {
                name: "primary".to_string(),
                weight: 1,
                credential: SecretSlot {
                    secret_ref: SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "ai-credential".to_string(),
                        namespace: None,
                    },
                    secret: None,
                },
                limits: None,
            }],
        },
        models: vec![AiModel {
            name: "model-a".to_string(),
            aliases: vec!["default".to_string()],
            public: true,
            pricing: None,
        }],
        defaults: AiBackendDefaults::default(),
    }
}

fn spec(provider: AiProvider) -> EdgionBackendSpec {
    EdgionBackendSpec {
        current_status: None,
        ai: ai_spec(provider),
        reconcile_outcome: None,
    }
}

#[test]
fn crd_identity_is_stable() {
    let crd = EdgionBackend::crd();
    assert_eq!(crd.spec.group, "edgion.io");
    assert_eq!(crd.spec.versions[0].name, "v1");
    assert_eq!(crd.spec.names.kind, "EdgionBackend");
    assert_eq!(crd.spec.names.plural, "edgionbackends");
    assert_eq!(crd.spec.names.short_names, Some(vec!["eb".to_string()]));
    assert_eq!(crd.spec.scope, "Namespaced");
}

#[test]
fn nested_wire_shape_is_strict_and_rejects_flat_fields() {
    let nested = serde_json::to_value(spec(AiProvider::OpenAI)).unwrap();
    assert!(nested.get("ai").is_some());
    assert!(nested.get("provider").is_none());

    let flat = serde_json::to_value(ai_spec(AiProvider::OpenAI)).unwrap();
    assert!(serde_json::from_value::<EdgionBackendSpec>(flat).is_err());
    assert_eq!(crate::ResourceKind::EdgionBackend as i32, 23);
    assert_eq!(
        crate::ResourceKind::try_from(23).ok(),
        Some(crate::ResourceKind::EdgionBackend)
    );
}

#[test]
fn nested_unknown_fields_are_rejected() {
    for yaml in [
            "ai: {}\nunknown: true\n",
            "ai:\n  provider: OpenAI\n  credentialPool:\n    credentials:\n      - name: primary\n        secretRef:\n          name: creds\n          unknown: true\n  models:\n    - name: model-a\n",
            "ai:\n  provider: OpenAI\n  credentialPool:\n    credentials:\n      - name: primary\n        secretRef:\n          name: creds\n        unknown: true\n  models:\n    - name: model-a\n",
        ] {
            assert!(
                yaml_serde::from_str::<EdgionBackendSpec>(yaml).is_err(),
                "unknown nested field must fail: {yaml}"
            );
        }
}

#[test]
fn provider_wire_names_are_stable() {
    assert_eq!(serde_json::to_string(&AiProvider::OpenAI).unwrap(), "\"OpenAI\"");
    assert_eq!(
        serde_json::to_string(&AiProvider::OpenAICompatible).unwrap(),
        "\"OpenAICompatible\""
    );
    assert_eq!(serde_json::to_string(&AiProvider::Anthropic).unwrap(), "\"Anthropic\"");
}

#[test]
fn serde_defaults_are_stable() {
    let parsed: AiBackendSpec = yaml_serde::from_str(
            "provider: OpenAI\ncredentialPool:\n  credentials:\n    - name: primary\n      secretRef:\n        name: ai-credential\nmodels:\n  - name: model-a\n",
        )
        .unwrap();
    assert!(!parsed.allow_insecure_http);
    assert!(parsed.defaults.streaming);
    assert_eq!(parsed.defaults.max_output_tokens, None);
    assert!(parsed.models[0].aliases.is_empty());
    assert_eq!(parsed.credential_pool.credentials[0].weight, 1);
    assert_eq!(parsed.credential_pool.on_redis_failure, OnRedisFailure::Deny);
}

#[test]
fn max_output_tokens_wire_and_schema_are_stable() {
    let parsed: AiBackendSpec = yaml_serde::from_str(
            "provider: Anthropic\ncredentialPool:\n  credentials:\n    - name: primary\n      secretRef:\n        name: ai-credential\nmodels:\n  - name: model-a\ndefaults:\n  maxOutputTokens: 4096\n",
        )
        .unwrap();
    assert_eq!(parsed.defaults.max_output_tokens, Some(4096));

    let serialized = serde_json::to_value(&parsed).unwrap();
    assert_eq!(serialized["defaults"]["maxOutputTokens"], 4096);
    assert!(serialized["defaults"].get("max_output_tokens").is_none());

    let schema = serde_json::to_value(schemars::schema_for!(AiBackendDefaults)).unwrap();
    assert_eq!(schema["properties"]["maxOutputTokens"]["minimum"], 1);
}

#[test]
fn rejects_inline_or_unknown_credential_fields() {
    for field in ["apiKey", "password"] {
        let yaml = format!(
                "provider: OpenAI\ncredentialPool:\n  credentials:\n    - name: primary\n      secretRef:\n        name: ai-credential\n      {field}: must-not-be-accepted\nmodels:\n  - name: model-a\n"
            );
        assert!(
            yaml_serde::from_str::<AiBackendSpec>(&yaml).is_err(),
            "unknown inline credential field {field} must be rejected"
        );
    }
}

#[test]
fn model_catalog_rejects_empty_and_colliding_names() {
    let mut value = ai_spec(AiProvider::OpenAI);
    value.models.clear();
    assert!(value.model_alias_table().unwrap_err()[0].contains("at least one"));

    value.models = vec![
        AiModel {
            name: "model-a".to_string(),
            aliases: vec!["".to_string(), "model-b".to_string(), "shared".to_string()],
            public: true,
            pricing: None,
        },
        AiModel {
            name: "model-b".to_string(),
            aliases: vec!["shared".to_string()],
            public: true,
            pricing: None,
        },
        AiModel {
            name: "model-b".to_string(),
            aliases: Vec::new(),
            public: true,
            pricing: None,
        },
        AiModel {
            name: " ".to_string(),
            aliases: Vec::new(),
            public: true,
            pricing: None,
        },
    ];
    let errors = value.model_alias_table().unwrap_err().join("\n");
    assert!(errors.contains("duplicate model name"));
    assert!(errors.contains("model name must not be empty"));
    assert!(errors.contains("empty alias"));
    assert!(errors.contains("collides with a canonical name"));
    assert!(errors.contains("assigned to both"));
}

#[test]
fn model_alias_table_resolves_to_canonical_names() {
    let table = ai_spec(AiProvider::OpenAI).model_alias_table().unwrap();
    assert_eq!(table.get("model-a").map(String::as_str), Some("model-a"));
    assert_eq!(table.get("default").map(String::as_str), Some("model-a"));
}

#[test]
fn model_catalog_uses_longest_suffix_wildcard_and_public_default() {
    let mut value = ai_spec(AiProvider::OpenAI);
    value.models = vec![
        AiModel {
            name: "broad".to_string(),
            aliases: vec!["gpt*".to_string()],
            public: false,
            pricing: None,
        },
        AiModel {
            name: "specific".to_string(),
            aliases: vec!["gpt-4*".to_string()],
            public: true,
            pricing: None,
        },
    ];
    let catalog = value.model_catalog().unwrap();
    assert_eq!(catalog.resolve("gpt-4.1").unwrap().canonical_name, "specific");
    assert_eq!(catalog.resolve("gpt-3").unwrap().canonical_name, "broad");
    assert!(!catalog.resolve("gpt-3").unwrap().public);

    let decoded: AiModel = serde_json::from_str(r#"{"name":"m","aliases":[]}"#).unwrap();
    assert!(decoded.public);
}

#[test]
fn model_catalog_rejects_invalid_and_duplicate_wildcards() {
    let mut value = ai_spec(AiProvider::OpenAI);
    value.models[0].aliases = vec![
        "*".to_string(),
        "bad*tail".to_string(),
        "dup*".to_string(),
        "dup*".to_string(),
    ];
    let errors = value.model_catalog().unwrap_err().join("\n");
    assert!(errors.contains("trailing '*'"));
    assert!(errors.contains("duplicate wildcard"));
}

#[test]
fn model_catalog_bounds_names_and_aliases() {
    let mut value = ai_spec(AiProvider::Anthropic);
    value.models[0].name = "m".repeat(AI_MODEL_NAME_MAX_BYTES + 1);
    value.models[0].aliases = vec!["a".repeat(AI_MODEL_NAME_MAX_BYTES + 1)];
    let errors = value.model_alias_table().unwrap_err().join("\n");
    assert!(errors.contains("model name must not exceed"));
    assert!(errors.contains("model alias"));

    value = ai_spec(AiProvider::Anthropic);
    value.models = vec![value.models[0].clone(); AI_MODEL_MAX_COUNT + 1];
    assert!(value
        .model_alias_table()
        .unwrap_err()
        .join("\n")
        .contains("at most 256"));

    value = ai_spec(AiProvider::Anthropic);
    value.models[0].aliases = (0..=AI_MODEL_ALIAS_MAX_COUNT)
        .map(|index| format!("alias-{index}"))
        .collect();
    assert!(value
        .model_alias_table()
        .unwrap_err()
        .join("\n")
        .contains("at most 32 aliases"));
}

#[test]
fn credential_pool_enforces_bounds_aliases_and_weights() {
    let mut value = ai_spec(AiProvider::OpenAI);
    value.credential_pool.credentials.clear();
    assert!(value.validate().unwrap_err().join("\n").contains("at least one"));

    value = ai_spec(AiProvider::OpenAI);
    value.credential_pool.credentials = vec![value.credential_pool.credentials[0].clone(); AI_CREDENTIAL_MAX_COUNT + 1];
    let errors = value.validate().unwrap_err().join("\n");
    assert!(errors.contains("at most 16"));
    assert!(errors.contains("duplicate credential alias"));

    value = ai_spec(AiProvider::OpenAI);
    value.credential_pool.credentials[0].name = "bad alias".to_string();
    value.credential_pool.credentials[0].weight = 0;
    let errors = value.validate().unwrap_err().join("\n");
    assert!(errors.contains("ASCII bytes"));
    assert!(errors.contains("weight must be"));
}

#[test]
fn credential_secret_ref_enforces_core_secret_identity() {
    let mut value = ai_spec(AiProvider::OpenAI);
    let secret_ref = &mut value.credential_pool.credentials[0].credential.secret_ref;
    secret_ref.name.clear();
    secret_ref.group = Some("external.example".to_string());
    secret_ref.kind = Some("ExternalSecret".to_string());
    secret_ref.namespace = Some("Invalid_Namespace".to_string());
    let errors = value.validate().unwrap_err().join("\n");
    assert!(errors.contains("secretRef.name"));
    assert!(errors.contains("secretRef.group"));
    assert!(errors.contains("secretRef.kind"));
    assert!(errors.contains("secretRef.namespace"));

    let mut value = ai_spec(AiProvider::OpenAI);
    let secret_ref = &mut value.credential_pool.credentials[0].credential.secret_ref;
    secret_ref.group = Some("core".to_string());
    secret_ref.kind = Some("Secret".to_string());
    secret_ref.namespace = Some("credential-ns".to_string());
    assert!(value.validate().is_ok());
}

#[test]
fn credential_limits_require_redis_ref_if_and_only_if_present() {
    let mut value = ai_spec(AiProvider::OpenAI);
    value.credential_pool.redis_ref = Some("default/ai-quota".to_string());
    assert!(value.validate().unwrap_err().join("\n").contains("must be absent"));

    value.credential_pool.credentials[0].limits = Some(AiCredentialLimits {
        requests_per_minute: Some(500),
        tokens_per_minute: Some(200_000),
    });
    value.defaults.max_output_tokens = Some(4096);
    assert!(value.validate().is_ok());

    value.credential_pool.redis_ref = None;
    assert!(value.validate().unwrap_err().join("\n").contains("is required"));

    value.credential_pool.redis_ref = Some("invalid".to_string());
    assert!(value.validate().unwrap_err().join("\n").contains("namespace/name"));

    value.credential_pool.redis_ref = Some("default/ai-quota".to_string());
    value.credential_pool.credentials[0].limits = Some(AiCredentialLimits {
        requests_per_minute: None,
        tokens_per_minute: None,
    });
    assert!(value
        .validate()
        .unwrap_err()
        .join("\n")
        .contains("must configure requestsPerMinute or tokensPerMinute"));
}

#[test]
fn tpm_limit_requires_service_max_output_bound() {
    let mut value = ai_spec(AiProvider::OpenAI);
    value.credential_pool.redis_ref = Some("default/ai-quota".to_string());
    value.credential_pool.credentials[0].limits = Some(AiCredentialLimits {
        requests_per_minute: None,
        tokens_per_minute: Some(10_000),
    });

    assert!(value
        .validate()
        .unwrap_err()
        .iter()
        .any(|error| error.contains("tokensPerMinute requires defaults.maxOutputTokens")));

    value.defaults.max_output_tokens = Some(4096);
    assert!(value.validate().is_ok());
}

#[test]
fn pricing_parses_fixed_point_and_defaults_cache_rates_to_input() {
    let pricing = AiModelPricing {
        input_per_million_usd: "15.25".to_string(),
        output_per_million_usd: "75".to_string(),
        cache_read_input_per_million_usd: Some("1.5".to_string()),
        cache_write_input_per_million_usd: None,
    };
    let parsed = pricing.parsed().unwrap();
    assert_eq!(parsed.input.get(), 15_250_000);
    assert_eq!(parsed.output.get(), 75_000_000);
    assert_eq!(parsed.cache_read_input.get(), 1_500_000);
    assert_eq!(parsed.cache_write_input, parsed.input);
}

#[test]
fn pricing_rejects_non_decimal_excess_precision_and_excessive_values() {
    for invalid in ["", "-1", "+1", "1e2", "1.0000001", "1000000.000001"] {
        let mut value = ai_spec(AiProvider::OpenAI);
        value.models[0].pricing = Some(AiModelPricing {
            input_per_million_usd: invalid.to_string(),
            output_per_million_usd: "1".to_string(),
            cache_read_input_per_million_usd: None,
            cache_write_input_per_million_usd: None,
        });
        assert!(value.validate().is_err(), "accepted invalid price {invalid:?}");
    }
}

#[test]
fn max_output_tokens_rejects_zero() {
    let mut value = ai_spec(AiProvider::Anthropic);
    value.defaults.max_output_tokens = Some(0);
    assert!(value
        .validate()
        .unwrap_err()
        .join("\n")
        .contains("defaults.maxOutputTokens must be greater than zero"));

    value.defaults.max_output_tokens = Some(1);
    assert!(value.validate().is_ok());
}

#[test]
fn endpoint_policy_is_enforced() {
    let openai = ai_spec(AiProvider::OpenAI);
    assert_eq!(openai.effective_endpoint().unwrap().as_str(), "https://api.openai.com/");

    let anthropic = ai_spec(AiProvider::Anthropic);
    assert_eq!(
        anthropic.effective_endpoint().unwrap().as_str(),
        "https://api.anthropic.com/"
    );

    let mut compatible = ai_spec(AiProvider::OpenAICompatible);
    assert!(compatible.effective_endpoint().unwrap_err().contains("required"));

    for endpoint in [
        "not a URL",
        "https://user@example.com/v1",
        "https://example.com/v1?q=1",
        "https://example.com/v1#part",
    ] {
        compatible.endpoint = Some(endpoint.to_string());
        assert!(compatible.effective_endpoint().is_err(), "accepted {endpoint}");
    }

    compatible.endpoint = Some("http://example.com/v1".to_string());
    assert!(compatible
        .effective_endpoint()
        .unwrap_err()
        .contains("allowInsecureHttp"));
    compatible.allow_insecure_http = true;
    assert_eq!(compatible.effective_endpoint().unwrap().path(), "/v1");

    compatible.endpoint = Some("https://example.com/openai/v1".to_string());
    compatible.allow_insecure_http = false;
    assert_eq!(compatible.effective_endpoint().unwrap().path(), "/openai/v1");
}

#[test]
fn resolved_secret_is_redacted_for_admin_but_preserved_for_transport() {
    let mut value = spec(AiProvider::OpenAI);
    let mut data = BTreeMap::new();
    data.insert("api-key".to_string(), ByteString(b"private-key".to_vec()));
    value.ai.credential_pool.credentials[0].credential.secret = Some(Secret {
        data: Some(data),
        ..Default::default()
    });

    let transport = serde_json::to_value(&value).unwrap();
    assert!(transport["ai"]["credentialPool"]["credentials"][0]["secret"]["data"]
        .get("api-key")
        .is_some());
    let roundtrip: EdgionBackendSpec = serde_json::from_value(transport.clone()).unwrap();
    assert_eq!(
        roundtrip
            .ai
            .credential_pool
            .credentials
            .first()
            .and_then(|credential| credential.credential.secret.as_ref())
            .and_then(|secret| secret.data.as_ref())
            .and_then(|data| data.get("api-key"))
            .map(|value| value.0.as_slice()),
        Some(b"private-key".as_slice())
    );
    let admin = {
        let _guard = AdminRedactGuard::new();
        serde_json::to_value(&value).unwrap()
    };
    assert_eq!(admin["ai"]["credentialPool"]["credentials"][0]["secret"], "[redacted]");
    assert!(!admin.to_string().contains("private-key"));
}
