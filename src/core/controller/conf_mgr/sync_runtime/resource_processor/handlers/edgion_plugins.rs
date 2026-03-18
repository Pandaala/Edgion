//! EdgionPlugins Handler
//!
//! Handles EdgionPlugins resources with Gateway API standard status management.
//! Also resolves Secret references for plugins like JwtAuth.
//!
//! Features:
//! - parse: Resolve Secret references and register to SecretRefManager for cascading updates
//! - on_delete: Clear SecretRefManager references

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    condition_types, format_secret_key, get_secret, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::types::prelude_resources::EdgionPlugins;
use crate::types::resources::edgion_plugins::plugin_configs::{
    CertSourceMode, HmacCredential, KeyMetadata, ResolvedJweCredential, ResolvedJwtCredential,
};
use crate::types::resources::edgion_plugins::{EdgionPlugin, EdgionPluginsStatus};
use crate::types::ResourceKind;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use k8s_openapi::chrono::Utc;
use std::collections::{HashMap, HashSet};

/// EdgionPlugins handler
///
/// Features:
/// - parse: Resolve JWT/KeyAuth Secret references and register to SecretRefManager
/// - on_delete: Clear SecretRefManager references
pub struct EdgionPluginsHandler;

impl EdgionPluginsHandler {
    pub fn new() -> Self {
        Self
    }

    /// Resolve BasicAuth users from Secrets and register references to SecretRefManager.
    fn resolve_basic_auth_users(ep: &mut EdgionPlugins, resource_ref: &ResourceRef, ctx: &HandlerContext) {
        let ep_ns = ep.metadata.namespace.as_deref().unwrap_or("default");

        if let Some(ref mut plugins) = ep.spec.request_plugins {
            for entry in plugins.iter_mut() {
                if let EdgionPlugin::BasicAuth(ref mut config) = entry.plugin {
                    config.resolved_users = None;

                    let Some(secret_refs) = config.secret_refs.clone() else {
                        continue;
                    };
                    if secret_refs.is_empty() {
                        continue;
                    }

                    let mut resolved = HashMap::new();
                    for secret_ref in secret_refs {
                        let ns = secret_ref.namespace.as_ref().or(ep.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(ns, &secret_ref.name);
                        let ns_str = ns.map(|s| s.as_str()).unwrap_or(ep_ns);

                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        let Some(secret) = get_secret(Some(ns_str), &secret_ref.name) else {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "BasicAuth: Secret not found yet, will be reprocessed when Secret arrives"
                            );
                            continue;
                        };

                        let Some(username) = Self::read_secret_utf8(&secret, &["username"]) else {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "BasicAuth: Secret missing username"
                            );
                            continue;
                        };
                        let Some(password) = Self::read_secret_utf8(&secret, &["password"]) else {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "BasicAuth: Secret missing password"
                            );
                            continue;
                        };

                        if resolved.contains_key(&username) {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                username = %username,
                                "BasicAuth: duplicate username found, skipping"
                            );
                            continue;
                        }
                        resolved.insert(username, password);
                    }

                    if !resolved.is_empty() {
                        tracing::info!(
                            edgion_plugins = %resource_ref.key(),
                            user_count = resolved.len(),
                            "BasicAuth: Resolved users from Secrets"
                        );
                        config.resolved_users = Some(resolved);
                    } else {
                        tracing::warn!(
                            edgion_plugins = %resource_ref.key(),
                            "BasicAuth: No users resolved from Secrets"
                        );
                    }
                }
            }
        }
    }

    /// Resolve KeyAuth keys from Secrets and register references to SecretRefManager
    fn resolve_key_auth_keys(ep: &mut EdgionPlugins, resource_ref: &ResourceRef, ctx: &HandlerContext) {
        let ep_ns = ep.metadata.namespace.as_deref().unwrap_or("default");

        // Process request plugins
        if let Some(ref mut plugins) = ep.spec.request_plugins {
            for entry in plugins.iter_mut() {
                if let EdgionPlugin::KeyAuth(ref mut config) = entry.plugin {
                    let Some(ref secret_refs) = config.secret_refs else {
                        continue;
                    };

                    let whitelist: HashSet<&str> = config.upstream_header_fields.iter().map(|s| s.as_str()).collect();
                    let mut all_keys: HashMap<String, KeyMetadata> = HashMap::new();

                    for secret_ref in secret_refs {
                        let ns = secret_ref.namespace.as_ref().or(ep.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(ns, &secret_ref.name);
                        let ns_str = ns.map(|s| s.as_str()).unwrap_or(ep_ns);

                        // Register reference for cascading updates
                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        let Some(secret) = get_secret(Some(ns_str), &secret_ref.name) else {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "KeyAuth: Secret not found yet, will be reprocessed when Secret arrives"
                            );
                            continue;
                        };

                        // Try to get keys.yaml from data (base64 decoded) or string_data (plain text)
                        let keys_yaml_str: String = if let Some(data) = &secret.data {
                            // Try data field first (K8s stores decoded bytes here)
                            if let Some(keys_yaml_bytes) = data.get("keys.yaml") {
                                match String::from_utf8(keys_yaml_bytes.0.clone()) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        tracing::warn!(
                                            edgion_plugins = %resource_ref.key(),
                                            secret_key = %secret_key,
                                            error = %e,
                                            "KeyAuth: Failed to decode keys.yaml as UTF-8"
                                        );
                                        continue;
                                    }
                                }
                            } else if let Some(string_data) = &secret.string_data {
                                // Fallback to string_data if data doesn't have the key
                                if let Some(s) = string_data.get("keys.yaml") {
                                    s.clone()
                                } else {
                                    tracing::warn!(
                                        edgion_plugins = %resource_ref.key(),
                                        secret_key = %secret_key,
                                        "KeyAuth: Secret missing 'keys.yaml' field in both data and string_data"
                                    );
                                    continue;
                                }
                            } else {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "KeyAuth: Secret missing 'keys.yaml' field"
                                );
                                continue;
                            }
                        } else if let Some(string_data) = &secret.string_data {
                            // No data field, try string_data (used in local file testing)
                            if let Some(s) = string_data.get("keys.yaml") {
                                s.clone()
                            } else {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "KeyAuth: Secret missing 'keys.yaml' field in string_data"
                                );
                                continue;
                            }
                        } else {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "KeyAuth: Secret has no data or string_data"
                            );
                            continue;
                        };

                        // Parse YAML list of key entries
                        let keys_list: Vec<HashMap<String, String>> = match serde_yaml::from_str(&keys_yaml_str) {
                            Ok(list) => list,
                            Err(e) => {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    error = %e,
                                    "KeyAuth: Failed to parse keys.yaml"
                                );
                                continue;
                            }
                        };

                        // Process each key entry
                        for key_entry in keys_list {
                            let Some(key_value) = key_entry.get(&config.key_field) else {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    key_field = %config.key_field,
                                    "KeyAuth: Key entry missing key field"
                                );
                                continue;
                            };

                            // Extract whitelisted headers
                            let mut metadata = KeyMetadata::default();
                            for (field, value) in &key_entry {
                                if field != &config.key_field && whitelist.contains(field.as_str()) {
                                    metadata.headers.insert(field.clone(), value.clone());
                                }
                            }

                            // Check for duplicate keys
                            if all_keys.contains_key(key_value) {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "KeyAuth: Duplicate API key found, skipping"
                                );
                                continue;
                            }

                            all_keys.insert(key_value.clone(), metadata);
                        }
                    }

                    if !all_keys.is_empty() {
                        tracing::info!(
                            edgion_plugins = %resource_ref.key(),
                            key_count = all_keys.len(),
                            "KeyAuth: Resolved {} API keys from Secrets",
                            all_keys.len()
                        );
                        config.resolved_keys = Some(all_keys);
                    } else {
                        tracing::warn!(
                            edgion_plugins = %resource_ref.key(),
                            "KeyAuth: No API keys resolved from Secrets"
                        );
                    }
                }
            }
        }
    }

    /// Resolve JWT credentials from Secrets and register references to SecretRefManager
    fn resolve_jwt_credentials(ep: &mut EdgionPlugins, resource_ref: &ResourceRef, ctx: &HandlerContext) {
        let ep_ns = ep.metadata.namespace.as_deref().unwrap_or("default");

        // Process request plugins
        if let Some(ref mut plugins) = ep.spec.request_plugins {
            for entry in plugins.iter_mut() {
                if let EdgionPlugin::JwtAuth(ref mut config) = entry.plugin {
                    // Resolve single secret_ref
                    if let Some(ref secret_ref) = config.secret_ref {
                        let ns = secret_ref.namespace.as_ref().or(ep.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(ns, &secret_ref.name);
                        let ns_str = ns.map(|s| s.as_str()).unwrap_or(ep_ns);

                        // Register reference relationship (critical for cascading updates)
                        // When this Secret arrives or updates, SecretHandler will trigger requeue
                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        if let Some(secret) = get_secret(Some(ns_str), &secret_ref.name) {
                            if let Some(data) = &secret.data {
                                let mut cred = ResolvedJwtCredential::default();
                                // Try "secret" key for HS*
                                if let Some(secret_bytes) = data.get("secret") {
                                    cred.secret = Some(STANDARD.encode(&secret_bytes.0));
                                }
                                // Try "publicKey" key for RS*/ES*
                                if let Some(pk_bytes) = data.get("publicKey") {
                                    cred.public_key = String::from_utf8(pk_bytes.0.clone()).ok();
                                }
                                if cred.secret.is_some() || cred.public_key.is_some() {
                                    config.resolved_credential = Some(cred);
                                    tracing::debug!(
                                        edgion_plugins = %resource_ref.key(),
                                        secret_key = %secret_key,
                                        "JwtAuth: Secret resolved and credential filled"
                                    );
                                }
                            }
                        } else {
                            // Secret not found yet - this is normal if Secret arrives after EdgionPlugins
                            // The SecretRefManager reference ensures we'll be requeued when Secret arrives
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "JwtAuth: Secret not found yet, will be reprocessed when Secret arrives"
                            );
                        }
                    }

                    // Resolve multiple secret_refs
                    if let Some(ref secret_refs) = config.secret_refs {
                        let mut resolved = HashMap::new();
                        for secret_ref in secret_refs {
                            let ns = secret_ref.namespace.as_ref().or(ep.metadata.namespace.as_ref());
                            let secret_key = format_secret_key(ns, &secret_ref.name);
                            let ns_str = ns.map(|s| s.as_str()).unwrap_or(ep_ns);

                            // Register reference for cascading updates
                            ctx.secret_ref_manager()
                                .add_ref(secret_key.clone(), resource_ref.clone());

                            if let Some(secret) = get_secret(Some(ns_str), &secret_ref.name) {
                                if let Some(data) = &secret.data {
                                    let mut cred = ResolvedJwtCredential::default();
                                    // Get "key" identifier
                                    if let Some(key_bytes) = data.get("key") {
                                        cred.key = String::from_utf8(key_bytes.0.clone()).ok();
                                    }
                                    // Get "secret" for HS*
                                    if let Some(secret_bytes) = data.get("secret") {
                                        cred.secret = Some(STANDARD.encode(&secret_bytes.0));
                                    }
                                    // Get "publicKey" for RS*/ES*
                                    if let Some(pk_bytes) = data.get("publicKey") {
                                        cred.public_key = String::from_utf8(pk_bytes.0.clone()).ok();
                                    }
                                    if let Some(ref key) = cred.key {
                                        resolved.insert(key.clone(), cred);
                                    }
                                }
                            }
                        }
                        if !resolved.is_empty() {
                            config.resolved_credentials = Some(resolved);
                        }
                    }
                }
            }
        }
    }

    /// Resolve HMAC credentials from Secrets and register references to SecretRefManager.
    fn resolve_hmac_credentials(ep: &mut EdgionPlugins, resource_ref: &ResourceRef, ctx: &HandlerContext) {
        let ep_ns = ep.metadata.namespace.as_deref().unwrap_or("default");

        if let Some(ref mut plugins) = ep.spec.request_plugins {
            for entry in plugins.iter_mut() {
                if let EdgionPlugin::HmacAuth(ref mut config) = entry.plugin {
                    config.resolved_credentials = None;

                    let Some(secret_refs) = config.secret_refs.clone() else {
                        continue;
                    };
                    if secret_refs.is_empty() {
                        continue;
                    }

                    let whitelist: HashSet<&str> = config.upstream_header_fields.iter().map(|v| v.as_str()).collect();
                    let mut resolved: HashMap<String, HmacCredential> = HashMap::new();

                    for secret_ref in secret_refs {
                        let ns = secret_ref.namespace.as_ref().or(ep.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(ns, &secret_ref.name);
                        let ns_str = ns.map(|s| s.as_str()).unwrap_or(ep_ns);

                        // Register reference for cascading updates
                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        let Some(secret) = get_secret(Some(ns_str), &secret_ref.name) else {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "HmacAuth: Secret not found yet, will be reprocessed when Secret arrives"
                            );
                            continue;
                        };

                        let Some(credentials_yaml) = Self::read_secret_utf8(&secret, &["credentials.yaml"]) else {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "HmacAuth: Secret missing credentials.yaml"
                            );
                            continue;
                        };

                        let entries: Vec<HashMap<String, serde_yaml::Value>> =
                            match serde_yaml::from_str(&credentials_yaml) {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::warn!(
                                        edgion_plugins = %resource_ref.key(),
                                        secret_key = %secret_key,
                                        error = %e,
                                        "HmacAuth: Failed to parse credentials.yaml"
                                    );
                                    continue;
                                }
                            };

                        for entry_map in entries {
                            let username = entry_map
                                .get(&config.username_field)
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .filter(|v| !v.is_empty());
                            let secret_value = entry_map
                                .get(&config.secret_field)
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .filter(|v| !v.is_empty());

                            let (Some(username), Some(secret_value)) = (username, secret_value) else {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    username_field = %config.username_field,
                                    secret_field = %config.secret_field,
                                    "HmacAuth: credential entry missing username/secret field"
                                );
                                continue;
                            };

                            if resolved.contains_key(username) {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    username = %username,
                                    "HmacAuth: duplicate username found in credentials, skipping"
                                );
                                continue;
                            }

                            if secret_value.len() < config.min_secret_length {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    username = %username,
                                    secret_len = secret_value.len(),
                                    min_len = config.min_secret_length,
                                    "HmacAuth: secret length is below configured minimum"
                                );
                            }

                            let mut headers = HashMap::new();
                            if let Some(serde_yaml::Value::Mapping(map)) = entry_map.get("headers") {
                                for (k, v) in map {
                                    let Some(header_name) = k.as_str() else {
                                        continue;
                                    };
                                    if !whitelist.contains(header_name) {
                                        continue;
                                    }

                                    let header_value = if let Some(value) = v.as_str() {
                                        value.to_string()
                                    } else {
                                        match v {
                                            serde_yaml::Value::Bool(b) => b.to_string(),
                                            serde_yaml::Value::Number(n) => n.to_string(),
                                            _ => continue,
                                        }
                                    };
                                    headers.insert(header_name.to_string(), header_value);
                                }
                            }

                            resolved.insert(
                                username.to_string(),
                                HmacCredential {
                                    secret: secret_value.as_bytes().to_vec(),
                                    headers,
                                },
                            );
                        }
                    }

                    if !resolved.is_empty() {
                        tracing::info!(
                            edgion_plugins = %resource_ref.key(),
                            credential_count = resolved.len(),
                            "HmacAuth: Resolved credentials from Secrets"
                        );
                        config.resolved_credentials = Some(resolved);
                    } else {
                        tracing::warn!(
                            edgion_plugins = %resource_ref.key(),
                            "HmacAuth: No credentials resolved from Secrets"
                        );
                    }
                }
            }
        }
    }

    /// Resolve JWE decrypt credentials from Secrets and register references.
    fn resolve_jwe_credentials(ep: &mut EdgionPlugins, resource_ref: &ResourceRef, ctx: &HandlerContext) {
        let ep_ns = ep.metadata.namespace.as_deref().unwrap_or("default");

        if let Some(ref mut plugins) = ep.spec.request_plugins {
            for entry in plugins.iter_mut() {
                if let EdgionPlugin::JweDecrypt(ref mut config) = entry.plugin {
                    config.resolved_credential = None;

                    let Some(secret_ref) = config.secret_ref.clone() else {
                        continue;
                    };

                    let ns = secret_ref.namespace.as_ref().or(ep.metadata.namespace.as_ref());
                    let secret_key = format_secret_key(ns, &secret_ref.name);
                    let ns_str = ns.map(|s| s.as_str()).unwrap_or(ep_ns);

                    ctx.secret_ref_manager()
                        .add_ref(secret_key.clone(), resource_ref.clone());

                    if let Some(secret) = get_secret(Some(ns_str), &secret_ref.name) {
                        let mut resolved = ResolvedJweCredential::default();

                        if let Some(data) = &secret.data {
                            if let Some(secret_bytes) = data.get("secret") {
                                resolved.secret = Some(STANDARD.encode(&secret_bytes.0));
                            }
                        }

                        if resolved.secret.is_none() {
                            if let Some(string_data) = &secret.string_data {
                                if let Some(secret_str) = string_data.get("secret") {
                                    resolved.secret = Some(STANDARD.encode(secret_str.as_bytes()));
                                }
                            }
                        }

                        if resolved.secret.is_some() {
                            config.resolved_credential = Some(resolved);
                            tracing::debug!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "JweDecrypt: Secret resolved and credential filled"
                            );
                        }
                    } else {
                        tracing::info!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            "JweDecrypt: Secret not found yet, will be reprocessed when Secret arrives"
                        );
                    }
                }
            }
        }
    }

    /// Resolve HeaderCertAuth CA secrets and register secret references.
    fn resolve_header_cert_auth_ca_secrets(ep: &mut EdgionPlugins, resource_ref: &ResourceRef, ctx: &HandlerContext) {
        let ep_ns = ep.metadata.namespace.as_deref().unwrap_or("default");

        if let Some(ref mut plugins) = ep.spec.request_plugins {
            for entry in plugins.iter_mut() {
                if let EdgionPlugin::HeaderCertAuth(ref mut config) = entry.plugin {
                    config.resolved_ca_secrets = None;

                    if config.mode != CertSourceMode::Header {
                        continue;
                    }
                    if config.ca_secret_refs.is_empty() {
                        continue;
                    }

                    let mut resolved = Vec::new();
                    for secret_ref in &config.ca_secret_refs {
                        let ns = secret_ref.namespace.as_ref().or(ep.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(ns, &secret_ref.name);
                        let ns_str = ns.map(|s| s.as_str()).unwrap_or(ep_ns);

                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        let Some(secret) = get_secret(Some(ns_str), &secret_ref.name) else {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "HeaderCertAuth: CA secret not found yet, will be reprocessed when Secret arrives"
                            );
                            continue;
                        };

                        let has_ca = secret.data.as_ref().is_some_and(|data| data.contains_key("ca.crt"))
                            || secret
                                .string_data
                                .as_ref()
                                .is_some_and(|data| data.contains_key("ca.crt"));
                        if !has_ca {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "HeaderCertAuth: Secret missing ca.crt"
                            );
                            continue;
                        }

                        resolved.push(secret);
                    }

                    if !resolved.is_empty() {
                        tracing::info!(
                            edgion_plugins = %resource_ref.key(),
                            ca_secret_count = resolved.len(),
                            "HeaderCertAuth: Resolved CA secrets"
                        );
                        config.resolved_ca_secrets = Some(resolved);
                    }
                }
            }
        }
    }

    fn read_secret_utf8(secret: &k8s_openapi::api::core::v1::Secret, keys: &[&str]) -> Option<String> {
        if let Some(data) = &secret.data {
            for key in keys {
                if let Some(bytes) = data.get(*key) {
                    if let Ok(value) = String::from_utf8(bytes.0.clone()) {
                        if !value.trim().is_empty() {
                            return Some(value);
                        }
                    }
                }
            }
        }
        if let Some(string_data) = &secret.string_data {
            for key in keys {
                if let Some(value) = string_data.get(*key) {
                    if !value.trim().is_empty() {
                        return Some(value.clone());
                    }
                }
            }
        }
        None
    }

    /// Resolve OpenID Connect secrets and register references to SecretRefManager.
    fn resolve_openid_connect_secrets(ep: &mut EdgionPlugins, resource_ref: &ResourceRef, ctx: &HandlerContext) {
        let ep_ns = ep.metadata.namespace.as_deref().unwrap_or("default");

        if let Some(ref mut plugins) = ep.spec.request_plugins {
            for entry in plugins.iter_mut() {
                if let EdgionPlugin::OpenidConnect(ref mut config) = entry.plugin {
                    // Reset runtime-resolved fields first to avoid stale values on update.
                    config.resolved_client_secret = None;
                    config.resolved_session_secret = None;

                    if let Some(secret_ref) = config.client_secret_ref.clone() {
                        let ns = secret_ref.namespace.as_ref().or(ep.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(ns, &secret_ref.name);
                        let ns_str = ns.map(|s| s.as_str()).unwrap_or(ep_ns);

                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        if let Some(secret) = get_secret(Some(ns_str), &secret_ref.name) {
                            let resolved =
                                Self::read_secret_utf8(&secret, &["clientSecret", "client_secret", "secret"]);
                            if let Some(value) = resolved {
                                config.resolved_client_secret = Some(value);
                                tracing::debug!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "OpenidConnect: client secret resolved"
                                );
                            } else {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "OpenidConnect: Secret missing client secret key (clientSecret/client_secret/secret)"
                                );
                            }
                        } else {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "OpenidConnect: client secret not found yet, will be reprocessed when Secret arrives"
                            );
                        }
                    }

                    if let Some(secret_ref) = config.session_secret_ref.clone() {
                        let ns = secret_ref.namespace.as_ref().or(ep.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(ns, &secret_ref.name);
                        let ns_str = ns.map(|s| s.as_str()).unwrap_or(ep_ns);

                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        if let Some(secret) = get_secret(Some(ns_str), &secret_ref.name) {
                            let resolved =
                                Self::read_secret_utf8(&secret, &["sessionSecret", "session_secret", "secret"]);
                            if let Some(value) = resolved {
                                config.resolved_session_secret = Some(value);
                                tracing::debug!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "OpenidConnect: session secret resolved"
                                );
                            } else {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "OpenidConnect: Secret missing session secret key (sessionSecret/session_secret/secret)"
                                );
                            }
                        } else {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "OpenidConnect: session secret not found yet, will be reprocessed when Secret arrives"
                            );
                        }
                    }
                }
            }
        }
    }
}

impl Default for EdgionPluginsHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<EdgionPlugins> for EdgionPluginsHandler {
    fn preparse(&self, ep: &mut EdgionPlugins, _ctx: &HandlerContext) -> Vec<String> {
        // Build plugin runtime and collect validation errors
        ep.preparse();
        ep.get_preparse_errors().to_vec()
    }

    fn parse(&self, mut ep: EdgionPlugins, ctx: &HandlerContext) -> ProcessResult<EdgionPlugins> {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            ep.metadata.namespace.clone(),
            ep.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        // Resolve BasicAuth users from Secrets and register references
        Self::resolve_basic_auth_users(&mut ep, &resource_ref, ctx);

        // Resolve JWT credentials from Secrets and register references
        Self::resolve_jwt_credentials(&mut ep, &resource_ref, ctx);

        // Resolve JWE decrypt credentials from Secrets and register references
        Self::resolve_jwe_credentials(&mut ep, &resource_ref, ctx);

        // Resolve HeaderCertAuth CA secrets and register references
        Self::resolve_header_cert_auth_ca_secrets(&mut ep, &resource_ref, ctx);

        // Resolve HMAC credentials from Secrets and register references
        Self::resolve_hmac_credentials(&mut ep, &resource_ref, ctx);

        // Resolve KeyAuth keys from Secrets and register references
        Self::resolve_key_auth_keys(&mut ep, &resource_ref, ctx);

        // Resolve OpenID Connect secrets and register references
        Self::resolve_openid_connect_secrets(&mut ep, &resource_ref, ctx);

        // Note: preparse() is called by processor before parse(), so we don't call it here

        ProcessResult::Continue(ep)
    }

    fn on_delete(&self, ep: &EdgionPlugins, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            ep.metadata.namespace.clone(),
            ep.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        tracing::debug!(
            edgion_plugins = %resource_ref.key(),
            "Cleared secret references on EdgionPlugins delete"
        );
    }

    fn update_status(&self, ep: &mut EdgionPlugins, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = ep.metadata.generation;

        // Note: validation_errors already includes preparse errors (merged by processor)

        // Initialize status if not present
        let status = ep
            .status
            .get_or_insert_with(|| EdgionPluginsStatus { conditions: vec![] });

        // Set Accepted condition
        let accepted = if validation_errors.is_empty() {
            k8s_condition_true(condition_types::ACCEPTED, "Accepted", "Resource accepted", generation)
        } else {
            k8s_condition_false(
                condition_types::ACCEPTED,
                "Invalid",
                &validation_errors.join("; "),
                generation,
            )
        };
        update_k8s_condition(&mut status.conditions, accepted);
    }
}

/// Create a k8s_openapi Condition with True status
fn k8s_condition_true(type_: &str, reason: &str, message: &str, observed_generation: Option<i64>) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "True".to_string(),
        reason: reason.to_string(),
        message: message.to_string(),
        last_transition_time: Time(Utc::now()),
        observed_generation,
    }
}

/// Create a k8s_openapi Condition with False status
fn k8s_condition_false(type_: &str, reason: &str, message: &str, observed_generation: Option<i64>) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "False".to_string(),
        reason: reason.to_string(),
        message: message.to_string(),
        last_transition_time: Time(Utc::now()),
        observed_generation,
    }
}

/// Update or insert a k8s_openapi Condition
fn update_k8s_condition(conditions: &mut Vec<Condition>, new_condition: Condition) {
    if let Some(existing) = conditions.iter_mut().find(|c| c.type_ == new_condition.type_) {
        let status_changed = existing.status != new_condition.status;
        existing.status = new_condition.status;
        existing.reason = new_condition.reason;
        existing.message = new_condition.message;
        existing.observed_generation = new_condition.observed_generation;
        if status_changed {
            existing.last_transition_time = new_condition.last_transition_time;
        }
    } else {
        conditions.push(new_condition);
    }
}

#[cfg(test)]
mod tests {
    use super::EdgionPluginsHandler;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
        replace_all_secrets, HandlerContext, ProcessResult, ProcessorHandler, SecretRefManager,
    };
    use crate::types::resources::edgion_plugins::{
        BasicAuthConfig, CertSourceMode, EdgionPlugin, EdgionPlugins, EdgionPluginsSpec, HeaderCertAuthConfig,
        HmacAuthConfig, JweDecryptConfig, OpenidConnectConfig, RequestFilterEntry,
    };
    use crate::types::resources::gateway::SecretObjectReference;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Arc, Mutex, OnceLock};

    static SECRET_STORE_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn build_secret(namespace: &str, name: &str, key: &str, value: &str) -> Secret {
        let mut data = BTreeMap::new();
        data.insert(key.to_string(), ByteString(value.as_bytes().to_vec()));
        Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        }
    }

    fn build_basic_auth_secret(namespace: &str, name: &str, username: &str, password: &str) -> Secret {
        let mut data = BTreeMap::new();
        data.insert("username".to_string(), ByteString(username.as_bytes().to_vec()));
        data.insert("password".to_string(), ByteString(password.as_bytes().to_vec()));
        Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        }
    }

    #[test]
    fn test_parse_resolves_basic_auth_users_and_registers_refs() {
        let _guard = SECRET_STORE_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("secret store test lock poisoned");
        replace_all_secrets(HashMap::new());

        let alice = build_basic_auth_secret("default", "basic-auth-alice", "alice", "alice-password");
        let bob = build_basic_auth_secret("default", "basic-auth-bob", "bob", "bob-password");
        let mut secrets = HashMap::new();
        secrets.insert("default/basic-auth-alice".to_string(), alice);
        secrets.insert("default/basic-auth-bob".to_string(), bob);
        replace_all_secrets(secrets);

        let mut ep = EdgionPlugins::new("basic-auth-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::BasicAuth(
            BasicAuthConfig {
                secret_refs: Some(vec![
                    SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "basic-auth-alice".to_string(),
                        namespace: None,
                    },
                    SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "basic-auth-bob".to_string(),
                        namespace: None,
                    },
                ]),
                ..Default::default()
            },
        ))]);

        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let ctx = HandlerContext::new(secret_ref_manager.clone(), None, None, Default::default(), 5);
        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx) {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let entry = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .expect("missing request plugin");
        let config = match &entry.plugin {
            EdgionPlugin::BasicAuth(c) => c,
            _ => panic!("unexpected plugin type"),
        };

        let resolved_users = config.resolved_users.as_ref().expect("expected resolved users");
        assert_eq!(resolved_users.get("alice").map(|v| v.as_str()), Some("alice-password"));
        assert_eq!(resolved_users.get("bob").map(|v| v.as_str()), Some("bob-password"));
        assert_eq!(ctx.secret_ref_manager().get_refs("default/basic-auth-alice").len(), 1);
        assert_eq!(ctx.secret_ref_manager().get_refs("default/basic-auth-bob").len(), 1);

        replace_all_secrets(HashMap::new());
    }

    #[test]
    fn test_parse_resolves_openid_connect_secrets_and_registers_refs() {
        let _guard = SECRET_STORE_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("secret store test lock poisoned");
        replace_all_secrets(HashMap::new());

        let client_secret = build_secret("default", "oidc-client", "clientSecret", "client-secret-value");
        let session_secret = build_secret("default", "oidc-session", "sessionSecret", "session-secret-value");
        let mut secrets = HashMap::new();
        secrets.insert("default/oidc-client".to_string(), client_secret);
        secrets.insert("default/oidc-session".to_string(), session_secret);
        replace_all_secrets(secrets);

        let mut ep = EdgionPlugins::new("oidc-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::OpenidConnect(Box::new(
            OpenidConnectConfig {
                discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
                client_id: "my-client".to_string(),
                client_secret_ref: Some(SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "oidc-client".to_string(),
                    namespace: None,
                }),
                session_secret_ref: Some(SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "oidc-session".to_string(),
                    namespace: None,
                }),
                ..Default::default()
            },
        )))]);

        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let ctx = HandlerContext::new(secret_ref_manager.clone(), None, None, Default::default(), 5);
        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx) {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let entry = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .expect("missing request plugin");
        let config = match &entry.plugin {
            EdgionPlugin::OpenidConnect(c) => c,
            _ => panic!("unexpected plugin type"),
        };

        assert_eq!(config.resolved_client_secret.as_deref(), Some("client-secret-value"));
        assert_eq!(config.resolved_session_secret.as_deref(), Some("session-secret-value"));
        assert_eq!(ctx.secret_ref_manager().get_refs("default/oidc-client").len(), 1);
        assert_eq!(ctx.secret_ref_manager().get_refs("default/oidc-session").len(), 1);

        replace_all_secrets(HashMap::new());
    }

    #[test]
    fn test_parse_resolves_jwe_secret_and_registers_refs() {
        let _guard = SECRET_STORE_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("secret store test lock poisoned");
        replace_all_secrets(HashMap::new());

        let jwe_secret = build_secret("default", "jwe-secret", "secret", "0123456789abcdef0123456789abcdef");
        let mut secrets = HashMap::new();
        secrets.insert("default/jwe-secret".to_string(), jwe_secret);
        replace_all_secrets(secrets);

        let mut ep = EdgionPlugins::new("jwe-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::JweDecrypt(
            JweDecryptConfig {
                secret_ref: Some(SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "jwe-secret".to_string(),
                    namespace: None,
                }),
                ..Default::default()
            },
        ))]);

        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let ctx = HandlerContext::new(secret_ref_manager.clone(), None, None, Default::default(), 5);
        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx) {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let entry = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .expect("missing request plugin");
        let config = match &entry.plugin {
            EdgionPlugin::JweDecrypt(c) => c,
            _ => panic!("unexpected plugin type"),
        };

        assert!(config.resolved_credential.is_some());
        let resolved_secret = config
            .resolved_credential
            .as_ref()
            .and_then(|c| c.secret.as_ref())
            .expect("expected resolved secret");
        assert!(!resolved_secret.is_empty());
        assert_eq!(ctx.secret_ref_manager().get_refs("default/jwe-secret").len(), 1);

        replace_all_secrets(HashMap::new());
    }

    #[test]
    fn test_parse_resolves_hmac_credentials_and_registers_refs() {
        let _guard = SECRET_STORE_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("secret store test lock poisoned");
        replace_all_secrets(HashMap::new());

        let credentials_yaml = r#"
- username: alice
  secret: alice-secret-32-bytes-0123456789abcd
  headers:
    X-Consumer-Username: alice
    X-Ignored: ignored
- username: bob
  secret: bob-secret-32-bytes-0123456789abcde
  headers:
    X-Consumer-Username: bob
"#;
        let hmac_secret = build_secret("default", "hmac-credentials", "credentials.yaml", credentials_yaml);
        let mut secrets = HashMap::new();
        secrets.insert("default/hmac-credentials".to_string(), hmac_secret);
        replace_all_secrets(secrets);

        let mut ep = EdgionPlugins::new("hmac-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::HmacAuth(HmacAuthConfig {
            secret_refs: Some(vec![SecretObjectReference {
                group: None,
                kind: None,
                name: "hmac-credentials".to_string(),
                namespace: None,
            }]),
            upstream_header_fields: vec!["X-Consumer-Username".to_string()],
            ..Default::default()
        }))]);

        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let ctx = HandlerContext::new(secret_ref_manager.clone(), None, None, Default::default(), 5);
        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx) {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let entry = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .expect("missing request plugin");
        let config = match &entry.plugin {
            EdgionPlugin::HmacAuth(c) => c,
            _ => panic!("unexpected plugin type"),
        };

        let resolved = config
            .resolved_credentials
            .as_ref()
            .expect("expected resolved credentials");
        assert_eq!(resolved.len(), 2);

        let alice = resolved.get("alice").expect("missing alice credential");
        assert_eq!(alice.secret, b"alice-secret-32-bytes-0123456789abcd".to_vec());
        assert_eq!(
            alice.headers.get("X-Consumer-Username").map(|v| v.as_str()),
            Some("alice")
        );
        assert!(!alice.headers.contains_key("X-Ignored"));

        assert_eq!(ctx.secret_ref_manager().get_refs("default/hmac-credentials").len(), 1);

        replace_all_secrets(HashMap::new());
    }

    #[test]
    fn test_parse_resolves_header_cert_auth_ca_secrets_and_registers_refs() {
        let _guard = SECRET_STORE_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("secret store test lock poisoned");
        replace_all_secrets(HashMap::new());

        let ca_secret = build_secret(
            "default",
            "header-cert-ca",
            "ca.crt",
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
        );
        let mut secrets = HashMap::new();
        secrets.insert("default/header-cert-ca".to_string(), ca_secret);
        replace_all_secrets(secrets);

        let mut ep = EdgionPlugins::new("header-cert-auth-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::HeaderCertAuth(
            HeaderCertAuthConfig {
                mode: CertSourceMode::Header,
                ca_secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "header-cert-ca".to_string(),
                    namespace: None,
                }],
                ..Default::default()
            },
        ))]);

        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let ctx = HandlerContext::new(secret_ref_manager.clone(), None, None, Default::default(), 5);
        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx) {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let entry = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .expect("missing request plugin");
        let config = match &entry.plugin {
            EdgionPlugin::HeaderCertAuth(c) => c,
            _ => panic!("unexpected plugin type"),
        };

        let resolved = config
            .resolved_ca_secrets
            .as_ref()
            .expect("expected resolved ca secrets");
        assert_eq!(resolved.len(), 1);
        assert_eq!(ctx.secret_ref_manager().get_refs("default/header-cert-ca").len(), 1);

        replace_all_secrets(HashMap::new());
    }
}
