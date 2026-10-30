//! OpenID Connect plugin configuration

use super::{lru_cache_config::LruCacheConfig, tiny_ufo_ttl_cache_config::TinyUfoTtlCacheConfig};

crate::redact::impl_redacted_debug!(OpenidConnectConfig);
use crate::resources::common::OutboundTlsLocal;
use crate::resources::edgion_plugins::auth_realm::validate_auth_realm;
use crate::resources::gateway::SecretObjectReference;
use jsonwebtoken::Algorithm;
use k8s_openapi::api::core::v1::Secret;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

/// Positive introspection-result cache settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct OidcIntrospectionCacheSection {
    #[serde(default)]
    pub tiny_ufo_ttl_cache: TinyUfoTtlCacheConfig,
}

/// Access-token cache settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct OidcAccessTokenCacheSection {
    #[serde(default)]
    pub lru_cache: LruCacheConfig,
}

/// Behavior when request is unauthenticated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum UnauthAction {
    /// Trigger authentication flow (redirect for browser-based flows).
    #[default]
    Auth,
    /// Deny with 401.
    Deny,
    /// Pass-through without authentication.
    Pass,
}

/// Endpoint authentication method.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndpointAuthMethod {
    #[default]
    ClientSecretBasic,
    ClientSecretPost,
}

/// Token verification mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum VerificationMode {
    /// Prefer JWT local verification, optionally fall back based on implementation phase.
    #[default]
    Auto,
    /// Only use JWKS local verification.
    JwksOnly,
    /// Only use RFC7662 introspection.
    IntrospectionOnly,
}

/// OpenID Connect plugin configuration.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenidConnectConfig {
    // Required
    pub discovery: String,
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret_ref: Option<SecretObjectReference>,

    // Auth behavior
    #[serde(default)]
    pub bearer_only: bool,
    #[serde(default)]
    pub use_pkce: bool,
    #[serde(default)]
    pub unauth_action: UnauthAction,

    // OIDC params
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_scopes: Option<Vec<String>>,
    #[serde(default = "default_realm")]
    pub realm: String,
    #[serde(default)]
    pub use_nonce: bool,
    #[serde(default)]
    pub revoke_tokens_on_logout: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_params: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_logout_redirect_uri: Option<String>,
    #[serde(default = "default_logout_path")]
    pub logout_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_secret_ref: Option<SecretObjectReference>,
    #[serde(default = "default_session_cookie_name")]
    pub session_cookie_name: String,
    #[serde(default = "default_session_lifetime_seconds")]
    pub session_lifetime: u64,
    #[serde(default = "default_session_cookie_same_site")]
    pub session_cookie_same_site: String,
    #[serde(default = "default_true")]
    pub session_cookie_http_only: bool,
    #[serde(default = "default_true")]
    pub session_cookie_secure: bool,
    #[serde(default = "default_max_session_cookie_bytes")]
    pub max_session_cookie_bytes: u64,
    #[serde(default = "default_true")]
    pub renew_access_token_on_expiry: bool,
    #[serde(default)]
    pub access_token_expires_leeway: u64,

    // Token verification
    #[serde(default)]
    pub verification_mode: VerificationMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_signing_alg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_signing_algs: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub use_jwks: bool,
    /// Outbound TLS settings (verify, validation, clientCertificateRef) for IdP connections
    /// (discovery, JWKS, introspection). All fields are optional; absent fields inherit from
    /// the global outboundTls default.
    #[serde(default)]
    pub tls: OutboundTlsLocal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audiences: Option<Vec<String>>,
    #[serde(default = "default_clock_skew_seconds")]
    pub clock_skew_seconds: u64,
    #[serde(default = "default_jwks_cache_ttl")]
    pub jwks_cache_ttl: u64,
    #[serde(default = "default_jwks_min_refresh_interval")]
    pub jwks_min_refresh_interval: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub introspection_endpoint: Option<String>,
    #[serde(default)]
    pub introspection_endpoint_auth_method: EndpointAuthMethod,
    #[serde(default)]
    pub introspection_cache_ttl: u64,
    /// Introspection result cache settings.
    #[serde(default)]
    pub introspection: OidcIntrospectionCacheSection,

    /// Access token cache settings.
    #[serde(default)]
    pub access_token: OidcAccessTokenCacheSection,

    #[serde(default)]
    pub token_endpoint_auth_method: EndpointAuthMethod,

    // Header forwarding
    #[serde(default)]
    pub set_access_token_header: bool,
    #[serde(default)]
    pub set_id_token_header: bool,
    #[serde(default)]
    pub set_userinfo_header: bool,
    #[serde(default)]
    pub access_token_in_authorization_header: bool,
    /// Map claims to case-insensitively unique upstream request-header destinations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims_to_headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub store_claims_in_ctx: bool,
    #[serde(default = "default_max_header_value_bytes")]
    pub max_header_value_bytes: u64,
    #[serde(default = "default_max_total_header_bytes")]
    pub max_total_header_bytes: u64,

    // Network
    #[serde(default = "default_timeout_seconds")]
    pub timeout: u64,

    // DoS protection: response body limits for external IdP fetches.
    // A chunked or dishonest IdP exceeding these limits is cut off mid-stream,
    // so peak memory per fetch is bounded by the configured value.
    /// Max response body size accepted for OIDC discovery. Default: 65 536 (64 KB).
    #[serde(default = "default_discovery_max_response_bytes")]
    pub discovery_max_response_bytes: usize,
    /// Max response body size accepted for JWKS. Default: 1 048 576 (1 MB).
    #[serde(default = "default_jwks_max_response_bytes")]
    pub jwks_max_response_bytes: usize,
    /// Max response body size accepted for the userinfo endpoint. Default: 131 072 (128 KB).
    #[serde(default = "default_userinfo_max_response_bytes")]
    pub userinfo_max_response_bytes: usize,

    // Runtime fields (controller-populated)
    /// Resolved OIDC OAuth2 client secret value (from `clientSecretRef`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_oidc_client_secret: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_session_secret: Option<String>,
    /// Resolved CA Secrets (from `tls.validation.caCertificateRefs`, filled by Controller).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_ca_certificates: Option<Vec<Secret>>,
    /// Resolved mTLS client-identity Secret (from `tls.clientCertificateRef`, filled by Controller).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_client_certificate: Option<Secret>,

    // Security extras
    /// Remove the original Authorization header from the upstream request after
    /// successful bearer-token auth.
    ///
    /// Gateway-owned session and state cookies are always stripped before
    /// proxying and are not controlled by this flag.
    /// Note: if `access_token_in_authorization_header` is also true, the original
    /// header is removed first, then a new Authorization: Bearer <token> header
    /// is set for upstream.
    /// Default: false.
    #[serde(default)]
    pub hide_credentials: bool,

    /// Delay in milliseconds before returning an authentication failure response.
    /// Increases the time cost for brute-force / credential-stuffing attacks.
    /// Default: 0 (no delay).
    #[serde(default)]
    pub auth_failure_delay_ms: u64,
}

fn default_true() -> bool {
    true
}
fn default_scope() -> String {
    "openid".to_string()
}
fn default_realm() -> String {
    "edgion".to_string()
}
fn default_clock_skew_seconds() -> u64 {
    120
}
fn default_jwks_cache_ttl() -> u64 {
    300
}
fn default_jwks_min_refresh_interval() -> u64 {
    10
}
fn default_discovery_max_response_bytes() -> usize {
    65_536
}
fn default_jwks_max_response_bytes() -> usize {
    1_048_576
}
fn default_userinfo_max_response_bytes() -> usize {
    131_072
}
fn default_max_header_value_bytes() -> u64 {
    4096
}
fn default_max_total_header_bytes() -> u64 {
    16384
}
fn default_timeout_seconds() -> u64 {
    3
}
fn default_session_cookie_name() -> String {
    "edgion_oidc_session".to_string()
}
fn default_session_lifetime_seconds() -> u64 {
    3600
}
fn default_session_cookie_same_site() -> String {
    "Lax".to_string()
}
fn default_max_session_cookie_bytes() -> u64 {
    3800
}
fn default_logout_path() -> String {
    "/logout".to_string()
}

fn is_reserved_authorization_param(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "response_type"
            | "client_id"
            | "redirect_uri"
            | "scope"
            | "state"
            | "nonce"
            | "code_challenge"
            | "code_challenge_method"
    )
}

impl Default for OpenidConnectConfig {
    fn default() -> Self {
        Self {
            discovery: String::new(),
            client_id: String::new(),
            client_secret_ref: None,
            bearer_only: false,
            use_pkce: false,
            unauth_action: UnauthAction::default(),
            scope: default_scope(),
            required_scopes: None,
            realm: default_realm(),
            use_nonce: false,
            revoke_tokens_on_logout: false,
            redirect_uri: None,
            authorization_params: None,
            post_logout_redirect_uri: None,
            logout_path: default_logout_path(),
            session_secret_ref: None,
            session_cookie_name: default_session_cookie_name(),
            session_lifetime: default_session_lifetime_seconds(),
            session_cookie_same_site: default_session_cookie_same_site(),
            session_cookie_http_only: default_true(),
            session_cookie_secure: default_true(),
            max_session_cookie_bytes: default_max_session_cookie_bytes(),
            renew_access_token_on_expiry: default_true(),
            access_token_expires_leeway: 0,
            verification_mode: VerificationMode::default(),
            token_signing_alg: None,
            allowed_signing_algs: None,
            use_jwks: default_true(),
            tls: OutboundTlsLocal::default(),
            issuers: None,
            audiences: None,
            clock_skew_seconds: default_clock_skew_seconds(),
            jwks_cache_ttl: default_jwks_cache_ttl(),
            jwks_min_refresh_interval: default_jwks_min_refresh_interval(),
            introspection_endpoint: None,
            introspection_endpoint_auth_method: EndpointAuthMethod::default(),
            introspection_cache_ttl: 0,
            introspection: OidcIntrospectionCacheSection::default(),
            access_token: OidcAccessTokenCacheSection::default(),
            token_endpoint_auth_method: EndpointAuthMethod::default(),
            set_access_token_header: false,
            set_id_token_header: false,
            set_userinfo_header: false,
            access_token_in_authorization_header: false,
            claims_to_headers: None,
            store_claims_in_ctx: false,
            max_header_value_bytes: default_max_header_value_bytes(),
            max_total_header_bytes: default_max_total_header_bytes(),
            timeout: default_timeout_seconds(),
            resolved_oidc_client_secret: None,
            resolved_session_secret: None,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            hide_credentials: false,
            auth_failure_delay_ms: 0,
            discovery_max_response_bytes: default_discovery_max_response_bytes(),
            jwks_max_response_bytes: default_jwks_max_response_bytes(),
            userinfo_max_response_bytes: default_userinfo_max_response_bytes(),
        }
    }
}

impl OpenidConnectConfig {
    pub fn compile_claim_header_projection(&self) -> Result<super::ClaimHeaderProjectionPlan, &'static str> {
        let mut owned_destinations = Vec::with_capacity(4);
        if self.set_access_token_header {
            owned_destinations.push("X-Access-Token");
        }
        if self.set_id_token_header {
            owned_destinations.push("X-ID-Token");
        }
        if self.set_userinfo_header {
            owned_destinations.push("X-Userinfo");
        }
        if self.access_token_in_authorization_header {
            owned_destinations.push("Authorization");
        }
        super::auth_header_projection::compile_authenticated_claim_header_projection(
            self.claims_to_headers.as_ref(),
            &owned_destinations,
        )
    }

    /// Validate configuration and return first error.
    pub fn get_validation_error(&self) -> Option<&str> {
        if let Err(error) = validate_auth_realm(&self.realm) {
            return Some(error);
        }
        if self.discovery.is_empty() {
            return Some("discovery is required");
        }
        if !self.discovery.starts_with("https://") && !self.discovery.starts_with("http://") {
            return Some("discovery must start with http:// or https://");
        }
        if self.client_id.is_empty() {
            return Some("clientId is required");
        }
        if self.realm.is_empty() {
            return Some("realm cannot be empty");
        }
        if self.scope.trim().is_empty() {
            return Some("scope cannot be empty");
        }
        if self
            .redirect_uri
            .as_ref()
            .is_some_and(|u| !(u.starts_with('/') || u.starts_with("https://") || u.starts_with("http://")))
        {
            return Some("redirectUri must be absolute URL or absolute path");
        }
        if self
            .post_logout_redirect_uri
            .as_ref()
            .is_some_and(|u| !(u.starts_with('/') || u.starts_with("https://") || u.starts_with("http://")))
        {
            return Some("postLogoutRedirectUri must be absolute URL or absolute path");
        }
        if let Some(params) = self.authorization_params.as_ref() {
            if params.keys().any(|k| is_reserved_authorization_param(k)) {
                return Some("authorizationParams cannot contain reserved OIDC keys");
            }
        }
        if !self.logout_path.starts_with('/') {
            return Some("logoutPath must start with '/'");
        }
        if self.timeout == 0 {
            return Some("timeout must be greater than 0");
        }
        if let Some(error) = self.introspection.tiny_ufo_ttl_cache.validation_error() {
            return Some(error);
        }
        if let Some(error) = self.access_token.lru_cache.validation_error() {
            return Some(error);
        }
        if self.session_cookie_name.trim().is_empty() {
            return Some("sessionCookieName cannot be empty");
        }
        if self.session_lifetime == 0 {
            return Some("sessionLifetime must be greater than 0");
        }
        let same_site = self.session_cookie_same_site.trim();
        if same_site.is_empty() {
            return Some("sessionCookieSameSite cannot be empty");
        }
        if !same_site.eq_ignore_ascii_case("Strict")
            && !same_site.eq_ignore_ascii_case("Lax")
            && !same_site.eq_ignore_ascii_case("None")
        {
            return Some("sessionCookieSameSite must be one of: Strict, Lax, None");
        }
        if same_site.eq_ignore_ascii_case("None") && !self.session_cookie_secure {
            return Some("sessionCookieSecure must be true when sessionCookieSameSite is None");
        }
        if self.max_session_cookie_bytes == 0 {
            return Some("maxSessionCookieBytes must be greater than 0");
        }
        if self.jwks_cache_ttl == 0 {
            return Some("jwksCacheTtl must be greater than 0");
        }
        if self.jwks_min_refresh_interval == 0 {
            return Some("jwksMinRefreshInterval must be greater than 0");
        }
        if self.discovery_max_response_bytes == 0 {
            return Some("discoveryMaxResponseBytes must be greater than 0");
        }
        if self.jwks_max_response_bytes == 0 {
            return Some("jwksMaxResponseBytes must be greater than 0");
        }
        if self.userinfo_max_response_bytes == 0 {
            return Some("userinfoMaxResponseBytes must be greater than 0");
        }
        if self.max_header_value_bytes == 0 {
            return Some("maxHeaderValueBytes must be greater than 0");
        }
        if self.max_total_header_bytes == 0 {
            return Some("maxTotalHeaderBytes must be greater than 0");
        }
        if self.max_total_header_bytes < self.max_header_value_bytes {
            return Some("maxTotalHeaderBytes must be >= maxHeaderValueBytes");
        }
        if let Err(error) = self.compile_claim_header_projection() {
            return Some(error);
        }
        if self.verification_mode == VerificationMode::JwksOnly && !self.use_jwks {
            return Some("useJwks must be true when verificationMode is JwksOnly");
        }

        if self
            .token_signing_alg
            .as_deref()
            .is_some_and(|alg| alg.eq_ignore_ascii_case("none"))
        {
            return Some("tokenSigningAlg cannot be 'none'");
        }
        if self
            .allowed_signing_algs
            .as_ref()
            .is_some_and(|algs| algs.iter().any(|a| a.eq_ignore_ascii_case("none")))
        {
            return Some("allowedSigningAlgs cannot contain 'none'");
        }

        if self
            .token_signing_alg
            .as_ref()
            .is_some_and(|alg| Algorithm::from_str(alg).is_err())
        {
            return Some("tokenSigningAlg is invalid");
        }

        if self.allowed_signing_algs.as_ref().is_some_and(|algs| algs.is_empty()) {
            return Some("allowedSigningAlgs cannot be empty");
        }
        if self
            .allowed_signing_algs
            .as_ref()
            .is_some_and(|algs| algs.iter().any(|alg| Algorithm::from_str(alg).is_err()))
        {
            return Some("allowedSigningAlgs contains invalid algorithm");
        }

        if self.token_signing_alg.is_some()
            && self
                .allowed_signing_algs
                .as_ref()
                .is_some_and(|algs| !algs.iter().any(|a| Some(a) == self.token_signing_alg.as_ref()))
        {
            return Some("tokenSigningAlg must be included in allowedSigningAlgs");
        }

        // clientSecret is optional only for bearerOnly + JWKS mode.
        let needs_client_secret = !self.bearer_only
            || self.verification_mode == VerificationMode::IntrospectionOnly
            || (!self.use_jwks && self.verification_mode == VerificationMode::Auto)
            || self.revoke_tokens_on_logout;
        if needs_client_secret && self.client_secret_ref.is_none() && self.resolved_oidc_client_secret.is_none() {
            return Some("clientSecretRef is required for non-bearer-only/introspection flows");
        }
        let needs_session_secret = !self.bearer_only && self.unauth_action == UnauthAction::Auth;
        if needs_session_secret && self.session_secret_ref.is_none() && self.resolved_session_secret.is_none() {
            return Some("sessionSecretRef is required when unauthAction=Auth");
        }

        None
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for OpenidConnectConfig {
    fn validation_error(&self) -> Option<String> {
        self.get_validation_error().map(str::to_owned)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/openid_connect_test.rs"]
mod tests;
