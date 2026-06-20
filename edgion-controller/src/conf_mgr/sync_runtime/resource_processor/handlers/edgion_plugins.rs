//! EdgionPlugins Handler
//!
//! Handles EdgionPlugins resources with Gateway API standard status management.
//! Delegates per-plugin Secret resolution to `plugins::resolve_credential`.
//!
//! Features:
//! - parse: Resolve Secret references and register to SecretRefManager for cascading updates
//! - parse: Register `overrideRef` EdgionConfigData → EP in `edgion_config_data_refs` for requeue on change
//! - on_delete: Clear SecretRefManager references + EdgionConfigData refs
//! - update_status: Validate `overrideRef` resolves to a `RegionRouteOverride` EdgionConfigData

use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, pick_resolved_refs_reason, update_condition, ResolvedRefsError,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_types, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::{EdgionPlugin, EdgionPluginsStatus};
use edgion_resources::ResourceKind;

use super::plugins::resolve_credential as cred;

pub struct EdgionPluginsHandler;

impl EdgionPluginsHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionPluginsHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// RegionRoute overrideRef helpers
// ============================================================================

impl EdgionPluginsHandler {
    /// Clear all EdgionConfigData reverse refs (overrideRef + IpList allow/deny refs) registered by this EdgionPlugins.
    fn clear_config_data_refs(ep: &EdgionPlugins, ctx: &HandlerContext) {
        let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
        let ep_name = ep.metadata.name.as_deref().unwrap_or("");
        let ep_key = format!("{}:{}/{}", ResourceKind::EdgionPlugins.as_str(), ns, ep_name);
        ctx.runtime().edgion_config_data_refs.clear_refs_from(&ep_key);
    }

    /// Collect all `overrideRef` EdgionConfigData keys from RegionRoute plugins.
    /// Returns "namespace/name" keys (namespace defaults to EP's namespace when absent).
    fn get_region_route_override_keys(ep: &EdgionPlugins) -> Vec<String> {
        let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
        let Some(plugins) = &ep.spec.request_plugins else {
            return vec![];
        };
        plugins
            .iter()
            .filter_map(|e| {
                if let EdgionPlugin::RegionRoute(ref config) = e.plugin {
                    config.override_ref.as_ref().map(|r| {
                        let override_ns = r.namespace.as_deref().unwrap_or(ns);
                        format!("{}/{}", override_ns, r.name)
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Collect "namespace/name" keys for every IpList referenced by an `IpRestriction`
    /// plugin's `allow_refs`/`deny_refs` in `request_plugins`. Namespace defaults to the
    /// EP namespace.
    fn get_ip_list_ref_keys(ep: &EdgionPlugins) -> Vec<String> {
        let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
        let Some(plugins) = &ep.spec.request_plugins else {
            return vec![];
        };
        let mut keys = Vec::new();
        for e in plugins {
            if let EdgionPlugin::IpRestriction(ref config) = e.plugin {
                for r in config
                    .allow_refs
                    .iter()
                    .flatten()
                    .chain(config.deny_refs.iter().flatten())
                {
                    let rns = r.namespace.as_deref().unwrap_or(ns);
                    keys.push(format!("{}/{}", rns, r.name));
                }
            }
        }
        keys
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionPlugins> for EdgionPluginsHandler {
    fn preparse(&self, ep: &mut EdgionPlugins, _ctx: &HandlerContext) -> Vec<String> {
        // The plugin runtime is built gateway-side (`compile_plugins`); the
        // schema carries no runtime. The controller only needs the validation
        // errors for the K8s status condition, so it runs the schema-layer
        // `validate_plugins` instead of compiling a runtime it would throw away.
        // This keeps the controller free of any `edgion-gateway` dependency. The
        // one gap versus `compile_plugins` is the DSL deep checks (Rhai compile /
        // VM init), which require the gateway-only DSL VM; the data plane still
        // fails closed for those at runtime. See
        // `edgion_resources::resources::edgion_plugins::validate`.
        ep.validate_plugins()
    }

    async fn parse(&self, mut ep: EdgionPlugins, ctx: &HandlerContext) -> ProcessResult<EdgionPlugins> {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            ep.metadata.namespace.clone(),
            ep.metadata.name.clone().unwrap_or_default(),
        );

        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        cred::resolve_basic_auth(&mut ep, &resource_ref, ctx);
        cred::resolve_jwt(&mut ep, &resource_ref, ctx);
        cred::resolve_jwe(&mut ep, &resource_ref, ctx);
        cred::resolve_header_cert_auth(&mut ep, &resource_ref, ctx);
        cred::resolve_hmac(&mut ep, &resource_ref, ctx);
        cred::resolve_key_auth(&mut ep, &resource_ref, ctx);
        cred::resolve_openid_connect(&mut ep, &resource_ref, ctx);
        cred::resolve_csrf(&mut ep, &resource_ref, ctx);

        // Clear existing EdgionConfigData override refs for this EP, then re-register from scratch.
        Self::clear_config_data_refs(&ep, ctx);

        let ns = ep.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let ep_name = ep.metadata.name.as_deref().unwrap_or("").to_string();
        let ep_key = format!("{}:{}/{}", ResourceKind::EdgionPlugins.as_str(), ns, ep_name);

        // Register requeue watcher: EP → EdgionConfigData(overrideRef).
        // When the referenced EdgionConfigData changes, the EdgionConfigDataHandler requeues this EP.
        for override_key in Self::get_region_route_override_keys(&ep) {
            ctx.runtime()
                .edgion_config_data_refs
                .add_ref(override_key, ep_key.clone());
        }

        // Register requeue watcher: EP → EdgionConfigData(IpList refs).
        // When the referenced EdgionConfigData changes, the EdgionConfigDataHandler requeues this EP.
        for ip_list_key in Self::get_ip_list_ref_keys(&ep) {
            ctx.runtime()
                .edgion_config_data_refs
                .add_ref(ip_list_key, ep_key.clone());
        }

        ProcessResult::Continue(ep)
    }

    async fn on_delete(&self, ep: &EdgionPlugins, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            ep.metadata.namespace.clone(),
            ep.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        Self::clear_config_data_refs(ep, ctx);

        tracing::debug!(
            edgion_plugins = %resource_ref.key(),
            "Cleared secret and EdgionConfigData references on EdgionPlugins delete"
        );
    }

    fn update_status(&self, ep: &mut EdgionPlugins, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = ep.metadata.generation;

        // Accepted reflects preparse + validate errors (already merged by processor).
        let accepted = if validation_errors.is_empty() {
            condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                "Resource accepted",
                generation,
            )
        } else {
            condition_false(
                condition_types::ACCEPTED,
                condition_reasons::INVALID,
                validation_errors.join("; "),
                generation,
            )
        };

        let mut resolved_errors = Self::resolve_secret_refs(ep, ctx);
        resolved_errors.extend(resolve_override_refs(ep, ctx));
        resolved_errors.extend(resolve_ip_list_refs(ep, ctx));
        let resolved_refs = if resolved_errors.is_empty() {
            condition_true(
                condition_types::RESOLVED_REFS,
                condition_reasons::RESOLVED_REFS,
                "All references resolved",
                generation,
            )
        } else {
            // Use the shared priority helper rather than `errors[0]` —
            // `SecretRefManager::get_dependencies` iterates a set with no
            // stable order, so taking the first element would make the
            // surfaced reason depend on hashing. `pick_resolved_refs_reason`
            // deterministically picks the most severe variant.
            let reason = pick_resolved_refs_reason(&resolved_errors);
            let message = resolved_errors
                .iter()
                .map(|e| e.message())
                .collect::<Vec<_>>()
                .join("; ");
            condition_false(condition_types::RESOLVED_REFS, reason, message, generation)
        };

        let status = ep
            .status
            .get_or_insert_with(|| EdgionPluginsStatus { conditions: vec![] });
        update_condition(&mut status.conditions, accepted);
        update_condition(&mut status.conditions, resolved_refs);
    }
}

impl EdgionPluginsHandler {
    /// Inspect every Secret this EdgionPlugins depends on (tracked in `SecretRefManager`
    /// by the `parse()` resolvers) and flag any that aren't present in the Secret store.
    fn resolve_secret_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            ep.metadata.namespace.clone(),
            ep.metadata.name.clone().unwrap_or_default(),
        );

        let mut errors = Vec::new();
        // Sort dependency keys so the joined condition.message is deterministic
        // across reconciles. get_dependencies' backing HashSet has non-deterministic
        // iteration order; without sorting, status_diff::condition_fields_equal
        // (which compares `message`) would trigger spurious K8s status writes.
        let mut deps = ctx.secret_ref_manager().get_dependencies(&resource_ref.key());
        deps.sort();
        for dep_key in deps {
            // dep_key is "<ns>/<name>" (or just "<name>" if namespace-less, per format_secret_key).
            let (ns, name) = match dep_key.split_once('/') {
                Some((ns, name)) => (ns.to_string(), name.to_string()),
                None => (String::new(), dep_key.clone()),
            };
            let ns_opt = if ns.is_empty() { None } else { Some(ns.as_str()) };
            if ctx.runtime().secret_store.get(ns_opt, &name).is_none() {
                errors.push(ResolvedRefsError::SecretNotFound {
                    namespace: if ns.is_empty() { "default".to_string() } else { ns },
                    name,
                });
            }
        }
        errors
    }
}

/// Check each RegionRoute plugin's `overrideRef` against the EdgionConfigData store.
/// Returns one `EdgionConfigDataNotFound` error per missing or wrong-type reference.
/// A type mismatch (ref exists but is not `RegionRouteOverride`) is also reported.
pub fn resolve_override_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
    let Some(plugins) = &ep.spec.request_plugins else {
        return vec![];
    };
    let mut errors = Vec::new();
    for entry in plugins {
        if let EdgionPlugin::RegionRoute(ref config) = entry.plugin {
            if let Some(ref override_ref) = config.override_ref {
                let override_ns = override_ref.namespace.as_deref().unwrap_or(ns);
                let target = ctx
                    .runtime()
                    .edgion_config_data_store
                    .get(Some(override_ns), &override_ref.name);
                let resolved = target
                    .as_ref()
                    .is_some_and(|t| matches!(&t.spec.data, ConfigEntry::RegionRouteOverride(_)));
                if !resolved {
                    errors.push(ResolvedRefsError::EdgionConfigDataNotFound {
                        namespace: override_ns.to_string(),
                        name: override_ref.name.clone(),
                        expected_type: "RegionRouteOverride".to_string(),
                    });
                }
            }
        }
    }
    errors
}

/// Check each `IpRestriction` plugin's `allow_refs`/`deny_refs` against the EdgionConfigData
/// store. Returns one `EdgionConfigDataNotFound { expected_type: "IpList" }` per missing or
/// wrong-type reference. `request_plugins` is a Vec (stable order), so no extra sort needed.
pub fn resolve_ip_list_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
    let Some(plugins) = &ep.spec.request_plugins else {
        return vec![];
    };
    let mut errors = Vec::new();
    for entry in plugins {
        if let EdgionPlugin::IpRestriction(ref config) = entry.plugin {
            for r in config
                .allow_refs
                .iter()
                .flatten()
                .chain(config.deny_refs.iter().flatten())
            {
                let rns = r.namespace.as_deref().unwrap_or(ns);
                let target = ctx.runtime().edgion_config_data_store.get(Some(rns), &r.name);
                let resolved = target.as_ref().is_some_and(|t| {
                    t.spec.enable
                        && matches!(&t.spec.data, ConfigEntry::IpList(d)
                            if edgion_resources::matcher::ip_cidr_helpers::build_grouped_matcher(&d.items).is_ok())
                });
                if !resolved {
                    errors.push(ResolvedRefsError::EdgionConfigDataNotFound {
                        namespace: rns.to_string(),
                        name: r.name.clone(),
                        expected_type: "IpList".to_string(),
                    });
                }
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::prelude_resources::EdgionConfigData;
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigDataRef, EdgionConfigDataSpec, RegionRouteOverride,
    };
    use edgion_resources::resources::edgion_plugins::{
        BasicAuthConfig, BasicAuthSecretGroup, CertSourceMode, EdgionPlugin, EdgionPlugins, EdgionPluginsSpec,
        HeaderCertAuthConfig, HmacAuthConfig, HmacAuthSecretGroup, JweDecryptConfig, OpenidConnectConfig,
        RegionRouteConfig, RequestFilterEntry,
    };
    use edgion_resources::resources::gateway::SecretObjectReference;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    fn make_ctx_with_secrets(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.secret_store.replace_all(secrets);
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
        (ctx, runtime)
    }

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

    #[tokio::test]
    async fn test_parse_resolves_basic_auth_users_and_registers_refs() {
        let alice = build_basic_auth_secret("default", "basic-auth-alice", "alice", "alice-password");
        let bob = build_basic_auth_secret("default", "basic-auth-bob", "bob", "bob-password");
        let mut secrets = HashMap::new();
        secrets.insert("default/basic-auth-alice".to_string(), alice);
        secrets.insert("default/basic-auth-bob".to_string(), bob);

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

        let mut ep = EdgionPlugins::new("basic-auth-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::BasicAuth(
            BasicAuthConfig {
                secret_groups: Some(vec![BasicAuthSecretGroup {
                    name: "default".to_string(),
                    description: None,
                    secret_refs: vec![
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
                    ],
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
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
        assert_eq!(
            resolved_users.get("alice").map(|v| v.password.as_str()),
            Some("alice-password")
        );
        assert_eq!(
            resolved_users.get("bob").map(|v| v.password.as_str()),
            Some("bob-password")
        );
        assert_eq!(ctx.secret_ref_manager().get_refs("default/basic-auth-alice").len(), 1);
        assert_eq!(ctx.secret_ref_manager().get_refs("default/basic-auth-bob").len(), 1);
    }

    #[tokio::test]
    async fn test_parse_resolves_openid_connect_secrets_and_registers_refs() {
        let client_secret = build_secret("default", "oidc-client", "clientSecret", "client-secret-value");
        let session_secret = build_secret("default", "oidc-session", "sessionSecret", "session-secret-value");
        let mut secrets = HashMap::new();
        secrets.insert("default/oidc-client".to_string(), client_secret);
        secrets.insert("default/oidc-session".to_string(), session_secret);

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

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

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
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
    }

    #[tokio::test]
    async fn test_parse_resolves_jwe_secret_and_registers_refs() {
        let jwe_secret = build_secret("default", "jwe-secret", "secret", "0123456789abcdef0123456789abcdef");
        let mut secrets = HashMap::new();
        secrets.insert("default/jwe-secret".to_string(), jwe_secret);

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

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

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
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
    }

    #[tokio::test]
    async fn test_parse_resolves_hmac_credentials_and_registers_refs() {
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

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

        let mut ep = EdgionPlugins::new("hmac-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::HmacAuth(HmacAuthConfig {
            secret_groups: Some(vec![HmacAuthSecretGroup {
                name: "default".to_string(),
                description: None,
                secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "hmac-credentials".to_string(),
                    namespace: None,
                }],
            }]),
            upstream_header_fields: vec!["X-Consumer-Username".to_string()],
            ..Default::default()
        }))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
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
    }

    #[tokio::test]
    async fn test_parse_resolves_header_cert_auth_ca_secrets_and_registers_refs() {
        let ca_secret = build_secret(
            "default",
            "header-cert-ca",
            "ca.crt",
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
        );
        let mut secrets = HashMap::new();
        secrets.insert("default/header-cert-ca".to_string(), ca_secret);

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

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

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
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
    }

    // ── RegionRoute + overrideRef resolution tests ────────────────────────

    /// Helper: build an EdgionConfigData with namespace and name set.
    fn build_edgion_config_data(namespace: &str, name: &str, spec: EdgionConfigDataSpec) -> EdgionConfigData {
        let mut ed = EdgionConfigData::new(name, spec);
        ed.metadata.namespace = Some(namespace.to_string());
        ed
    }

    #[tokio::test]
    async fn test_validate_region_route_override_ref_ok() {
        // RegionRouteOverride EdgionConfigData exists → resolve_override_refs returns no error
        let override_ed = build_edgion_config_data(
            "default",
            "rr-override",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                    enable: None,
                    active: None,
                    regions: None,
                }),
            },
        );

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/rr-override".to_string(), override_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RegionRoute(
            RegionRouteConfig {
                override_ref: Some(EdgionConfigDataRef {
                    name: "rr-override".to_string(),
                    namespace: None,
                }),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let ref_errors = super::resolve_override_refs(&parsed, &ctx);
        assert!(ref_errors.is_empty(), "expected no errors; got: {:?}", ref_errors);
    }

    #[tokio::test]
    async fn test_parse_registers_override_ref_for_requeue() {
        // Two RegionRoute plugins each with a different overrideRef.
        // Both must be registered in edgion_config_data_refs so that an update to
        // either EdgionConfigData triggers a requeue of this EdgionPlugins.
        let make_override_ed = |name: &str| {
            build_edgion_config_data(
                "default",
                name,
                EdgionConfigDataSpec {
                    enable: true,
                    active: None,
                    data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                        enable: None,
                        active: None,
                        regions: None,
                    }),
                },
            )
        };

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/override-a".to_string(), make_override_ed("override-a"));
        ed_map.insert("default/override-b".to_string(), make_override_ed("override-b"));
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("dual-rr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![
            RequestFilterEntry::new(EdgionPlugin::RegionRoute(RegionRouteConfig {
                override_ref: Some(EdgionConfigDataRef {
                    name: "override-a".to_string(),
                    namespace: None,
                }),
                ..Default::default()
            })),
            RequestFilterEntry::new(EdgionPlugin::RegionRoute(RegionRouteConfig {
                override_ref: Some(EdgionConfigDataRef {
                    name: "override-b".to_string(),
                    namespace: None,
                }),
                ..Default::default()
            })),
        ]);

        let handler = EdgionPluginsHandler::new();
        let _parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let ep_key = "EdgionPlugins:default/dual-rr-ep";
        let override_a_refs = runtime.edgion_config_data_refs.get_refs("default/override-a");
        let override_b_refs = runtime.edgion_config_data_refs.get_refs("default/override-b");
        assert!(
            override_a_refs.contains(&ep_key.to_string()),
            "override-a must reference ep; got: {:?}",
            override_a_refs
        );
        assert!(
            override_b_refs.contains(&ep_key.to_string()),
            "override-b must reference ep; got: {:?}",
            override_b_refs
        );
    }

    #[tokio::test]
    async fn test_validate_region_route_override_ref_missing() {
        // Empty EdgionConfigData store → resolve_override_refs() reports the missing reference.
        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RegionRoute(
            RegionRouteConfig {
                override_ref: Some(EdgionConfigDataRef {
                    name: "missing-override".to_string(),
                    namespace: None,
                }),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();

        // preparse() must NOT report missing EdgionConfigData as a structural error (it's a ref-resolution concern).
        let preparse_errors = handler.preparse(&mut ep, &ctx);
        assert!(
            preparse_errors.is_empty(),
            "preparse should not flag missing EdgionConfigData (handled by ResolvedRefs), got: {:?}",
            preparse_errors
        );

        // parse() must succeed — missing EdgionConfigData is a ref-resolution concern, not a parse error.
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        // resolve_override_refs() is the path that surfaces the missing EdgionConfigData.
        let ref_errors = super::resolve_override_refs(&parsed, &ctx);
        assert_eq!(ref_errors.len(), 1, "expected one EdgionConfigDataNotFound error");
        match &ref_errors[0] {
            crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::
                EdgionConfigDataNotFound { namespace, name, expected_type } => {
                assert_eq!(namespace, "default");
                assert_eq!(name, "missing-override");
                assert_eq!(expected_type, "RegionRouteOverride");
            }
            other => panic!("expected EdgionConfigDataNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_validate_region_route_override_ref_type_mismatch() {
        // EdgionConfigData exists but has wrong type → resolve_override_refs reports error.
        let wrong_type_ed = build_edgion_config_data(
            "default",
            "not-a-rr-override",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                data: ConfigEntry::KeyList(edgion_resources::resources::edgion_config_data::KeyListData {
                    match_mode: edgion_resources::resources::edgion_config_data::KeyMatchMode::default(),
                    items: vec![],
                }),
            },
        );

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/not-a-rr-override".to_string(), wrong_type_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RegionRoute(
            RegionRouteConfig {
                override_ref: Some(EdgionConfigDataRef {
                    name: "not-a-rr-override".to_string(),
                    namespace: None,
                }),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let ref_errors = super::resolve_override_refs(&parsed, &ctx);
        assert_eq!(
            ref_errors.len(),
            1,
            "expected one EdgionConfigDataNotFound for type mismatch"
        );
        match &ref_errors[0] {
            crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::
                EdgionConfigDataNotFound { expected_type, .. } => {
                assert_eq!(expected_type, "RegionRouteOverride");
            }
            other => panic!("expected EdgionConfigDataNotFound, got: {:?}", other),
        }
    }

    // ── IpRestriction + allowRefs/denyRefs resolution tests ─────────────────

    #[tokio::test]
    async fn ip_restriction_deny_refs_registered_and_resolved() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

        // Seed an IpList EdgionConfigData.
        let ip_ed = build_edgion_config_data(
            "default",
            "blocklist",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "g".into(),
                        description: None,
                        cidrs: vec!["9.9.9.9/32".into()],
                    }],
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/blocklist".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "blocklist".into(),
                    namespace: None,
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        // Registered for requeue.
        let refs = runtime.edgion_config_data_refs.get_refs("default/blocklist");
        assert!(
            refs.contains(&"EdgionPlugins:default/ipr-ep".to_string()),
            "got: {:?}",
            refs
        );
        // Resolves cleanly (IpList exists).
        assert!(super::resolve_ip_list_refs(&parsed, &ctx).is_empty());
    }

    #[tokio::test]
    async fn ip_restriction_missing_deny_ref_reports_iplist_error() {
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;
        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "missing".into(),
                    namespace: None,
                }]),
                ..Default::default()
            },
        ))]);
        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("skip: {}", reason),
        };
        let errors = super::resolve_ip_list_refs(&parsed, &ctx);
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::
                EdgionConfigDataNotFound { namespace, name, expected_type } => {
                assert_eq!(namespace, "default");
                assert_eq!(name, "missing");
                assert_eq!(expected_type, "IpList");
            }
            other => panic!("expected EdgionConfigDataNotFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn ip_restriction_disabled_deny_ref_reports_error() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

        // Seed a disabled (enable: false) IpList — should not count as resolved.
        let ip_ed = build_edgion_config_data(
            "default",
            "disabled-list",
            EdgionConfigDataSpec {
                enable: false,
                active: None,
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "g".into(),
                        description: None,
                        cidrs: vec!["1.2.3.4/32".into()],
                    }],
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/disabled-list".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "disabled-list".into(),
                    namespace: None,
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("skip: {}", reason),
        };
        let errors = super::resolve_ip_list_refs(&parsed, &ctx);
        assert_eq!(errors.len(), 1, "disabled IpList must not resolve; got: {:?}", errors);
        match &errors[0] {
            crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::
                EdgionConfigDataNotFound { expected_type, .. } => {
                assert_eq!(expected_type, "IpList");
            }
            other => panic!("expected EdgionConfigDataNotFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn ip_restriction_allow_ref_resolves() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

        // Seed an enabled, valid IpList — should resolve cleanly.
        let ip_ed = build_edgion_config_data(
            "default",
            "allowlist",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "g".into(),
                        description: None,
                        cidrs: vec!["192.168.0.0/24".into()],
                    }],
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/allowlist".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                allow_refs: Some(vec![EdgionConfigDataRef {
                    name: "allowlist".into(),
                    namespace: None,
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("skip: {}", reason),
        };
        // Enabled, valid IpList referenced via allow_refs -> resolves cleanly.
        assert!(
            super::resolve_ip_list_refs(&parsed, &ctx).is_empty(),
            "enabled valid IpList allow_ref must resolve without errors"
        );
    }

    #[tokio::test]
    async fn ip_restriction_enabled_malformed_ip_list_reports_error() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

        // Seed an enabled IpList whose CIDRs produce a post-normalization duplicate:
        // 10.0.0.0/8 and 10.0.0.1/8 both normalize to the same /8 network in the
        // radix builder. validate_groups_structure (exact-string) misses this;
        // build_grouped_matcher catches it. The IpList is enabled so the
        // enable-guard alone would not report an error — only the stricter predicate does.
        let ip_ed = build_edgion_config_data(
            "default",
            "malformed-list",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                data: ConfigEntry::IpList(IpListData {
                    items: vec![
                        IpGroup {
                            name: "a".into(),
                            description: None,
                            cidrs: vec!["10.0.0.0/8".into()],
                        },
                        IpGroup {
                            name: "b".into(),
                            description: None,
                            cidrs: vec!["10.0.0.1/8".into()],
                        },
                    ],
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/malformed-list".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "malformed-list".into(),
                    namespace: None,
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("skip: {}", reason),
        };
        let errors = super::resolve_ip_list_refs(&parsed, &ctx);
        assert_eq!(
            errors.len(),
            1,
            "enabled-but-uncompilable IpList must not resolve; got: {:?}",
            errors
        );
        match &errors[0] {
            crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::EdgionConfigDataNotFound {
                expected_type,
                ..
            } => {
                assert_eq!(expected_type, "IpList");
            }
            other => panic!("expected EdgionConfigDataNotFound, got {:?}", other),
        }
    }
}
