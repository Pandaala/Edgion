//! OpenID Connect plugin implementation (Phase 1: Bearer + Discovery + JWKS).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use bytes::Bytes;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use pingora_http::ResponseHeader;
use rand::RngCore;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock};

use crate::core::conf_mgr::sync_runtime::resource_processor::get_secret;
use crate::core::plugins::edgion_plugins::common::auth_common::{
    send_auth_error_response, set_claims_headers as set_common_claims_headers, Claims,
};
use crate::core::plugins::edgion_plugins::common::http_client::get_http_client_with_ssl_verify;
use crate::core::plugins::edgion_plugins::common::jwt_common::{
    map_jwt_decode_error, resolve_algorithm_policy, select_jwk, validate_token_alg, JwkSelectError,
};
use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{
    EndpointAuthMethod, OpenidConnectConfig, UnauthAction, VerificationMode,
};

type OidcError = Box<dyn std::error::Error + Send + Sync>;
type OidcResult<T> = Result<T, OidcError>;
type VerifyResult<T> = Result<T, (u16, String)>;

#[derive(Debug, Clone, Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    jwks_uri: String,
    #[serde(default)]
    authorization_endpoint: Option<String>,
    #[serde(default)]
    token_endpoint: Option<String>,
    #[serde(default)]
    introspection_endpoint: Option<String>,
    #[serde(default)]
    userinfo_endpoint: Option<String>,
    #[serde(default)]
    end_session_endpoint: Option<String>,
    #[serde(default)]
    revocation_endpoint: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct JwksState {
    set: Option<JwkSet>,
    expires_at: Option<Instant>,
    last_refresh_at: Option<Instant>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuthorizationStateCookie {
    state: String,
    original_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code_verifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OidcSessionCookie {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    session_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    access_token: String,
    created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenEndpointResponse {
    access_token: String,
    #[allow(dead_code)]
    #[serde(default)]
    id_token: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[allow(dead_code)]
    #[serde(default)]
    token_type: Option<String>,
}

#[derive(Debug, Clone)]
struct RefreshSingleflightResult {
    payload: OidcSessionCookie,
    at: Instant,
}

#[derive(Debug, Clone)]
struct IntrospectionCacheEntry {
    claims: Value,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
struct AccessTokenCacheEntry {
    token: String,
    expires_at: Option<u64>,
}

/// OpenID Connect request filter plugin.
pub struct OpenidConnect {
    name: String,
    config: OpenidConnectConfig,
    #[allow(dead_code)]
    plugin_namespace: String,
    discovery_doc: Arc<RwLock<Option<DiscoveryDocument>>>,
    discovery_refresh: Arc<Mutex<()>>,
    jwks_state: Arc<RwLock<JwksState>>,
    jwks_refresh: Arc<Mutex<()>>,
    introspection_cache: Arc<RwLock<HashMap<String, IntrospectionCacheEntry>>>,
    access_token_cache: Arc<RwLock<HashMap<String, AccessTokenCacheEntry>>>,
    refresh_singleflight_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    refresh_singleflight_results: Arc<Mutex<HashMap<String, RefreshSingleflightResult>>>,
}

#[path = "openid_impl.rs"]
mod openid_impl;
#[path = "request_filter_impl.rs"]
mod request_filter_impl;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
