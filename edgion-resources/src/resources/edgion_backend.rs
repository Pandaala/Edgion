//! EdgionBackend resource and its AI provider/model catalog.

use std::collections::{HashMap, HashSet};

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use super::common::{ConditionsStatus, ControllerStatus, SecretObjectReference, SecretSlot};
use super::edgion_plugins::OnRedisFailure;

pub const OPENAI_DEFAULT_ENDPOINT: &str = "https://api.openai.com";
pub const ANTHROPIC_DEFAULT_ENDPOINT: &str = "https://api.anthropic.com";
pub const AI_MODEL_NAME_MAX_BYTES: usize = 512;
pub const AI_MODEL_MAX_COUNT: usize = 256;
pub const AI_MODEL_ALIAS_MAX_COUNT: usize = 32;
pub const AI_CREDENTIAL_MAX_COUNT: usize = 16;
pub const AI_CREDENTIAL_ALIAS_MAX_BYTES: usize = 64;
pub const AI_CREDENTIAL_ALIAS_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$";
pub const AI_REDIS_REF_PATTERN: &str = r"^[A-Za-z0-9_-]+/[A-Za-z0-9_-]+$";
pub const AI_CREDENTIAL_WEIGHT_MAX: u32 = 1_000_000;
pub const AI_CREDENTIAL_LIMIT_MAX: u64 = 9_007_199_254_740_991;
pub const AI_PRICE_MAX_USD_PER_MILLION: u64 = 1_000_000;
pub const AI_PRICE_MAX_DECIMAL_PLACES: usize = 6;
pub const AI_PRICE_PATTERN: &str = r"^[0-9]+(\.[0-9]{1,6})?$";
pub const KUBE_DNS_LABEL_PATTERN: &str = r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$";

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "EdgionBackend",
    plural = "edgionbackends",
    shortname = "eb",
    namespaced,
    status = "EdgionBackendStatus"
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgionBackendSpec {
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<ConditionsStatus>,

    pub ai: AiBackendSpec,
    /// Controller-only resolution outcome used to build current-generation status.
    #[serde(skip)]
    #[schemars(skip)]
    pub reconcile_outcome: Option<EdgionBackendReconcileOutcome>,
}

/// Phase-1 AI capability nested under `EdgionBackend.spec.ai`.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiBackendSpec {
    pub provider: AiProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub allow_insecure_http: bool,
    pub credential_pool: AiCredentialPool,
    #[schemars(length(min = 1, max = 256))]
    pub models: Vec<AiModel>,
    #[serde(default)]
    pub defaults: AiBackendDefaults,
}

/// Bounded, non-secret result of resolving the current credential pool.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiBackendReconcileOutcome {
    pub resolved_count: usize,
    pub total_count: usize,
    pub credential_errors: Vec<AiCredentialResolutionError>,
}

/// Controller-only status input for the selected backend variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdgionBackendReconcileOutcome {
    Ai(AiBackendReconcileOutcome),
}

/// One failed credential resolution, safe for bounded status construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiCredentialResolutionError {
    pub alias: String,
    pub class: AiCredentialResolutionClass,
}

/// Static resolution class. It deliberately carries no Secret, URL, or raw error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiCredentialResolutionClass {
    InvalidReference,
    RefNotPermitted,
    SecretNotFound,
    MissingKeys,
    DecodeError,
    InvalidSecretType,
}

impl AiCredentialResolutionClass {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::InvalidReference => "InvalidReference",
            Self::RefNotPermitted => "RefNotPermitted",
            Self::SecretNotFound => "SecretNotFound",
            Self::MissingKeys => "MissingKeys",
            Self::DecodeError => "DecodeError",
            Self::InvalidSecretType => "InvalidSecretType",
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiCredentialPool {
    /// Redis LinkSys reference in `namespace/name` form. Required exactly when
    /// at least one credential declares an RPM or TPM limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(pattern(AI_REDIS_REF_PATTERN))]
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
    /// Whether this canonical model and its direct aliases are visible through
    /// the route-scoped OpenAI model-list endpoint.
    #[serde(default = "default_model_public")]
    pub public: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<AiModelPricing>,
}

const fn default_model_public() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiModelCatalogEntry {
    pub canonical_name: String,
    pub public: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiModelWildcardAlias {
    pub pattern: String,
    pub prefix: String,
    pub entry: AiModelCatalogEntry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiModelCatalog {
    pub exact: HashMap<String, AiModelCatalogEntry>,
    /// Descending literal-prefix length, then ascending pattern for stable
    /// diagnostics and deterministic lookup.
    pub wildcards: Vec<AiModelWildcardAlias>,
}

impl AiModelCatalog {
    pub fn resolve(&self, requested: &str) -> Option<&AiModelCatalogEntry> {
        self.exact.get(requested).or_else(|| {
            self.wildcards
                .iter()
                .find(|alias| requested.starts_with(&alias.prefix))
                .map(|alias| &alias.entry)
        })
    }
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
pub struct AiBackendDefaults {
    #[serde(default = "default_streaming")]
    pub streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub max_output_tokens: Option<u32>,
}

impl Default for AiBackendDefaults {
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

pub type EdgionBackendStatus = ControllerStatus<ConditionsStatus>;

impl EdgionBackendSpec {
    /// Validate the AI backend configuration.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        self.ai.validate()
    }
}

/// Normalize one absolute external HTTP(S) endpoint for backend variants.
pub fn normalize_external_http_endpoint(
    endpoint: &str,
    allow_insecure_http: bool,
    field: &str,
) -> Result<String, String> {
    if endpoint.chars().any(char::is_control) {
        return Err(format!("{field} must not contain control characters"));
    }
    let mut parsed = Url::parse(endpoint).map_err(|_| format!("{field} must be an absolute HTTP or HTTPS URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(format!("{field} must be an absolute HTTP or HTTPS URL"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!("{field} must not contain user information"));
    }
    if parsed.port() == Some(0) {
        return Err(format!("{field} port must be in 1..=65535"));
    }
    if parsed.query().is_some() {
        return Err(format!("{field} must not contain a query"));
    }
    if parsed.fragment().is_some() {
        return Err(format!("{field} must not contain a fragment"));
    }
    if parsed.scheme() == "http" && !allow_insecure_http {
        return Err(format!("HTTP {field} requires allowInsecureHttp: true"));
    }
    let lower_endpoint = endpoint.to_ascii_lowercase();
    let path = parsed.path();
    if endpoint.contains('\\')
        || path.contains("//")
        || lower_endpoint.contains("%2f")
        || lower_endpoint.contains("%5c")
        || lower_endpoint.contains("%2e")
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
    {
        return Err(format!("{field} path must be unambiguous and confined"));
    }
    if parsed.path() != "/" {
        let normalized_path = parsed.path().trim_end_matches('/').to_string();
        parsed.set_path(&normalized_path);
    }
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

impl AiBackendSpec {
    /// Declarative accounting dependency, including unresolved and zero-weight credentials.
    pub fn has_credential_tpm(&self) -> bool {
        self.credential_pool.credentials.iter().any(|credential| {
            credential
                .limits
                .as_ref()
                .is_some_and(|limits| limits.tokens_per_minute.is_some())
        })
    }

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

    /// Builds the cold-path exact and suffix-wildcard model catalog.
    pub fn model_catalog(&self) -> Result<AiModelCatalog, Vec<String>> {
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
            if model.name.contains('*') {
                errors.push(format!("model name {:?} must not contain '*'", model.name));
                continue;
            }
            if !canonical_names.insert(model.name.as_str()) {
                errors.push(format!("duplicate model name {:?}", model.name));
            }
        }

        let mut exact = HashMap::new();
        for model in &self.models {
            if !model.name.trim().is_empty() {
                exact.entry(model.name.clone()).or_insert_with(|| AiModelCatalogEntry {
                    canonical_name: model.name.clone(),
                    public: model.public,
                });
            }
        }
        let mut wildcard_patterns = HashSet::new();
        let mut wildcards = Vec::new();
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
                let star_count = alias.bytes().filter(|byte| *byte == b'*').count();
                if star_count > 0 {
                    if star_count != 1 || !alias.ends_with('*') || alias.len() == 1 {
                        errors.push(format!(
                            "model alias {alias:?} must use at most one trailing '*' after a non-empty prefix"
                        ));
                        continue;
                    }
                    if !wildcard_patterns.insert(alias.as_str()) {
                        errors.push(format!("duplicate wildcard model alias {alias:?}"));
                        continue;
                    }
                    wildcards.push(AiModelWildcardAlias {
                        pattern: alias.clone(),
                        prefix: alias[..alias.len() - 1].to_string(),
                        entry: AiModelCatalogEntry {
                            canonical_name: model.name.clone(),
                            public: model.public,
                        },
                    });
                    continue;
                }
                if canonical_names.contains(alias.as_str()) {
                    errors.push(format!("model alias {alias:?} collides with a canonical name"));
                    continue;
                }
                if let Some(existing) = exact.insert(
                    alias.clone(),
                    AiModelCatalogEntry {
                        canonical_name: model.name.clone(),
                        public: model.public,
                    },
                ) {
                    errors.push(format!(
                        "model alias {alias:?} is assigned to both {:?} and {:?}",
                        existing.canonical_name, model.name
                    ));
                }
            }
        }

        wildcards.sort_by(|left, right| {
            right
                .prefix
                .len()
                .cmp(&left.prefix.len())
                .then_with(|| left.pattern.cmp(&right.pattern))
        });

        if errors.is_empty() {
            Ok(AiModelCatalog { exact, wildcards })
        } else {
            Err(errors)
        }
    }

    /// Exact-name compatibility view used by older cold-path consumers.
    pub fn model_alias_table(&self) -> Result<HashMap<String, String>, Vec<String>> {
        self.model_catalog().map(|catalog| {
            catalog
                .exact
                .into_iter()
                .map(|(name, entry)| (name, entry.canonical_name))
                .collect()
        })
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
        if let Err(model_errors) = self.model_catalog() {
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
            errors.extend(secret_ref_validation_errors(
                &credential.credential.secret_ref,
                &format!("credentialPool.credentials[{index}].secretRef"),
            ));
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

fn valid_dns_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && value.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
}

fn valid_kube_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 253 && value.split('.').all(valid_dns_label)
}

fn secret_ref_validation_errors(secret_ref: &SecretObjectReference, field: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if !matches!(secret_ref.group.as_deref(), None | Some("") | Some("core")) {
        errors.push(format!("{field}.group must be empty or core"));
    }
    if !matches!(secret_ref.kind.as_deref(), None | Some("Secret")) {
        errors.push(format!("{field}.kind must be Secret"));
    }
    if !valid_kube_name(&secret_ref.name) {
        errors.push(format!("{field}.name must be a valid non-empty Kubernetes name"));
    }
    if secret_ref
        .namespace
        .as_deref()
        .is_some_and(|namespace| !valid_dns_label(namespace))
    {
        errors.push(format!("{field}.namespace must be a valid Kubernetes namespace"));
    }
    errors
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
#[path = "../../tests/unit/resources/edgion_backend_test.rs"]
mod tests;
