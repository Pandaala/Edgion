use std::collections::HashSet;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::duration::parse_duration;
use crate::resources::common::{is_core_api_group, SecretObjectReference, TlsConfig};

pub const OAUTH2_SCOPE_MAX_COUNT: usize = 16;
pub const OAUTH2_SCOPE_MAX_LEN: usize = 256;
pub const OAUTH2_SCOPES_JOINED_MAX_LEN: usize = 2 * 1024;
pub const OAUTH2_TOKEN_ENDPOINT_MAX_LEN: usize = 2 * 1024;

const ROTATION_INTERVAL_MIN: Duration = Duration::from_secs(30);
const ROTATION_INTERVAL_MAX: Duration = Duration::from_secs(24 * 60 * 60);
const REQUEST_TIMEOUT_MIN: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT_MAX: Duration = Duration::from_secs(60);
const RETRY_INITIAL_BACKOFF_MIN: Duration = Duration::from_secs(1);
const RETRY_INITIAL_BACKOFF_MAX: Duration = Duration::from_secs(60);
const RETRY_MAX_BACKOFF_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX_BACKOFF_MAX: Duration = Duration::from_secs(15 * 60);

fn default_rotation_interval() -> String {
    "15m".to_string()
}

fn default_request_timeout() -> String {
    "10s".to_string()
}

fn default_retry_initial_backoff() -> String {
    "2s".to_string()
}

fn default_retry_max_backoff() -> String {
    "2m".to_string()
}

fn default_block_private() -> bool {
    true
}

/// Controller-managed credential source. The Gateway treats this LinkSys
/// variant as metadata-only and never contacts the credential issuer.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialSourceConfig {
    pub provider: CredentialSourceProvider,

    #[serde(default)]
    pub rotation: CredentialRotationConfig,

    #[serde(default)]
    pub egress: CredentialSourceEgressConfig,
}

impl CredentialSourceConfig {
    pub fn validation_errors(&self) -> Vec<String> {
        let mut errors = self.provider.validation_errors();
        errors.extend(self.rotation.validation_errors());
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum CredentialSourceProvider {
    #[serde(rename = "oauth2ClientCredentials")]
    OAuth2ClientCredentials(OAuth2ClientCredentialsConfig),
}

impl CredentialSourceProvider {
    fn validation_errors(&self) -> Vec<String> {
        match self {
            Self::OAuth2ClientCredentials(config) => config.validation_errors(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OAuth2ClientCredentialsConfig {
    #[schemars(length(min = 1, max = OAUTH2_TOKEN_ENDPOINT_MAX_LEN))]
    pub token_endpoint: String,

    pub client_authentication: OAuth2ClientAuthentication,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = OAUTH2_SCOPE_MAX_COUNT))]
    pub scopes: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,
}

impl OAuth2ClientCredentialsConfig {
    fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if let Err(error) = validate_token_endpoint(&self.token_endpoint) {
            errors.push(error);
        }
        errors.extend(self.client_authentication.validation_errors());
        errors.extend(validate_scopes(&self.scopes));
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OAuth2ClientAuthentication {
    #[serde(default)]
    pub method: OAuth2ClientAuthenticationMethod,

    pub active_secret_ref: SecretObjectReference,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_secret_ref: Option<SecretObjectReference>,
}

impl OAuth2ClientAuthentication {
    fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        validate_bootstrap_secret_ref("activeSecretRef", &self.active_secret_ref, &mut errors);
        if let Some(secret_ref) = &self.previous_secret_ref {
            validate_bootstrap_secret_ref("previousSecretRef", secret_ref, &mut errors);
            if secret_ref == &self.active_secret_ref {
                errors.push(
                    "credentialSource.provider.clientAuthentication previousSecretRef must differ from activeSecretRef"
                        .to_string(),
                );
            }
        }
        errors
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub enum OAuth2ClientAuthenticationMethod {
    #[default]
    #[serde(rename = "clientSecretBasic")]
    ClientSecretBasic,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialRotationConfig {
    #[serde(default = "default_rotation_interval")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub interval: String,

    #[serde(default = "default_request_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub request_timeout: String,

    #[serde(default = "default_retry_initial_backoff")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub retry_initial_backoff: String,

    #[serde(default = "default_retry_max_backoff")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub retry_max_backoff: String,
}

impl CredentialRotationConfig {
    fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let interval = validate_duration(
            "rotation.interval",
            &self.interval,
            ROTATION_INTERVAL_MIN,
            ROTATION_INTERVAL_MAX,
            &mut errors,
        );
        let request_timeout = validate_duration(
            "rotation.requestTimeout",
            &self.request_timeout,
            REQUEST_TIMEOUT_MIN,
            REQUEST_TIMEOUT_MAX,
            &mut errors,
        );
        let initial = validate_duration(
            "rotation.retryInitialBackoff",
            &self.retry_initial_backoff,
            RETRY_INITIAL_BACKOFF_MIN,
            RETRY_INITIAL_BACKOFF_MAX,
            &mut errors,
        );
        let maximum = validate_duration(
            "rotation.retryMaxBackoff",
            &self.retry_max_backoff,
            RETRY_MAX_BACKOFF_MIN,
            RETRY_MAX_BACKOFF_MAX,
            &mut errors,
        );
        if let (Some(initial), Some(maximum)) = (initial, maximum) {
            if initial > maximum {
                errors
                    .push("credentialSource.rotation.retryInitialBackoff must not exceed retryMaxBackoff".to_string());
            }
        }
        let _ = (interval, request_timeout);
        errors
    }
}

impl Default for CredentialRotationConfig {
    fn default() -> Self {
        Self {
            interval: default_rotation_interval(),
            request_timeout: default_request_timeout(),
            retry_initial_backoff: default_retry_initial_backoff(),
            retry_max_backoff: default_retry_max_backoff(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialSourceEgressConfig {
    #[serde(default = "default_block_private")]
    pub block_private: bool,
}

impl Default for CredentialSourceEgressConfig {
    fn default() -> Self {
        Self {
            block_private: default_block_private(),
        }
    }
}

fn validate_duration(
    field: &str,
    value: &str,
    minimum: Duration,
    maximum: Duration,
    errors: &mut Vec<String>,
) -> Option<Duration> {
    match parse_duration(value) {
        Ok(duration) if duration >= minimum && duration <= maximum => Some(duration),
        Ok(_) => {
            errors.push(format!(
                "credentialSource.{field} must be between {}s and {}s",
                minimum.as_secs(),
                maximum.as_secs()
            ));
            None
        }
        Err(error) => {
            errors.push(format!("credentialSource.{field} is invalid: {error}"));
            None
        }
    }
}

fn validate_token_endpoint(raw: &str) -> Result<(), String> {
    if raw.len() > OAUTH2_TOKEN_ENDPOINT_MAX_LEN {
        return Err(format!(
            "credentialSource.provider.tokenEndpoint must be at most {OAUTH2_TOKEN_ENDPOINT_MAX_LEN} bytes"
        ));
    }
    let parsed = url::Url::parse(raw)
        .map_err(|_| "credentialSource.provider.tokenEndpoint must be a valid HTTPS URL".to_string())?;
    if parsed.scheme() != "https" {
        return Err("credentialSource.provider.tokenEndpoint must use https".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("credentialSource.provider.tokenEndpoint must include a host".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("credentialSource.provider.tokenEndpoint must not contain userinfo".to_string());
    }
    if parsed.query().is_some() {
        return Err("credentialSource.provider.tokenEndpoint must not contain a query string".to_string());
    }
    if parsed.fragment().is_some() {
        return Err("credentialSource.provider.tokenEndpoint must not contain a fragment".to_string());
    }
    Ok(())
}

fn validate_bootstrap_secret_ref(field: &str, secret_ref: &SecretObjectReference, errors: &mut Vec<String>) {
    if secret_ref.name.is_empty() {
        errors.push(format!(
            "credentialSource.provider.clientAuthentication.{field}.name must not be empty"
        ));
    }
    if secret_ref
        .group
        .as_deref()
        .is_some_and(|group| !is_core_api_group(group))
    {
        errors.push(format!(
            "credentialSource.provider.clientAuthentication.{field}.group must identify the Kubernetes core API group"
        ));
    }
    if secret_ref.kind.as_deref().is_some_and(|kind| kind != "Secret") {
        errors.push(format!(
            "credentialSource.provider.clientAuthentication.{field}.kind must be Secret"
        ));
    }
}

fn validate_scopes(scopes: &[String]) -> Vec<String> {
    let mut errors = Vec::new();
    if scopes.len() > OAUTH2_SCOPE_MAX_COUNT {
        errors.push(format!(
            "credentialSource.provider.scopes must contain at most {OAUTH2_SCOPE_MAX_COUNT} entries"
        ));
    }
    let mut seen = HashSet::new();
    let mut aggregate_len = scopes.len().saturating_sub(1);
    for (index, scope) in scopes.iter().enumerate() {
        aggregate_len = aggregate_len.saturating_add(scope.len());
        if scope.is_empty() || scope.len() > OAUTH2_SCOPE_MAX_LEN || !scope.bytes().all(is_scope_token_byte) {
            errors.push(format!(
                "credentialSource.provider.scopes[{index}] must be a 1..={OAUTH2_SCOPE_MAX_LEN}-byte RFC 6749 scope-token"
            ));
        }
        if !seen.insert(scope) {
            errors.push(format!(
                "credentialSource.provider.scopes[{index}] duplicates an earlier scope"
            ));
        }
    }
    if aggregate_len > OAUTH2_SCOPES_JOINED_MAX_LEN {
        errors.push(format!(
            "credentialSource.provider.scopes joined value must be at most {OAUTH2_SCOPES_JOINED_MAX_LEN} bytes"
        ));
    }
    errors
}

fn is_scope_token_byte(byte: u8) -> bool {
    byte == 0x21 || (0x23..=0x5b).contains(&byte) || (0x5d..=0x7e).contains(&byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_yaml() -> &'static str {
        r#"
provider:
  type: oauth2ClientCredentials
  tokenEndpoint: https://auth.example.com/oauth2/token
  clientAuthentication:
    activeSecretRef:
      name: oauth-client
  scopes: [inference.invoke]
"#
    }

    #[test]
    fn minimal_config_parses_with_secure_defaults() {
        let config: CredentialSourceConfig = serde_yaml::from_str(valid_yaml()).expect("valid config must parse");
        assert_eq!(config.rotation, CredentialRotationConfig::default());
        assert!(config.egress.block_private);
        let CredentialSourceProvider::OAuth2ClientCredentials(provider) = &config.provider;
        assert_eq!(
            provider.client_authentication.method,
            OAuth2ClientAuthenticationMethod::ClientSecretBasic
        );
        assert!(config.validation_errors().is_empty());
    }

    #[test]
    fn rejects_unsafe_endpoint_invalid_scopes_refs_and_rotation() {
        let yaml = r#"
provider:
  type: oauth2ClientCredentials
  tokenEndpoint: http://user:pass@auth.example.com/token?tenant=x#fragment
  clientAuthentication:
    activeSecretRef: {group: edgion.io, kind: LinkSys, name: source}
    previousSecretRef: {group: edgion.io, kind: LinkSys, name: source}
  scopes: [valid, "bad scope", valid]
rotation:
  interval: 1s
  requestTimeout: 61s
  retryInitialBackoff: 1m
  retryMaxBackoff: 1s
"#;
        let config: CredentialSourceConfig = serde_yaml::from_str(yaml).expect("structurally valid config must parse");
        let errors = config.validation_errors().join("; ");
        for expected in [
            "must use https",
            "group",
            "duplicates",
            "rotation.interval",
            "must not exceed",
        ] {
            assert!(errors.contains(expected), "missing {expected:?} in {errors}");
        }
    }

    #[test]
    fn provider_and_system_discriminators_round_trip_exactly() {
        let config: CredentialSourceConfig = serde_yaml::from_str(valid_yaml()).expect("valid config must parse");
        let system = super::super::SystemConfig::CredentialSource(config);
        let value = serde_json::to_value(system).expect("system config must serialize");
        assert_eq!(value["type"], "credentialSource");
        assert_eq!(value["config"]["provider"]["type"], "oauth2ClientCredentials");
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = format!("{}unknownField: ignored\n", valid_yaml());
        assert!(serde_yaml::from_str::<CredentialSourceConfig>(&yaml).is_err());
    }
}
