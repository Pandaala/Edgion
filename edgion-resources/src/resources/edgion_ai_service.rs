//! AI provider and model catalog consumed by AI Gateway routes.

use std::collections::{HashMap, HashSet};

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use super::common::{Condition, SecretSlot};
use super::edgion_plugins::OnRedisFailure;

pub const OPENAI_DEFAULT_ENDPOINT: &str = "https://api.openai.com";
pub const ANTHROPIC_DEFAULT_ENDPOINT: &str = "https://api.anthropic.com";
pub const AI_MODEL_NAME_MAX_BYTES: usize = 512;
pub const AI_MODEL_MAX_COUNT: usize = 256;
pub const AI_MODEL_ALIAS_MAX_COUNT: usize = 32;
pub const AI_CREDENTIAL_MAX_COUNT: usize = 16;
pub const AI_CREDENTIAL_ALIAS_MAX_BYTES: usize = 64;
pub const AI_CREDENTIAL_ALIAS_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$";
pub const AI_CREDENTIAL_WEIGHT_MAX: u32 = 1_000_000;
pub const AI_CREDENTIAL_LIMIT_MAX: u64 = 9_007_199_254_740_991;
pub const AI_PRICE_MAX_USD_PER_MILLION: u64 = 1_000_000;
pub const AI_PRICE_MAX_DECIMAL_PLACES: usize = 6;
pub const AI_PRICE_PATTERN: &str = r"^[0-9]+(\.[0-9]{1,6})?$";

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "EdgionAIService",
    plural = "edgionaiservices",
    shortname = "eais",
    namespaced,
    status = "EdgionAIServiceStatus"
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgionAIServiceSpec {
    pub provider: AiProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub allow_insecure_http: bool,
    pub credential_pool: AiCredentialPool,
    #[schemars(length(min = 1, max = 256))]
    pub models: Vec<AiModel>,
    #[serde(default)]
    pub defaults: AiServiceDefaults,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiCredentialPool {
    /// Redis LinkSys reference in `namespace/name` form. Required exactly when
    /// at least one credential declares an RPM or TPM limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redis_ref: Option<String>,
    /// Redis dependency failure is fail-close by default.
    #[serde(default = "default_on_redis_failure_deny")]
    pub on_redis_failure: OnRedisFailure,
    #[schemars(length(min = 1, max = 16))]
    pub credentials: Vec<AiCredential>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiCredential {
    /// Stable, non-secret identifier used for bounded status and access-log attribution.
    #[schemars(length(min = 1, max = 64), pattern(AI_CREDENTIAL_ALIAS_PATTERN))]
    pub name: String,
    #[serde(default = "default_credential_weight")]
    #[schemars(range(min = 1, max = 1_000_000))]
    pub weight: u32,
    #[serde(flatten)]
    pub credential: SecretSlot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<AiCredentialLimits>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiCredentialLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 9007199254740991_u64))]
    pub requests_per_minute: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 9007199254740991_u64))]
    pub tokens_per_minute: Option<u64>,
}

const fn default_on_redis_failure_deny() -> OnRedisFailure {
    OnRedisFailure::Deny
}

const fn default_credential_weight() -> u32 {
    1
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, PartialEq, Eq)]
pub enum AiProvider {
    OpenAI,
    OpenAICompatible,
    Anthropic,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiModel {
    #[schemars(length(max = 512))]
    pub name: String,
    #[serde(default)]
    #[schemars(length(max = 32), inner(length(min = 1, max = 512)))]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<AiModelPricing>,
}

/// Estimated USD prices per one million tokens for one canonical model.
///
/// Decimal strings are used deliberately: resource configuration must not
/// introduce floating-point rounding into cost audit records.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiModelPricing {
    #[schemars(length(max = 14), pattern(AI_PRICE_PATTERN))]
    pub input_per_million_usd: String,
    #[schemars(length(max = 14), pattern(AI_PRICE_PATTERN))]
    pub output_per_million_usd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 14), pattern(AI_PRICE_PATTERN))]
    pub cache_read_input_per_million_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 14), pattern(AI_PRICE_PATTERN))]
    pub cache_write_input_per_million_usd: Option<String>,
}

/// Fixed-point price represented as micro-USD per one million tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiPriceMicrosPerMillion(u64);

impl AiPriceMicrosPerMillion {
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl AiModelPricing {
    /// Parse every configured price on the configuration cold path.
    pub fn parsed(&self) -> Result<ParsedAiModelPricing, String> {
        let input = parse_price(&self.input_per_million_usd, "inputPerMillionUsd")?;
        let output = parse_price(&self.output_per_million_usd, "outputPerMillionUsd")?;
        let cache_read = self
            .cache_read_input_per_million_usd
            .as_deref()
            .map(|value| parse_price(value, "cacheReadInputPerMillionUsd"))
            .transpose()?
            .unwrap_or(input);
        let cache_write = self
            .cache_write_input_per_million_usd
            .as_deref()
            .map(|value| parse_price(value, "cacheWriteInputPerMillionUsd"))
            .transpose()?
            .unwrap_or(input);
        Ok(ParsedAiModelPricing {
            input,
            output,
            cache_read_input: cache_read,
            cache_write_input: cache_write,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedAiModelPricing {
    pub input: AiPriceMicrosPerMillion,
    pub output: AiPriceMicrosPerMillion,
    pub cache_read_input: AiPriceMicrosPerMillion,
    pub cache_write_input: AiPriceMicrosPerMillion,
}

fn parse_price(value: &str, field: &str) -> Result<AiPriceMicrosPerMillion, String> {
    if value.is_empty() || value.starts_with('+') || value.starts_with('-') {
        return Err(format!("pricing.{field} must be a non-negative decimal string"));
    }
    let mut parts = value.split('.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.is_some_and(|part| {
            part.is_empty()
                || part.len() > AI_PRICE_MAX_DECIMAL_PLACES
                || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(format!(
            "pricing.{field} must be a decimal string with at most {AI_PRICE_MAX_DECIMAL_PLACES} fractional digits"
        ));
    }
    let whole_value = whole
        .parse::<u64>()
        .map_err(|_| format!("pricing.{field} is too large"))?;
    if whole_value > AI_PRICE_MAX_USD_PER_MILLION {
        return Err(format!(
            "pricing.{field} must not exceed {AI_PRICE_MAX_USD_PER_MILLION} USD per million tokens"
        ));
    }
    let fraction_value = match fraction {
        Some(part) => {
            let parsed = part.parse::<u64>().map_err(|_| format!("pricing.{field} is invalid"))?;
            parsed * 10_u64.pow((AI_PRICE_MAX_DECIMAL_PLACES - part.len()) as u32)
        }
        None => 0,
    };
    if whole_value == AI_PRICE_MAX_USD_PER_MILLION && fraction_value != 0 {
        return Err(format!(
            "pricing.{field} must not exceed {AI_PRICE_MAX_USD_PER_MILLION} USD per million tokens"
        ));
    }
    Ok(AiPriceMicrosPerMillion(whole_value * 1_000_000 + fraction_value))
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiServiceDefaults {
    #[serde(default = "default_streaming")]
    pub streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub max_output_tokens: Option<u32>,
}

impl Default for AiServiceDefaults {
    fn default() -> Self {
        Self {
            streaming: default_streaming(),
            max_output_tokens: None,
        }
    }
}

const fn default_streaming() -> bool {
    true
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgionAIServiceStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

impl EdgionAIServiceSpec {
    /// Resolves and validates the provider endpoint without performing I/O.
    pub fn effective_endpoint(&self) -> Result<Url, String> {
        let endpoint = match (&self.provider, self.endpoint.as_deref()) {
            (AiProvider::OpenAI, None) => OPENAI_DEFAULT_ENDPOINT,
            (AiProvider::Anthropic, None) => ANTHROPIC_DEFAULT_ENDPOINT,
            (AiProvider::OpenAICompatible, None) => {
                return Err("endpoint is required for provider OpenAICompatible".to_string());
            }
            (_, Some(endpoint)) => endpoint,
        };

        let parsed = Url::parse(endpoint).map_err(|error| format!("invalid endpoint: {error}"))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err("endpoint must be an absolute HTTP or HTTPS URL".to_string());
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("endpoint must not contain user information".to_string());
        }
        if parsed.query().is_some() {
            return Err("endpoint must not contain a query".to_string());
        }
        if parsed.fragment().is_some() {
            return Err("endpoint must not contain a fragment".to_string());
        }
        if parsed.scheme() == "http" && !self.allow_insecure_http {
            return Err("HTTP endpoint requires allowInsecureHttp: true".to_string());
        }
        Ok(parsed)
    }

    /// Builds the cold-path lookup table from canonical names and aliases.
    pub fn model_alias_table(&self) -> Result<HashMap<String, String>, Vec<String>> {
        let mut errors = Vec::new();
        if self.models.is_empty() {
            errors.push("models must contain at least one entry".to_string());
        }
        if self.models.len() > AI_MODEL_MAX_COUNT {
            errors.push(format!("models must contain at most {AI_MODEL_MAX_COUNT} entries"));
        }

        let mut canonical_names = HashSet::new();
        for model in &self.models {
            if model.name.trim().is_empty() {
                errors.push("model name must not be empty".to_string());
                continue;
            }
            if model.name.len() > AI_MODEL_NAME_MAX_BYTES {
                errors.push(format!(
                    "model name must not exceed {AI_MODEL_NAME_MAX_BYTES} UTF-8 bytes"
                ));
                continue;
            }
            if !canonical_names.insert(model.name.as_str()) {
                errors.push(format!("duplicate model name {:?}", model.name));
            }
        }

        let mut table = HashMap::new();
        for model in &self.models {
            if !model.name.trim().is_empty() {
                table.entry(model.name.clone()).or_insert_with(|| model.name.clone());
            }
        }
        for model in &self.models {
            if model.aliases.len() > AI_MODEL_ALIAS_MAX_COUNT {
                errors.push(format!(
                    "model {:?} must contain at most {AI_MODEL_ALIAS_MAX_COUNT} aliases",
                    model.name
                ));
            }
            for alias in &model.aliases {
                if alias.trim().is_empty() {
                    errors.push(format!("model {:?} has an empty alias", model.name));
                    continue;
                }
                if alias.len() > AI_MODEL_NAME_MAX_BYTES {
                    errors.push(format!(
                        "model alias for {:?} must not exceed {AI_MODEL_NAME_MAX_BYTES} UTF-8 bytes",
                        model.name
                    ));
                    continue;
                }
                if canonical_names.contains(alias.as_str()) {
                    errors.push(format!("model alias {alias:?} collides with a canonical name"));
                    continue;
                }
                if let Some(existing) = table.insert(alias.clone(), model.name.clone()) {
                    errors.push(format!(
                        "model alias {alias:?} is assigned to both {existing:?} and {:?}",
                        model.name
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(table)
        } else {
            Err(errors)
        }
    }

    /// Performs all configuration-only validation shared by ingest consumers.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if let Err(error) = self.effective_endpoint() {
            errors.push(error);
        }
        errors.extend(self.credential_pool.validation_errors());
        if self.defaults.max_output_tokens.is_none() {
            for (index, credential) in self.credential_pool.credentials.iter().enumerate() {
                if credential
                    .limits
                    .as_ref()
                    .and_then(|limits| limits.tokens_per_minute)
                    .is_some()
                {
                    errors.push(format!(
                        "credentialPool.credentials[{index}].limits.tokensPerMinute requires defaults.maxOutputTokens"
                    ));
                }
            }
        }
        if self.defaults.max_output_tokens == Some(0) {
            errors.push("defaults.maxOutputTokens must be greater than zero".to_string());
        }
        if let Err(model_errors) = self.model_alias_table() {
            errors.extend(model_errors);
        }
        for model in &self.models {
            if let Some(pricing) = &model.pricing {
                if let Err(error) = pricing.parsed() {
                    errors.push(format!("model {:?}: {error}", model.name));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl AiCredentialPool {
    fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.credentials.is_empty() {
            errors.push("credentialPool.credentials must contain at least one entry".to_string());
        }
        if self.credentials.len() > AI_CREDENTIAL_MAX_COUNT {
            errors.push(format!(
                "credentialPool.credentials must contain at most {AI_CREDENTIAL_MAX_COUNT} entries"
            ));
        }

        let mut aliases = HashSet::new();
        let mut has_limits = false;
        for (index, credential) in self.credentials.iter().enumerate() {
            if !valid_credential_alias(&credential.name) {
                errors.push(format!(
                    "credentialPool.credentials[{index}].name must be 1..={AI_CREDENTIAL_ALIAS_MAX_BYTES} ASCII bytes and contain only letters, digits, '.', '_', or '-'"
                ));
            } else if !aliases.insert(credential.name.as_str()) {
                errors.push(format!("duplicate credential alias {:?}", credential.name));
            }
            if !(1..=AI_CREDENTIAL_WEIGHT_MAX).contains(&credential.weight) {
                errors.push(format!(
                    "credentialPool.credentials[{index}].weight must be in 1..={AI_CREDENTIAL_WEIGHT_MAX}"
                ));
            }
            if let Some(limits) = &credential.limits {
                has_limits = true;
                if limits.requests_per_minute.is_none() && limits.tokens_per_minute.is_none() {
                    errors.push(format!(
                        "credentialPool.credentials[{index}].limits must configure requestsPerMinute or tokensPerMinute"
                    ));
                }
                for (field, value) in [
                    ("requestsPerMinute", limits.requests_per_minute),
                    ("tokensPerMinute", limits.tokens_per_minute),
                ] {
                    if value.is_some_and(|value| value == 0 || value > AI_CREDENTIAL_LIMIT_MAX) {
                        errors.push(format!(
                            "credentialPool.credentials[{index}].limits.{field} must be in 1..={AI_CREDENTIAL_LIMIT_MAX}"
                        ));
                    }
                }
            }
        }

        match (has_limits, self.redis_ref.as_deref()) {
            (true, None | Some("")) => {
                errors.push("credentialPool.redisRef is required when any credential limit is configured".to_string());
            }
            (false, Some(_)) => {
                errors
                    .push("credentialPool.redisRef must be absent when no credential limit is configured".to_string());
            }
            (_, Some(redis_ref)) if !valid_redis_ref(redis_ref) => {
                errors.push("credentialPool.redisRef must use namespace/name form".to_string());
            }
            _ => {}
        }

        errors
    }
}

fn valid_credential_alias(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= AI_CREDENTIAL_ALIAS_MAX_BYTES
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'.' | b'_' | b'-')))
}

fn valid_redis_ref(value: &str) -> bool {
    let mut parts = value.split('/');
    let (Some(namespace), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !namespace.is_empty()
        && !name.is_empty()
        && [namespace, name].iter().all(|part| {
            part.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use kube::CustomResourceExt;

    use super::*;
    use crate::redact::AdminRedactGuard;
    use crate::resources::common::SecretObjectReference;

    fn spec(provider: AiProvider) -> EdgionAIServiceSpec {
        EdgionAIServiceSpec {
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
                pricing: None,
            }],
            defaults: AiServiceDefaults::default(),
        }
    }

    #[test]
    fn crd_identity_is_stable() {
        let crd = EdgionAIService::crd();
        assert_eq!(crd.spec.group, "edgion.io");
        assert_eq!(crd.spec.versions[0].name, "v1");
        assert_eq!(crd.spec.names.kind, "EdgionAIService");
        assert_eq!(crd.spec.names.plural, "edgionaiservices");
        assert_eq!(crd.spec.names.short_names, Some(vec!["eais".to_string()]));
        assert_eq!(crd.spec.scope, "Namespaced");
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
        let parsed: EdgionAIServiceSpec = serde_yaml::from_str(
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
        let parsed: EdgionAIServiceSpec = serde_yaml::from_str(
            "provider: Anthropic\ncredentialPool:\n  credentials:\n    - name: primary\n      secretRef:\n        name: ai-credential\nmodels:\n  - name: model-a\ndefaults:\n  maxOutputTokens: 4096\n",
        )
        .unwrap();
        assert_eq!(parsed.defaults.max_output_tokens, Some(4096));

        let serialized = serde_json::to_value(&parsed).unwrap();
        assert_eq!(serialized["defaults"]["maxOutputTokens"], 4096);
        assert!(serialized["defaults"].get("max_output_tokens").is_none());

        let schema = serde_json::to_value(schemars::schema_for!(AiServiceDefaults)).unwrap();
        assert_eq!(schema["properties"]["maxOutputTokens"]["minimum"], 1);
    }

    #[test]
    fn rejects_inline_or_unknown_credential_fields() {
        for field in ["apiKey", "password"] {
            let yaml = format!(
                "provider: OpenAI\ncredentialPool:\n  credentials:\n    - name: primary\n      secretRef:\n        name: ai-credential\n      {field}: must-not-be-accepted\nmodels:\n  - name: model-a\n"
            );
            assert!(
                serde_yaml::from_str::<EdgionAIServiceSpec>(&yaml).is_err(),
                "unknown inline credential field {field} must be rejected"
            );
        }
    }

    #[test]
    fn model_catalog_rejects_empty_and_colliding_names() {
        let mut value = spec(AiProvider::OpenAI);
        value.models.clear();
        assert!(value.model_alias_table().unwrap_err()[0].contains("at least one"));

        value.models = vec![
            AiModel {
                name: "model-a".to_string(),
                aliases: vec!["".to_string(), "model-b".to_string(), "shared".to_string()],
                pricing: None,
            },
            AiModel {
                name: "model-b".to_string(),
                aliases: vec!["shared".to_string()],
                pricing: None,
            },
            AiModel {
                name: "model-b".to_string(),
                aliases: Vec::new(),
                pricing: None,
            },
            AiModel {
                name: " ".to_string(),
                aliases: Vec::new(),
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
        let table = spec(AiProvider::OpenAI).model_alias_table().unwrap();
        assert_eq!(table.get("model-a").map(String::as_str), Some("model-a"));
        assert_eq!(table.get("default").map(String::as_str), Some("model-a"));
    }

    #[test]
    fn model_catalog_bounds_names_and_aliases() {
        let mut value = spec(AiProvider::Anthropic);
        value.models[0].name = "m".repeat(AI_MODEL_NAME_MAX_BYTES + 1);
        value.models[0].aliases = vec!["a".repeat(AI_MODEL_NAME_MAX_BYTES + 1)];
        let errors = value.model_alias_table().unwrap_err().join("\n");
        assert!(errors.contains("model name must not exceed"));
        assert!(errors.contains("model alias"));

        value = spec(AiProvider::Anthropic);
        value.models = vec![value.models[0].clone(); AI_MODEL_MAX_COUNT + 1];
        assert!(value
            .model_alias_table()
            .unwrap_err()
            .join("\n")
            .contains("at most 256"));

        value = spec(AiProvider::Anthropic);
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
        let mut value = spec(AiProvider::OpenAI);
        value.credential_pool.credentials.clear();
        assert!(value.validate().unwrap_err().join("\n").contains("at least one"));

        value = spec(AiProvider::OpenAI);
        value.credential_pool.credentials =
            vec![value.credential_pool.credentials[0].clone(); AI_CREDENTIAL_MAX_COUNT + 1];
        let errors = value.validate().unwrap_err().join("\n");
        assert!(errors.contains("at most 16"));
        assert!(errors.contains("duplicate credential alias"));

        value = spec(AiProvider::OpenAI);
        value.credential_pool.credentials[0].name = "bad alias".to_string();
        value.credential_pool.credentials[0].weight = 0;
        let errors = value.validate().unwrap_err().join("\n");
        assert!(errors.contains("ASCII bytes"));
        assert!(errors.contains("weight must be"));
    }

    #[test]
    fn credential_limits_require_redis_ref_if_and_only_if_present() {
        let mut value = spec(AiProvider::OpenAI);
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
        let mut value = spec(AiProvider::OpenAI);
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
            let mut value = spec(AiProvider::OpenAI);
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
        let mut value = spec(AiProvider::Anthropic);
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
        let openai = spec(AiProvider::OpenAI);
        assert_eq!(openai.effective_endpoint().unwrap().as_str(), "https://api.openai.com/");

        let anthropic = spec(AiProvider::Anthropic);
        assert_eq!(
            anthropic.effective_endpoint().unwrap().as_str(),
            "https://api.anthropic.com/"
        );

        let mut compatible = spec(AiProvider::OpenAICompatible);
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
        value.credential_pool.credentials[0].credential.secret = Some(Secret {
            data: Some(data),
            ..Default::default()
        });

        let transport = serde_json::to_value(&value).unwrap();
        assert!(transport["credentialPool"]["credentials"][0]["secret"]["data"]
            .get("api-key")
            .is_some());
        let roundtrip: EdgionAIServiceSpec = serde_json::from_value(transport.clone()).unwrap();
        assert_eq!(
            roundtrip
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
        assert_eq!(admin["credentialPool"]["credentials"][0]["secret"], "[redacted]");
        assert!(!admin.to_string().contains("private-key"));
    }
}
