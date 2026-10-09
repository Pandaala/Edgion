//! Bounded OAuth 2.0 Client Credentials provider.
//!
//! This module owns only issuer I/O and typed response classification. The
//! credential manager owns bootstrap selection, scheduling, publication, and
//! status projection.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine;
use edgion_common::dns::ResolverSpec;
use edgion_common::egress::{EgressPolicy, EgressResolver};
use edgion_common::link_sys_core::http_client::{build_outbound_client_with_tls_selection, OutboundClientConfig};
use edgion_common::link_sys_core::outbound_tls::PreparedLinkSysTls;
use edgion_resources::credentials::{BearerResolver, SecretMaterialResolver};
use edgion_resources::resources::edgion_plugins::{
    CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN, CREDENTIAL_INJECTOR_VALUE_PREFIX_MAX_LEN,
};
use edgion_resources::resources::link_sys::credential_source::{
    OAuth2ClientAuthenticationMethod, OAUTH2_TOKEN_ENDPOINT_MAX_LEN,
};
use edgion_resources::resources::link_sys::{CredentialSourceProvider, SystemConfig};
use futures::StreamExt;
use k8s_openapi::ByteString;
use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use thiserror::Error;
use zeroize::Zeroizing;

pub const CLIENT_ID_KEY: &str = "clientId";
pub const CLIENT_SECRET_KEY: &str = "clientSecret";
pub const CLIENT_ID_MAX_LEN: usize = 512;
pub const CLIENT_SECRET_MAX_LEN: usize = 4 * 1024;
pub const AUTHORIZATION_VALUE_MAX_LEN: usize = 8 * 1024;
pub const REQUEST_FORM_MAX_LEN: usize = 4 * 1024;
pub const RESPONSE_BODY_MAX_LEN: usize = 16 * 1024;
pub const ACCESS_TOKEN_MAX_LEN: usize =
    CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN - CREDENTIAL_INJECTOR_VALUE_PREFIX_MAX_LEN;
pub const EXPIRES_IN_MIN_SECONDS: u64 = 40;
pub const EXPIRES_IN_MAX_SECONDS: u64 = 7 * 24 * 60 * 60;

const EGRESS_CACHE_TTL: Duration = Duration::from_secs(5);

/// Parsed fixed-key bootstrap Secret material.
///
/// The source Kubernetes Secret remains an unzeroized cache owned by the
/// controller. `Zeroizing` here is therefore best-effort defense-in-depth for
/// the provider's final `clientSecret` copy, not an end-to-end guarantee.
pub struct BootstrapCredentials {
    client_id: String,
    client_secret: Zeroizing<String>,
}

impl fmt::Debug for BootstrapCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BootstrapCredentials")
            .field("client_id", &"[redacted]")
            .field("client_secret", &"[redacted]")
            .finish()
    }
}

impl BootstrapCredentials {
    pub fn parse(data: &BTreeMap<String, ByteString>) -> Result<Self, BootstrapCredentialError> {
        let client_id = parse_bootstrap_value(data, CLIENT_ID_KEY, CLIENT_ID_MAX_LEN)?;
        let client_secret = Zeroizing::new(parse_bootstrap_value(data, CLIENT_SECRET_KEY, CLIENT_SECRET_MAX_LEN)?);
        Ok(Self {
            client_id,
            client_secret,
        })
    }
}

fn parse_bootstrap_value(
    data: &BTreeMap<String, ByteString>,
    key: &'static str,
    max_len: usize,
) -> Result<String, BootstrapCredentialError> {
    let raw = data.get(key).ok_or(BootstrapCredentialError::MissingKey(key))?;
    if raw.0.is_empty() {
        return Err(BootstrapCredentialError::Empty(key));
    }
    if raw.0.len() > max_len {
        return Err(BootstrapCredentialError::TooLong { key, max_len });
    }
    let value = std::str::from_utf8(&raw.0).map_err(|_| BootstrapCredentialError::InvalidUtf8(key))?;
    Ok(value.to_owned())
}

/// Safe, fixed-vocabulary bootstrap validation failure.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum BootstrapCredentialError {
    #[error("bootstrap Secret is missing required key `{0}`")]
    MissingKey(&'static str),
    #[error("bootstrap Secret key `{0}` must not be empty")]
    Empty(&'static str),
    #[error("bootstrap Secret key `{0}` must contain UTF-8")]
    InvalidUtf8(&'static str),
    #[error("bootstrap Secret key `{key}` exceeds the {max_len}-byte limit")]
    TooLong { key: &'static str, max_len: usize },
}

/// One validated issuer result. Debug output deliberately omits the token.
pub struct OAuth2Token {
    pub access_token: String,
    pub expires_in: Duration,
}

impl fmt::Debug for OAuth2Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuth2Token")
            .field("access_token", &"[redacted]")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuth2FailureKind {
    /// The active bootstrap was explicitly rejected as `invalid_client` and a
    /// configured previous bootstrap may be tried once.
    InvalidClient,
    /// A transport, throttling, or server failure eligible for backoff retry.
    Transient,
    /// A bounded provider rejection or malformed success response.
    Permanent,
}

/// Error classification consumed by the credential manager scheduler.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("OAuth2 credential acquisition failed ({kind:?})")]
pub struct OAuth2ProviderError {
    pub kind: OAuth2FailureKind,
    pub retry_after: Option<Duration>,
}

impl OAuth2ProviderError {
    fn transient(retry_after: Option<Duration>) -> Self {
        Self {
            kind: OAuth2FailureKind::Transient,
            retry_after,
        }
    }

    fn permanent() -> Self {
        Self {
            kind: OAuth2FailureKind::Permanent,
            retry_after: None,
        }
    }
}

/// Reusable issuer client for one immutable LinkSys source generation.
pub struct OAuth2Provider {
    client: Client,
    token_endpoint: Url,
    authorization_form: String,
    retry_after_cap: Duration,
}

impl OAuth2Provider {
    pub fn new(
        prepared: &PreparedLinkSysTls,
        request_timeout: Duration,
        retry_after_cap: Duration,
        resolver: ResolverSpec,
    ) -> Result<Self, OAuth2ProviderError> {
        let SystemConfig::CredentialSource(source) = &prepared.owner().spec.config else {
            return Err(OAuth2ProviderError::permanent());
        };
        let CredentialSourceProvider::OAuth2ClientCredentials(config) = &source.provider;
        match config.client_authentication.method {
            OAuth2ClientAuthenticationMethod::ClientSecretBasic => {}
        }
        let token_endpoint = validate_endpoint(&config.token_endpoint)?;
        let authorization_form = build_request_form(&config.scopes)?;
        let egress_resolver = EgressResolver::new(
            resolver,
            EgressPolicy {
                block_private: source.egress.block_private,
            },
            EGRESS_CACHE_TTL,
        );
        let client = prepared
            .with_tls_selection(|proof, defaults| {
                build_outbound_client_with_tls_selection(
                    OutboundClientConfig {
                        connect_timeout: Some(request_timeout),
                        total_timeout: Some(request_timeout),
                        disable_proxy: true,
                        tls_context: Some("OAuth2CredentialProvider"),
                        dns_resolver: Some(Arc::new(egress_resolver)),
                        ..Default::default()
                    },
                    proof,
                    defaults,
                )
            })
            .map_err(|_| OAuth2ProviderError::permanent())?;

        Ok(Self {
            client,
            token_endpoint,
            authorization_form,
            retry_after_cap,
        })
    }

    pub async fn acquire(&self, bootstrap: &BootstrapCredentials) -> Result<OAuth2Token, OAuth2ProviderError> {
        let authorization = build_basic_authorization(bootstrap)?;
        let response = self
            .client
            .post(self.token_endpoint.clone())
            .header(AUTHORIZATION, authorization)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(self.authorization_form.clone())
            .send()
            .await
            .map_err(|_| OAuth2ProviderError::transient(None))?;

        let status = response.status();
        let retry_after = retry_after_for(status, response.headers().get(RETRY_AFTER), self.retry_after_cap);
        let oversize_kind = classify_status_without_body(status);
        let body = read_bounded_body(response, oversize_kind, retry_after).await?;
        classify_response(status, &body, retry_after)
    }
}

fn validate_endpoint(raw: &str) -> Result<Url, OAuth2ProviderError> {
    if raw.len() > OAUTH2_TOKEN_ENDPOINT_MAX_LEN {
        return Err(OAuth2ProviderError::permanent());
    }
    let endpoint = Url::parse(raw).map_err(|_| OAuth2ProviderError::permanent())?;
    if endpoint.scheme() != "https"
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(OAuth2ProviderError::permanent());
    }
    Ok(endpoint)
}

fn build_request_form(scopes: &[String]) -> Result<String, OAuth2ProviderError> {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("grant_type", "client_credentials");
    if !scopes.is_empty() {
        serializer.append_pair("scope", &scopes.join(" "));
    }
    let body = serializer.finish();
    if body.len() > REQUEST_FORM_MAX_LEN {
        return Err(OAuth2ProviderError::permanent());
    }
    Ok(body)
}

fn build_basic_authorization(bootstrap: &BootstrapCredentials) -> Result<HeaderValue, OAuth2ProviderError> {
    let encoded_id: String = url::form_urlencoded::byte_serialize(bootstrap.client_id.as_bytes()).collect();
    let encoded_secret =
        Zeroizing::new(url::form_urlencoded::byte_serialize(bootstrap.client_secret.as_bytes()).collect::<String>());
    let mut joined = Zeroizing::new(String::with_capacity(encoded_id.len() + encoded_secret.len() + 1));
    joined.push_str(&encoded_id);
    joined.push(':');
    joined.push_str(&encoded_secret);
    let encoded = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(joined.as_bytes()));
    let mut value = Zeroizing::new(String::with_capacity(encoded.len() + "Basic ".len()));
    value.push_str("Basic ");
    value.push_str(&encoded);
    if value.len() > AUTHORIZATION_VALUE_MAX_LEN {
        return Err(OAuth2ProviderError::permanent());
    }
    let mut header = HeaderValue::from_str(&value).map_err(|_| OAuth2ProviderError::permanent())?;
    header.set_sensitive(true);
    Ok(header)
}

async fn read_bounded_body(
    response: reqwest::Response,
    oversize_kind: OAuth2FailureKind,
    retry_after: Option<Duration>,
) -> Result<Zeroizing<Vec<u8>>, OAuth2ProviderError> {
    let mut body = Zeroizing::new(Vec::new());
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| OAuth2ProviderError::transient(None))?;
        let next_len = body.len().checked_add(chunk.len()).ok_or_else(|| match oversize_kind {
            OAuth2FailureKind::Transient => OAuth2ProviderError::transient(retry_after),
            _ => OAuth2ProviderError::permanent(),
        })?;
        if next_len > RESPONSE_BODY_MAX_LEN {
            return Err(match oversize_kind {
                OAuth2FailureKind::Transient => OAuth2ProviderError::transient(retry_after),
                _ => OAuth2ProviderError::permanent(),
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn classify_status_without_body(status: StatusCode) -> OAuth2FailureKind {
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        OAuth2FailureKind::Transient
    } else {
        OAuth2FailureKind::Permanent
    }
}

fn classify_response(
    status: StatusCode,
    body: &[u8],
    retry_after: Option<Duration>,
) -> Result<OAuth2Token, OAuth2ProviderError> {
    if status.is_success() {
        return parse_success_response(body);
    }
    if matches!(status, StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED)
        && serde_json::from_slice::<OAuthErrorResponse>(body).is_ok_and(|response| response.error == "invalid_client")
    {
        return Err(OAuth2ProviderError {
            kind: OAuth2FailureKind::InvalidClient,
            retry_after: None,
        });
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(OAuth2ProviderError::transient(retry_after));
    }
    Err(OAuth2ProviderError::permanent())
}

fn parse_success_response(body: &[u8]) -> Result<OAuth2Token, OAuth2ProviderError> {
    let response: OAuthSuccessResponse = serde_json::from_slice(body).map_err(|_| OAuth2ProviderError::permanent())?;
    if !response.token_type.eq_ignore_ascii_case("Bearer")
        || !(EXPIRES_IN_MIN_SECONDS..=EXPIRES_IN_MAX_SECONDS).contains(&response.expires_in)
        || response.access_token.len() > ACCESS_TOKEN_MAX_LEN
    {
        return Err(OAuth2ProviderError::permanent());
    }
    let data = BTreeMap::from([(
        "bearerToken".to_string(),
        ByteString(response.access_token.into_bytes()),
    )]);
    let material = BearerResolver
        .parse(&data)
        .map_err(|_| OAuth2ProviderError::permanent())?;
    Ok(OAuth2Token {
        access_token: material.token,
        expires_in: Duration::from_secs(response.expires_in),
    })
}

fn retry_after_for(status: StatusCode, value: Option<&HeaderValue>, cap: Duration) -> Option<Duration> {
    if !matches!(status, StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE) {
        return None;
    }
    let value = value?.to_str().ok()?;
    let delay = edgion_common::time::parse_retry_after(value, SystemTime::now())?;
    (!delay.is_zero()).then(|| delay.min(cap))
}

#[derive(Deserialize)]
struct OAuthSuccessResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct OAuthErrorResponse {
    error: String,
}

#[cfg(test)]
#[path = "../../../../tests/unit/services/credential_manager/providers/oauth2_test.rs"]
mod tests;
