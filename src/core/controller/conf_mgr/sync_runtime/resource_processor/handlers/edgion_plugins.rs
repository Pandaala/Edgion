//! EdgionPlugins Handler
//!
//! Handles EdgionPlugins resources with Gateway API standard status management.
//! Delegates per-plugin Secret resolution to `plugins::resolve_credential`.
//!
//! Features:
//! - parse: Resolve Secret references and register to SecretRefManager for cascading updates
//! - on_delete: Clear SecretRefManager references

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::region_route_table::RefPluginEntry;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, pick_resolved_refs_reason, update_condition, ResolvedRefsError,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    condition_types, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::types::prelude_resources::EdgionPlugins;
use crate::types::resources::edgion_plugins::{EdgionPlugin, EdgionPluginsStatus};
use crate::types::resources::plugin_metadata::MetaDataEntry;
use crate::types::ResourceKind;

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
// RegionRoute + PluginMetaData helpers
// ============================================================================

impl EdgionPluginsHandler {
    /// Clear all PluginMetaData refs and service table ref_plugins for this EdgionPlugins.
    fn clear_region_route_refs(ep: &EdgionPlugins, ctx: &HandlerContext) {
        let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
        let ep_name = ep.metadata.name.as_deref().unwrap_or("");
        let ep_key = format!("{}:{}/{}", ResourceKind::EdgionPlugins.as_str(), ns, ep_name);
        ctx.runtime().plugin_metadata_refs.clear_refs_from(&ep_key);
        ctx.runtime().service_region_route_table.clear_ref_plugins_from(
            ResourceKind::EdgionPlugins.as_str(),
            ns,
            ep_name,
        );
    }

    /// Extract all `service_region_route_ref` names from every RegionRoute plugin.
    fn get_service_region_route_refs(ep: &EdgionPlugins) -> Vec<String> {
        let Some(plugins) = &ep.spec.request_plugins else {
            return vec![];
        };
        plugins
            .iter()
            .filter_map(|e| {
                if let EdgionPlugin::RegionRoute(ref config) = e.plugin {
                    config.service_region_route_ref.clone()
                } else {
                    None
                }
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionPlugins> for EdgionPluginsHandler {
    fn preparse(&self, ep: &mut EdgionPlugins, _ctx: &HandlerContext) -> Vec<String> {
        ep.preparse();
        ep.get_preparse_errors().to_vec()
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

        Self::clear_region_route_refs(&ep, ctx);

        let ns = ep.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let ep_name = ep.metadata.name.as_deref().unwrap_or("").to_string();
        let ep_key = format!("{}:{}/{}", ResourceKind::EdgionPlugins.as_str(), ns, ep_name);

        // Register PluginMetaData refs and service table entries for each RegionRoute plugin
        for pm_name in Self::get_service_region_route_refs(&ep) {
            let pm_key = format!("{}/{}", ns, pm_name);
            ctx.runtime()
                .plugin_metadata_refs
                .add_ref(pm_key.clone(), ep_key.clone());
            ctx.runtime().service_region_route_table.add_ref_plugin(
                &pm_key,
                RefPluginEntry {
                    kind: ResourceKind::EdgionPlugins.as_str().to_string(),
                    namespace: ns.clone(),
                    name: ep_name.clone(),
                },
            );

            // Also register ref to the ClusterRegionRoute PluginMetaData
            // (via ServiceRegionRoute PluginMetaData's clusterRef) for requeue
            if let Some(svc_entry) = ctx.runtime().service_region_route_table.get(&pm_key) {
                if let Some(ref cref) = svc_entry.cluster_pm_ref {
                    let c_ns = cref.namespace.as_deref().unwrap_or(&ns);
                    let cluster_plugin_metadata_key = format!("{}/{}", c_ns, cref.name);
                    ctx.runtime()
                        .plugin_metadata_refs
                        .add_ref(cluster_plugin_metadata_key, ep_key.clone());
                }
            }
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
        Self::clear_region_route_refs(ep, ctx);

        tracing::debug!(
            edgion_plugins = %resource_ref.key(),
            "Cleared secret and PluginMetaData references on EdgionPlugins delete"
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
        resolved_errors.extend(resolve_pm_refs(ep, ctx));
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

/// Check each RegionRoute plugin's `serviceRegionRouteRef` against the PluginMetaData store.
/// Returns one `PluginMetaDataNotFound` error per missing or wrong-type reference.
fn resolve_pm_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
    let Some(plugins) = &ep.spec.request_plugins else {
        return vec![];
    };
    let mut errors = Vec::new();
    for entry in plugins {
        if let EdgionPlugin::RegionRoute(ref config) = entry.plugin {
            if let Some(ref svc_name) = config.service_region_route_ref {
                let target = ctx.runtime().plugin_metadata_store.get(Some(ns), svc_name);
                let resolved = target
                    .as_ref()
                    .is_some_and(|t| matches!(&t.spec.metadata, MetaDataEntry::ServiceRegionRoute(_)));
                if !resolved {
                    errors.push(ResolvedRefsError::PluginMetaDataNotFound {
                        namespace: ns.to_string(),
                        name: svc_name.clone(),
                        expected_type: "ServiceRegionRoute".to_string(),
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
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
        HandlerContext, ProcessResult, ProcessorHandler, SecretRefManager,
    };
    use crate::core::controller::conf_mgr::ControllerRuntime;
    use crate::types::resources::edgion_plugins::plugin_configs::PluginMetaDataRef;
    use crate::types::resources::edgion_plugins::{
        BasicAuthConfig, CertSourceMode, EdgionPlugin, EdgionPlugins, EdgionPluginsSpec, HeaderCertAuthConfig,
        HmacAuthConfig, JweDecryptConfig, OpenidConnectConfig, RegionRouteConfig, RequestFilterEntry,
    };
    use crate::types::resources::gateway::SecretObjectReference;
    use crate::types::resources::plugin_metadata::{
        MetaDataEntry, PluginMetaData, PluginMetaDataSpec, ServiceRegionRouteConfig,
    };
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    fn make_ctx_with_secrets(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.secret_store.replace_all(secrets);
        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let ctx = HandlerContext::new(
            secret_ref_manager,
            None,
            None,
            Default::default(),
            5,
            20,
            runtime.clone(),
        );
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
        assert_eq!(resolved_users.get("alice").map(|v| v.as_str()), Some("alice-password"));
        assert_eq!(resolved_users.get("bob").map(|v| v.as_str()), Some("bob-password"));
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
            secret_refs: Some(vec![SecretObjectReference {
                group: None,
                kind: None,
                name: "hmac-credentials".to_string(),
                namespace: None,
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

    // ── RegionRoute + PluginMetaData resolution tests ────────────────────────

    /// Helper: build a PluginMetaData with namespace and name set.
    fn build_plugin_metadata(namespace: &str, name: &str, spec: PluginMetaDataSpec) -> PluginMetaData {
        let mut pm = PluginMetaData::new(name, spec);
        pm.metadata.namespace = Some(namespace.to_string());
        pm
    }

    #[tokio::test]
    async fn test_validate_region_route_refs_ok() {
        // ServiceRegionRoute PluginMetaData exists → no validation_error
        let service_pm = build_plugin_metadata(
            "default",
            "svc-rr",
            PluginMetaDataSpec {
                metadata: MetaDataEntry::ServiceRegionRoute(ServiceRegionRouteConfig {
                    cluster_ref: Some(PluginMetaDataRef {
                        name: "global-rr".to_string(),
                        namespace: None,
                    }),
                    regions: vec![],
                }),
            },
        );

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut pm_map = HashMap::new();
        pm_map.insert("default/svc-rr".to_string(), service_pm);
        runtime.plugin_metadata_store.replace_all(pm_map);

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RegionRoute(
            RegionRouteConfig {
                service_region_route_ref: Some("svc-rr".to_string()),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let _config = match &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::RegionRoute(c) => c,
            _ => panic!("unexpected plugin type"),
        };
    }

    #[tokio::test]
    async fn test_parse_registers_all_region_route_pm_refs() {
        // Two RegionRoute plugins referencing different PluginMetaData resources.
        // Both must be registered in plugin_metadata_refs so that an update to
        // either one triggers a requeue of this EdgionPlugins.
        let make_svc_pm = |name: &str| {
            build_plugin_metadata(
                "default",
                name,
                PluginMetaDataSpec {
                    metadata: MetaDataEntry::ServiceRegionRoute(ServiceRegionRouteConfig {
                        cluster_ref: None,
                        regions: vec![],
                    }),
                },
            )
        };

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut pm_map = HashMap::new();
        pm_map.insert("default/svc-a".to_string(), make_svc_pm("svc-a"));
        pm_map.insert("default/svc-b".to_string(), make_svc_pm("svc-b"));
        runtime.plugin_metadata_store.replace_all(pm_map);

        let mut ep = EdgionPlugins::new("dual-rr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![
            RequestFilterEntry::new(EdgionPlugin::RegionRoute(RegionRouteConfig {
                service_region_route_ref: Some("svc-a".to_string()),
                ..Default::default()
            })),
            RequestFilterEntry::new(EdgionPlugin::RegionRoute(RegionRouteConfig {
                service_region_route_ref: Some("svc-b".to_string()),
                ..Default::default()
            })),
        ]);

        let handler = EdgionPluginsHandler::new();
        let _parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let ep_key = "EdgionPlugins:default/dual-rr-ep";
        let svc_a_refs = runtime.plugin_metadata_refs.get_refs("default/svc-a");
        let svc_b_refs = runtime.plugin_metadata_refs.get_refs("default/svc-b");
        assert!(
            svc_a_refs.contains(&ep_key.to_string()),
            "svc-a must reference ep; got: {:?}",
            svc_a_refs
        );
        assert!(
            svc_b_refs.contains(&ep_key.to_string()),
            "svc-b must reference ep; got: {:?}",
            svc_b_refs
        );

        // Assert service_region_route_table.ref_plugins also got both EP entries.
        // The fix loop touches both data structures together; divergence between them
        // would silently regress, so both must be asserted.
        use crate::core::controller::conf_mgr::sync_runtime::resource_processor::region_route_table::RefPluginEntry;
        let expected = RefPluginEntry {
            kind: "EdgionPlugins".to_string(),
            namespace: "default".to_string(),
            name: "dual-rr-ep".to_string(),
        };
        let svc_a_entry = runtime
            .service_region_route_table
            .get("default/svc-a")
            .expect("svc-a entry must exist");
        let svc_b_entry = runtime
            .service_region_route_table
            .get("default/svc-b")
            .expect("svc-b entry must exist");
        assert!(
            svc_a_entry.ref_plugins.contains(&expected),
            "svc-a service table must list ep as ref_plugin; got: {:?}",
            svc_a_entry.ref_plugins
        );
        assert!(
            svc_b_entry.ref_plugins.contains(&expected),
            "svc-b service table must list ep as ref_plugin; got: {:?}",
            svc_b_entry.ref_plugins
        );
    }

    #[tokio::test]
    async fn test_validate_region_route_refs_missing() {
        // Empty PluginMetaData store → resolve_pm_refs() reports the missing reference
        // as a ResolvedRefsError::PluginMetaDataNotFound. Preparse no longer touches
        // PluginMetaData refs (they aren't structural validation of the EP itself —
        // they're a ref-resolution concern, surfaced via the ResolvedRefs condition).
        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RegionRoute(
            RegionRouteConfig {
                service_region_route_ref: Some("missing-pm".to_string()),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();

        // preparse() should NOT report missing PM as a structural error
        let preparse_errors = handler.preparse(&mut ep, &ctx);
        assert!(
            preparse_errors.is_empty(),
            "preparse should not flag missing PM (handled by ResolvedRefs), got: {:?}",
            preparse_errors
        );

        // parse() must succeed — missing PM is a ref-resolution concern, not a parse error.
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        // resolve_pm_refs() is the path that surfaces the missing PluginMetaData.
        let ref_errors = super::resolve_pm_refs(&parsed, &ctx);
        assert_eq!(ref_errors.len(), 1, "expected one PluginMetaDataNotFound error");
        match &ref_errors[0] {
            crate::core::controller::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::
                PluginMetaDataNotFound { namespace, name, expected_type } => {
                assert_eq!(namespace, "default");
                assert_eq!(name, "missing-pm");
                assert_eq!(expected_type, "ServiceRegionRoute");
            }
            other => panic!("expected PluginMetaDataNotFound, got: {:?}", other),
        }
    }
}
