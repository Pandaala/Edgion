//! EdgionBackend resource and its AI provider/model catalog.

use std::collections::{HashMap, HashSet};

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use super::common::{Condition, SecretObjectReference, SecretSlot};
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
pub const A2A_SERVICE_NAME_PATTERN: &str = r"^[a-z]([-a-z0-9]{0,61}[a-z0-9])?$";
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai: Option<AiBackendSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a2a: Option<A2aBackendSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpBackendSpec>,
    /// Controller-resolved A2A target shipped to the Gateway over conf_sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_a2a: Option<ResolvedA2aBackend>,
    /// Controller-resolved MCP target shipped to the Gateway over conf_sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_mcp: Option<ResolvedMcpBackend>,
    /// Controller-only resolution outcome used to build current-generation status.
    #[serde(skip)]
    #[schemars(skip)]
    pub reconcile_outcome: Option<EdgionBackendReconcileOutcome>,
}

/// A2A backend identity and transport target.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct A2aBackendSpec {
    pub target: A2aBackendTarget,
    #[serde(default)]
    pub allow_insecure_http: bool,
}

/// Strict exactly-one target union for an A2A backend.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct A2aBackendTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_ref: Option<BackendServiceReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

/// Typed reference to one port of a core Kubernetes Service.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendServiceReference {
    #[schemars(length(min = 1, max = 63), pattern(A2A_SERVICE_NAME_PATTERN))]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 63), pattern(KUBE_DNS_LABEL_PATTERN))]
    pub namespace: Option<String>,
    #[schemars(range(min = 1, max = 65535))]
    pub port: u16,
}

/// MCP backend identity and stateless Streamable HTTP transport target.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpBackendSpec {
    pub target: McpBackendTarget,
    #[serde(default)]
    pub allow_insecure_http: bool,
    pub transport: McpTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<McpAuthentication>,
}

/// Strict exactly-one target union for an MCP backend.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpBackendTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_ref: Option<BackendServiceReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Fixed MCP transport identity. It does not select a wire HTTP version.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpTransport {
    pub r#type: McpTransportType,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, PartialEq, Eq)]
pub enum McpTransportType {
    StreamableHTTP,
}

/// Optional static bearer authentication for an MCP upstream.
#[derive(Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpAuthentication {
    pub bearer_token: SecretSlot,
}

impl std::fmt::Debug for McpAuthentication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpAuthentication")
            .field("secret_ref", &self.bearer_token.secret_ref)
            .field("secret", &"[redacted]")
            .finish()
    }
}

/// Current-generation MCP target proved by the Controller.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedMcpBackend {
    pub target: ResolvedMcpTarget,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ResolvedMcpTarget {
    Service {
        namespace: String,
        name: String,
        port: u16,
        path: String,
    },
    External {
        normalized_endpoint: String,
    },
}

/// Current-generation A2A target proved by the Controller.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedA2aBackend {
    pub target: ResolvedA2aTarget,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ResolvedA2aTarget {
    Service { namespace: String, name: String, port: u16 },
    External { normalized_endpoint: String },
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
    A2a(A2aBackendReconcileOutcome),
    Mcp(McpBackendReconcileOutcome),
}

/// Bounded result of resolving an A2A target reference.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct A2aBackendReconcileOutcome {
    pub error: Option<A2aTargetResolutionClass>,
}

/// Static A2A resolution class. It deliberately carries no target or raw error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum A2aTargetResolutionClass {
    RefNotPermitted,
    TargetNotFound,
    UnsupportedProtocol,
}

/// Independent, bounded MCP target and credential resolution results.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpBackendReconcileOutcome {
    pub target_error: Option<McpTargetResolutionClass>,
    pub credential_error: Option<McpCredentialResolutionClass>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpTargetResolutionClass {
    RefNotPermitted,
    BackendNotFound,
    UnsupportedProtocol,
}

impl McpTargetResolutionClass {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::RefNotPermitted => "RefNotPermitted",
            Self::BackendNotFound => "BackendNotFound",
            Self::UnsupportedProtocol => "UnsupportedProtocol",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpCredentialResolutionClass {
    RefNotPermitted,
    SecretNotFound,
    MissingKeys,
    DecodeError,
    InvalidValue,
    InvalidSecretType,
}

impl McpCredentialResolutionClass {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::RefNotPermitted => "RefNotPermitted",
            Self::SecretNotFound => "SecretNotFound",
            Self::MissingKeys => "MissingKeys",
            Self::DecodeError => "DecodeError",
            Self::InvalidValue => "InvalidValue",
            Self::InvalidSecretType => "InvalidSecretType",
        }
    }
}

impl A2aTargetResolutionClass {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::RefNotPermitted => "RefNotPermitted",
            Self::TargetNotFound => "TargetNotFound",
            Self::UnsupportedProtocol => "UnsupportedProtocol",
        }
    }
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
    RefNotPermitted,
    SecretNotFound,
    MissingKeys,
    DecodeError,
    InvalidSecretType,
}

impl AiCredentialResolutionClass {
    pub const fn reason(self) -> &'static str {
        match self {
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

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgionBackendStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

impl EdgionBackendSpec {
    /// Validate the backend union and its selected capability.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let configured_variants =
            usize::from(self.ai.is_some()) + usize::from(self.a2a.is_some()) + usize::from(self.mcp.is_some());
        if configured_variants != 1 {
            return Err(vec!["spec must configure exactly one backend variant".to_string()]);
        }
        match (self.ai.as_ref(), self.a2a.as_ref(), self.mcp.as_ref()) {
            (Some(ai), None, None) => ai.validate(),
            (None, Some(a2a), None) => a2a.validate(),
            (None, None, Some(mcp)) => mcp.validate(),
            _ => Err(vec!["spec must configure exactly one backend variant".to_string()]),
        }
    }
}

impl A2aBackendSpec {
    /// Validate and normalize this A2A target without performing I/O.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        match (&self.target.service_ref, &self.target.endpoint) {
            (Some(service_ref), None) => {
                if self.allow_insecure_http {
                    errors.push("allowInsecureHttp must be false for a Service target".to_string());
                }
                errors.extend(service_ref.validation_errors("a2a.target.serviceRef"));
            }
            (None, Some(_)) => {
                if let Err(error) = self.normalized_endpoint() {
                    errors.push(error);
                }
            }
            _ => errors.push("a2a.target must configure exactly one of serviceRef or endpoint".to_string()),
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Return the canonical external endpoint used in the resolved target.
    pub fn normalized_endpoint(&self) -> Result<String, String> {
        let endpoint = self
            .target
            .endpoint
            .as_deref()
            .ok_or_else(|| "a2a.target.endpoint is required for an external target".to_string())?;
        normalize_external_http_endpoint(endpoint, self.allow_insecure_http, "a2a.target.endpoint")
    }
}

impl McpBackendSpec {
    /// Validate and normalize this MCP target without performing I/O.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        match (&self.target.service_ref, &self.target.endpoint) {
            (Some(service_ref), None) => {
                if self.allow_insecure_http {
                    errors.push("allowInsecureHttp must be false for a Service target".to_string());
                }
                errors.extend(service_ref.validation_errors("mcp.target.serviceRef"));
                match self.target.path.as_deref() {
                    Some(path) => {
                        if let Err(error) = normalize_absolute_backend_path(path, "mcp.target.path") {
                            errors.push(error);
                        }
                    }
                    None => errors.push("mcp.target.path is required for a Service target".to_string()),
                }
            }
            (None, Some(_)) => {
                if self.target.path.is_some() {
                    errors.push("mcp.target.path must be absent for an external target".to_string());
                }
                if let Err(error) = self.normalized_endpoint() {
                    errors.push(error);
                }
            }
            _ => errors.push("mcp.target must configure exactly one of serviceRef or endpoint".to_string()),
        }
        if let Some(authentication) = &self.authentication {
            errors.extend(secret_ref_validation_errors(
                &authentication.bearer_token.secret_ref,
                "mcp.authentication.bearerToken.secretRef",
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn normalized_service_path(&self) -> Result<String, String> {
        let path = self
            .target
            .path
            .as_deref()
            .ok_or_else(|| "mcp.target.path is required for a Service target".to_string())?;
        normalize_absolute_backend_path(path, "mcp.target.path")
    }

    pub fn normalized_endpoint(&self) -> Result<String, String> {
        let endpoint = self
            .target
            .endpoint
            .as_deref()
            .ok_or_else(|| "mcp.target.endpoint is required for an external target".to_string())?;
        if self.allow_insecure_http
            && Url::parse(endpoint)
                .ok()
                .is_some_and(|parsed| parsed.scheme() == "https")
        {
            return Err("allowInsecureHttp must be false for an HTTPS target".to_string());
        }
        normalize_external_http_endpoint(endpoint, self.allow_insecure_http, "mcp.target.endpoint")
    }
}

impl BackendServiceReference {
    pub fn validation_errors(&self, field: &str) -> Vec<String> {
        let mut errors = Vec::new();
        if !valid_service_name(&self.name) {
            errors.push(format!("{field}.name must be a valid Kubernetes name"));
        }
        if self
            .namespace
            .as_deref()
            .is_some_and(|namespace| !valid_dns_label(namespace))
        {
            errors.push(format!("{field}.namespace must be a valid Kubernetes namespace"));
        }
        if self.port == 0 {
            errors.push(format!("{field}.port must be in 1..=65535"));
        }
        errors
    }
}

/// Normalize one absolute backend path while rejecting ambiguous forms.
pub fn normalize_absolute_backend_path(path: &str, field: &str) -> Result<String, String> {
    let lower_path = path.to_ascii_lowercase();
    if path.is_empty()
        || !path.starts_with('/')
        || path.contains('?')
        || path.contains('#')
        || path.contains('\\')
        || path.contains("//")
        || path.chars().any(char::is_control)
        || lower_path.contains("%2f")
        || lower_path.contains("%5c")
        || lower_path.contains("%2e")
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
    {
        return Err(format!("{field} must be an absolute, unambiguous path"));
    }
    if path == "/" {
        Ok(path.to_string())
    } else {
        Ok(path.trim_end_matches('/').to_string())
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

fn valid_service_name(value: &str) -> bool {
    valid_dns_label(value) && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
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
            ai: Some(ai_spec(provider)),
            a2a: None,
            mcp: None,
            resolved_a2a: None,
            resolved_mcp: None,
            reconcile_outcome: None,
        }
    }

    fn a2a_service_spec() -> EdgionBackendSpec {
        EdgionBackendSpec {
            ai: None,
            a2a: Some(A2aBackendSpec {
                target: A2aBackendTarget {
                    service_ref: Some(BackendServiceReference {
                        name: "agent-service".to_string(),
                        namespace: Some("agents".to_string()),
                        port: 8080,
                    }),
                    endpoint: None,
                },
                allow_insecure_http: false,
            }),
            mcp: None,
            resolved_a2a: None,
            resolved_mcp: None,
            reconcile_outcome: None,
        }
    }

    fn mcp_service_spec() -> EdgionBackendSpec {
        EdgionBackendSpec {
            ai: None,
            a2a: None,
            mcp: Some(McpBackendSpec {
                target: McpBackendTarget {
                    service_ref: Some(BackendServiceReference {
                        name: "mcp-server".to_string(),
                        namespace: Some("tools".to_string()),
                        port: 8080,
                    }),
                    endpoint: None,
                    path: Some("/mcp".to_string()),
                },
                allow_insecure_http: false,
                transport: McpTransport {
                    r#type: McpTransportType::StreamableHTTP,
                },
                authentication: Some(McpAuthentication {
                    bearer_token: SecretSlot {
                        secret_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "mcp-credential".to_string(),
                            namespace: None,
                        },
                        secret: None,
                    },
                }),
            }),
            resolved_a2a: None,
            resolved_mcp: None,
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
    fn backend_union_requires_exactly_one_variant() {
        let missing = EdgionBackendSpec {
            ai: None,
            a2a: None,
            mcp: None,
            resolved_a2a: None,
            resolved_mcp: None,
            reconcile_outcome: None,
        };
        assert_eq!(
            missing.validate().unwrap_err(),
            vec!["spec must configure exactly one backend variant"]
        );
        assert!(spec(AiProvider::OpenAI).validate().is_ok());
        assert!(a2a_service_spec().validate().is_ok());
        assert!(mcp_service_spec().validate().is_ok());

        let mut both = spec(AiProvider::OpenAI);
        both.a2a = a2a_service_spec().a2a;
        assert_eq!(
            both.validate().unwrap_err(),
            vec!["spec must configure exactly one backend variant"]
        );

        for (ai, a2a, mcp) in [(true, false, true), (false, true, true), (true, true, true)] {
            let mut value = mcp_service_spec();
            value.ai = ai.then(|| ai_spec(AiProvider::OpenAI));
            value.a2a = a2a.then(|| a2a_service_spec().a2a.unwrap());
            value.mcp = mcp.then(|| mcp_service_spec().mcp.unwrap());
            assert_eq!(
                value.validate().unwrap_err(),
                vec!["spec must configure exactly one backend variant"]
            );
        }
    }

    #[test]
    fn mcp_service_target_path_transport_and_auth_are_strict() {
        let mut value = mcp_service_spec();
        let mcp = value.mcp.as_mut().unwrap();
        assert_eq!(mcp.normalized_service_path().unwrap(), "/mcp");
        mcp.target.path = Some("/base/tools///".to_string());
        assert!(mcp.validate().is_err());

        for path in [
            "",
            "relative",
            "/base//tools",
            "/base/../tools",
            "/base/%2ftools",
            "/base?x=1",
            "/base#f",
            "/base\\tools",
            "/base\n",
        ] {
            mcp.target.path = Some(path.to_string());
            assert!(mcp.validate().is_err(), "invalid MCP path was accepted: {path:?}");
        }
        mcp.target.path = Some("/base/tools/".to_string());
        assert_eq!(mcp.normalized_service_path().unwrap(), "/base/tools");

        for yaml in [
            "target:\n  serviceRef:\n    name: mcp\n    port: 8080\n  path: /mcp\ntransport:\n  type: StreamableHTTP\nunknown: true\n",
            "target:\n  serviceRef:\n    name: mcp\n    port: 8080\n    unknown: true\n  path: /mcp\ntransport:\n  type: StreamableHTTP\n",
            "target:\n  serviceRef:\n    name: mcp\n    port: 8080\n  path: /mcp\ntransport:\n  type: StreamableHTTP\n  version: 2026-07-28\n",
            "target:\n  serviceRef:\n    name: mcp\n    port: 8080\n  path: /mcp\ntransport:\n  type: Other\n",
            "target:\n  serviceRef:\n    name: mcp\n    port: 8080\n  path: /mcp\ntransport:\n  type: StreamableHTTP\nauthentication: {}\n",
            "target:\n  serviceRef:\n    name: mcp\n    port: 8080\n  path: /mcp\ntransport:\n  type: StreamableHTTP\nauthentication:\n  bearerToken:\n    token: inline\n",
        ] {
            assert!(serde_yaml::from_str::<McpBackendSpec>(yaml).is_err(), "invalid MCP YAML was accepted: {yaml}");
        }
    }

    #[test]
    fn mcp_bearer_secret_ref_enforces_core_secret_identity() {
        let mut value = mcp_service_spec();
        let secret_ref = &mut value
            .mcp
            .as_mut()
            .unwrap()
            .authentication
            .as_mut()
            .unwrap()
            .bearer_token
            .secret_ref;
        secret_ref.name.clear();
        secret_ref.group = Some("external.example".to_string());
        secret_ref.kind = Some("ConfigMap".to_string());
        secret_ref.namespace = Some("Invalid_Namespace".to_string());
        let errors = value.validate().unwrap_err().join("\n");
        for field in ["group", "kind", "name", "namespace"] {
            assert!(errors.contains(field), "missing validation error for {field}: {errors}");
        }

        let secret_ref = &mut value
            .mcp
            .as_mut()
            .unwrap()
            .authentication
            .as_mut()
            .unwrap()
            .bearer_token
            .secret_ref;
        secret_ref.group = Some("core".to_string());
        secret_ref.kind = Some("Secret".to_string());
        secret_ref.name = "tools-token".to_string();
        secret_ref.namespace = Some("credential-ns".to_string());
        assert!(value.validate().is_ok());
    }

    #[test]
    fn mcp_target_union_and_external_endpoint_are_strict() {
        let external = |endpoint: &str, allow_insecure_http| McpBackendSpec {
            target: McpBackendTarget {
                service_ref: None,
                endpoint: Some(endpoint.to_string()),
                path: None,
            },
            allow_insecure_http,
            transport: McpTransport {
                r#type: McpTransportType::StreamableHTTP,
            },
            authentication: None,
        };
        assert_eq!(
            external("https://tools.example.com/base/mcp/", false)
                .normalized_endpoint()
                .unwrap(),
            "https://tools.example.com/base/mcp"
        );
        assert!(external("http://tools.example.com/mcp", true).validate().is_ok());
        assert!(external("http://tools.example.com/mcp", false).validate().is_err());
        assert!(external("https://tools.example.com/mcp", true).validate().is_err());
        for endpoint in [
            "tools.example.com/mcp",
            "ftp://tools.example.com/mcp",
            "https://user@tools.example.com/mcp",
            "https://tools.example.com:0/mcp",
            "https://tools.example.com/mcp?q=1",
            "https://tools.example.com/mcp#fragment",
            "https://tools.example.com/base//mcp",
            "https://tools.example.com/base/%2e%2e/mcp",
            "https://tools.example.com/base\\mcp",
        ] {
            assert!(
                external(endpoint, false).validate().is_err(),
                "invalid endpoint was accepted: {endpoint}"
            );
        }

        let mut both = mcp_service_spec().mcp.unwrap();
        both.target.endpoint = Some("https://tools.example.com/mcp".to_string());
        assert!(both.validate().unwrap_err().join("\n").contains("exactly one"));

        let mut external_with_path = external("https://tools.example.com/mcp", false);
        external_with_path.target.path = Some("/other".to_string());
        assert!(external_with_path
            .validate()
            .unwrap_err()
            .join("\n")
            .contains("must be absent"));
    }

    #[test]
    fn resolved_mcp_and_secret_are_transport_visible_but_schema_hidden_and_admin_redacted() {
        let mut value = mcp_service_spec();
        value.resolved_mcp = Some(ResolvedMcpBackend {
            target: ResolvedMcpTarget::Service {
                namespace: "tools".to_string(),
                name: "mcp-server".to_string(),
                port: 8080,
                path: "/mcp".to_string(),
            },
        });
        let secret = Secret {
            data: Some(BTreeMap::from([(
                "bearerToken".to_string(),
                ByteString(b"private-token".to_vec()),
            )])),
            ..Default::default()
        };
        value
            .mcp
            .as_mut()
            .unwrap()
            .authentication
            .as_mut()
            .unwrap()
            .bearer_token
            .secret = Some(secret);

        let transported = serde_json::to_value(&value).unwrap();
        assert_eq!(transported["resolvedMcp"]["target"]["type"], "service");
        assert!(transported["mcp"]["authentication"]["bearerToken"]["secret"]["data"]
            .get("bearerToken")
            .is_some());
        let redacted = {
            let _guard = AdminRedactGuard::new();
            serde_json::to_value(&value).unwrap()
        };
        assert_eq!(redacted["mcp"]["authentication"]["bearerToken"]["secret"], "[redacted]");
        assert!(!redacted.to_string().contains("private-token"));
        assert!(!format!("{value:?}").contains("private-token"));

        let schema = serde_json::to_value(schemars::schema_for!(EdgionBackendSpec)).unwrap();
        assert!(schema["properties"].get("resolvedMcp").is_none());
        assert!(!schema.to_string().contains("\"secret\""));
    }

    #[test]
    fn a2a_target_union_and_service_reference_are_strict() {
        let mut value = a2a_service_spec();
        let a2a = value.a2a.as_mut().unwrap();
        a2a.target.endpoint = Some("https://agent.example.com".to_string());
        assert!(a2a.validate().unwrap_err().join("\n").contains("exactly one"));

        a2a.target.endpoint = None;
        a2a.allow_insecure_http = true;
        assert!(a2a.validate().unwrap_err().join("\n").contains("must be false"));

        a2a.allow_insecure_http = false;
        let service_ref = a2a.target.service_ref.as_mut().unwrap();
        service_ref.name = "Invalid_Name".to_string();
        service_ref.namespace = Some("Invalid_Namespace".to_string());
        service_ref.port = 0;
        let errors = a2a.validate().unwrap_err().join("\n");
        assert!(errors.contains("valid Kubernetes name"));
        assert!(errors.contains("valid Kubernetes namespace"));
        assert!(errors.contains("1..=65535"));

        for yaml in [
            "target:\n  unknown: true\n",
            "target:\n  serviceRef:\n    name: agent\n    port: 8080\n    unknown: true\n",
        ] {
            assert!(serde_yaml::from_str::<A2aBackendSpec>(yaml).is_err());
        }
    }

    #[test]
    fn a2a_service_name_enforces_kubernetes_1_26_dns_1035_bounds() {
        let mut spec = a2a_service_spec();
        spec.a2a.as_mut().unwrap().target.service_ref.as_mut().unwrap().name = "a".repeat(63);
        assert!(spec.validate().is_ok());

        spec.a2a.as_mut().unwrap().target.service_ref.as_mut().unwrap().name = "a".repeat(64);
        assert!(spec.validate().is_err());

        spec.a2a.as_mut().unwrap().target.service_ref.as_mut().unwrap().name = "agent.service".to_string();
        assert!(spec.validate().is_err());

        spec.a2a.as_mut().unwrap().target.service_ref.as_mut().unwrap().name = "1-agent".to_string();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn a2a_external_endpoint_is_validated_and_normalized() {
        let external = |endpoint: &str, allow_insecure_http| A2aBackendSpec {
            target: A2aBackendTarget {
                service_ref: None,
                endpoint: Some(endpoint.to_string()),
            },
            allow_insecure_http,
        };

        assert!(external("https://agent.example.com/base///", false).validate().is_err());
        assert_eq!(
            external("https://agent.example.com/", false)
                .normalized_endpoint()
                .unwrap(),
            "https://agent.example.com"
        );
        for (case, value) in [
            external("agent.example.com", false),
            external("ftp://agent.example.com", false),
            external("https://user@agent.example.com", false),
            external("https://agent.example.com:0", false),
            external("https://agent.example.com/?q=1", false),
            external("https://agent.example.com/#fragment", false),
            external("http://agent.example.com", false),
            external("https://agent.example.com/base//task", false),
            external("https://agent.example.com/base/%2e%2e/task", false),
            external("https://agent.example.com/base/%2Ftask", false),
            external("https://agent.example.com/base\\task", false),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(value.validate().is_err(), "invalid endpoint case {case} was accepted");
        }
        assert!(external("http://agent.example.com", true).validate().is_ok());
        assert!(external("https://agent.example.com", true).validate().is_ok());
    }

    #[test]
    fn resolved_a2a_is_conf_sync_visible_but_schema_hidden() {
        let mut value = a2a_service_spec();
        value.resolved_a2a = Some(ResolvedA2aBackend {
            target: ResolvedA2aTarget::Service {
                namespace: "agents".to_string(),
                name: "agent-service".to_string(),
                port: 8080,
            },
        });
        value.reconcile_outcome = Some(EdgionBackendReconcileOutcome::A2a(A2aBackendReconcileOutcome {
            error: Some(A2aTargetResolutionClass::TargetNotFound),
        }));

        let serialized = serde_json::to_value(&value).unwrap();
        assert_eq!(serialized["resolvedA2a"]["target"]["type"], "service");
        assert!(serialized.get("reconcileOutcome").is_none());

        let schema = serde_json::to_value(schemars::schema_for!(EdgionBackendSpec)).unwrap();
        assert!(schema["properties"].get("resolvedA2a").is_none());
        assert!(schema["properties"].get("reconcileOutcome").is_none());
    }

    #[test]
    fn nested_wire_shape_is_strict_and_incompatible_with_legacy_flat_shape() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct LegacyFlatSpec {
            provider: AiProvider,
            #[serde(default)]
            endpoint: Option<String>,
            allow_insecure_http: bool,
            credential_pool: AiCredentialPool,
            models: Vec<AiModel>,
            defaults: AiBackendDefaults,
        }

        let nested = serde_json::to_value(spec(AiProvider::OpenAI)).unwrap();
        assert!(nested.get("ai").is_some());
        assert!(nested.get("provider").is_none());
        assert!(serde_json::from_value::<LegacyFlatSpec>(nested).is_err());

        let flat = serde_json::to_value(ai_spec(AiProvider::OpenAI)).unwrap();
        assert!(serde_json::from_value::<EdgionBackendSpec>(flat).is_err());

        let legacy: LegacyFlatSpec = serde_json::from_value(serde_json::to_value(ai_spec(AiProvider::OpenAI)).unwrap())
            .expect("frozen legacy DTO must decode its own shape");
        let _ = (
            legacy.provider,
            legacy.endpoint,
            legacy.allow_insecure_http,
            legacy.credential_pool,
            legacy.models,
            legacy.defaults,
        );
        assert_eq!(crate::ResourceKind::EdgionBackend as i32, 23);
        assert_eq!(
            crate::ResourceKind::try_from(23).ok(),
            Some(crate::ResourceKind::EdgionBackend)
        );
    }

    #[test]
    fn nested_unknown_fields_are_rejected() {
        for yaml in [
            "ai: {}\na2a: {}\n",
            "ai:\n  provider: OpenAI\n  credentialPool:\n    credentials:\n      - name: primary\n        secretRef:\n          name: creds\n          unknown: true\n  models:\n    - name: model-a\n",
            "ai:\n  provider: OpenAI\n  credentialPool:\n    credentials:\n      - name: primary\n        secretRef:\n          name: creds\n        unknown: true\n  models:\n    - name: model-a\n",
        ] {
            assert!(
                serde_yaml::from_str::<EdgionBackendSpec>(yaml).is_err(),
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
        let parsed: AiBackendSpec = serde_yaml::from_str(
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
        let parsed: AiBackendSpec = serde_yaml::from_str(
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
                serde_yaml::from_str::<AiBackendSpec>(&yaml).is_err(),
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
        value.credential_pool.credentials =
            vec![value.credential_pool.credentials[0].clone(); AI_CREDENTIAL_MAX_COUNT + 1];
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
        value.ai.as_mut().unwrap().credential_pool.credentials[0]
            .credential
            .secret = Some(Secret {
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
                .as_ref()
                .unwrap()
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
}
