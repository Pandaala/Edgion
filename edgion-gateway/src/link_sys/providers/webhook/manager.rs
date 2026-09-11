//! WebhookManager — global webhook service registry.
//!
//! Manages webhook service connections resolved from LinkSys Webhook resources.
//! Keyed by "namespace/name" matching webhook_ref in the WebhookKeyGet plugin.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{Mutex, MutexGuard, RwLock};

use crate::plugins::http::common::outbound::call::{outbound_call, OutboundCallError, OutboundResponse};
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::http::common::outbound::request::OutboundRequestDiagnostics;
use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;

// ============================================================
// Global Manager
// ============================================================

static WEBHOOK_MANAGER: std::sync::LazyLock<WebhookManager> = std::sync::LazyLock::new(WebhookManager::new);

/// Global cap for webhook response body size. Initialized to 16KB.
static WEBHOOK_GLOBAL_MAX: AtomicUsize = AtomicUsize::new(16384);

/// Read the current global max response bytes for webhook calls.
pub fn get_webhook_global_max() -> usize {
    WEBHOOK_GLOBAL_MAX.load(Ordering::Relaxed)
}

/// Set the global max response bytes (called when EdgionGatewayConfig is updated).
pub fn configure_webhook_global_max(bytes: usize) {
    WEBHOOK_GLOBAL_MAX.store(bytes, Ordering::Relaxed);
    tracing::info!(bytes, "Webhook global max response bytes updated");
}

/// Get the global webhook manager
pub fn get_webhook_manager() -> &'static WebhookManager {
    &WEBHOOK_MANAGER
}

/// Global webhook manager, keyed by "namespace/name" matching webhook_ref.
pub(crate) struct WebhookRegistration {
    retired: AtomicBool,
}

impl WebhookRegistration {
    fn new() -> Self {
        Self {
            retired: AtomicBool::new(false),
        }
    }

    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }

    fn retire(&self) {
        self.retired.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
pub(crate) struct PreparedWebhookEntry {
    registration: Arc<WebhookRegistration>,
    client: Arc<ManagedOutboundClient>,
}

impl PreparedWebhookEntry {
    #[cfg(test)]
    pub(crate) fn registration(&self) -> &Arc<WebhookRegistration> {
        &self.registration
    }

    #[cfg(test)]
    pub(crate) fn client(&self) -> &Arc<ManagedOutboundClient> {
        &self.client
    }
}

struct WebhookEntry {
    registration: Arc<WebhookRegistration>,
    client: Arc<ManagedOutboundClient>,
}

struct WebhookMutationState {
    latest_accepted_sequence: u64,
}

pub struct WebhookManager {
    /// Registered webhook services: webhook_ref → managed client
    services: RwLock<HashMap<String, WebhookEntry>>,
    mutation: Mutex<WebhookMutationState>,
}

impl WebhookManager {
    fn new() -> Self {
        Self {
            services: RwLock::new(HashMap::new()),
            mutation: Mutex::new(WebhookMutationState {
                latest_accepted_sequence: 0,
            }),
        }
    }

    /// Look up a webhook by its ref ("namespace/name")
    pub async fn get(&self, webhook_ref: &str) -> Option<Arc<ManagedOutboundClient>> {
        self.services.read().await.get(webhook_ref).map(|e| e.client.clone())
    }

    pub(crate) async fn begin_mutation(&self, sequence: u64) -> Result<WebhookMutationGuard<'_>, StaleWebhookDispatch> {
        let mut state = self.mutation.lock().await;
        if sequence <= state.latest_accepted_sequence {
            return Err(StaleWebhookDispatch {
                sequence,
                latest_accepted_sequence: state.latest_accepted_sequence,
            });
        }
        state.latest_accepted_sequence = sequence;
        Ok(WebhookMutationGuard {
            manager: self,
            _state: state,
        })
    }
}

#[cfg(test)]
static NEXT_TEST_MUTATION_SEQUENCE: AtomicUsize = AtomicUsize::new(1);

#[cfg(test)]
impl WebhookManager {
    pub(crate) fn new_for_test() -> Self {
        Self::new()
    }

    fn next_test_sequence() -> u64 {
        NEXT_TEST_MUTATION_SEQUENCE.fetch_add(1, Ordering::Relaxed) as u64
    }

    async fn upsert(
        &self,
        webhook_ref: &str,
        config: WebhookServiceConfig,
        linksys_ns: Option<&str>,
    ) -> anyhow::Result<()> {
        let mutation = self
            .begin_mutation(Self::next_test_sequence())
            .await
            .map_err(|e| anyhow::anyhow!("stale test mutation: {e:?}"))?;
        let prepared = mutation.prepare(webhook_ref, config, linksys_ns)?;
        mutation.commit_one(webhook_ref, prepared).await
    }

    async fn remove(&self, webhook_ref: &str) {
        let mutation = self
            .begin_mutation(Self::next_test_sequence())
            .await
            .expect("test mutation sequences are strictly increasing");
        mutation.commit_remove(webhook_ref).await;
    }
}

// ============================================================
// Ordered mutation capability
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StaleWebhookDispatch {
    pub(crate) sequence: u64,
    pub(crate) latest_accepted_sequence: u64,
}

pub(crate) struct WebhookMutationGuard<'a> {
    manager: &'a WebhookManager,
    _state: MutexGuard<'a, WebhookMutationState>,
}

impl WebhookMutationGuard<'_> {
    pub(crate) fn prepare(
        &self,
        webhook_ref: &str,
        mut config: WebhookServiceConfig,
        linksys_ns: Option<&str>,
    ) -> anyhow::Result<PreparedWebhookEntry> {
        // The shared engine uses TLS block presence for client and Service
        // transport selection. Only active LinkSys policy may reach it.
        config.conn.tls = config.conn.tls.take().filter(|tls| tls.enabled);
        if config.conn.target.url.is_none() && config.conn.target.service.namespace.is_none() {
            if let Some(ns) = linksys_ns {
                config.conn.target.service.namespace = Some(ns.to_string());
            }
        }

        let managed_client = ManagedOutboundClient::from_parts(
            &config.conn,
            &config.request,
            config.resolved_secrets.as_ref(),
            webhook_ref,
            false,
        )
        .map_err(|e| anyhow::anyhow!("failed to register webhook '{}': {}", webhook_ref, e))?;

        Ok(PreparedWebhookEntry {
            registration: Arc::new(WebhookRegistration::new()),
            client: Arc::new(managed_client),
        })
    }

    pub(crate) async fn commit_one(&self, webhook_ref: &str, prepared: PreparedWebhookEntry) -> anyhow::Result<()> {
        let mut services = self.manager.services.write().await;
        if prepared.registration.is_retired() {
            anyhow::bail!("prepared webhook registration for '{webhook_ref}' is retired");
        }

        if let Some(old) = services.get(webhook_ref) {
            if !Arc::ptr_eq(&old.registration, &prepared.registration) {
                old.registration.retire();
            }
        }
        services.insert(
            webhook_ref.to_string(),
            WebhookEntry {
                registration: prepared.registration,
                client: prepared.client,
            },
        );
        tracing::info!(webhook = %webhook_ref, "Webhook registered/updated in manager");
        Ok(())
    }

    pub(crate) async fn commit_remove(&self, webhook_ref: &str) {
        let mut services = self.manager.services.write().await;
        if let Some(old) = services.remove(webhook_ref) {
            old.registration.retire();
        }
        tracing::info!(webhook = %webhook_ref, "Webhook removed from manager");
    }

    pub(crate) async fn commit_full(&self, prepared: HashMap<String, PreparedWebhookEntry>) -> anyhow::Result<()> {
        let new_services: HashMap<String, WebhookEntry> = prepared
            .into_iter()
            .map(|(key, entry)| {
                (
                    key,
                    WebhookEntry {
                        registration: entry.registration,
                        client: entry.client,
                    },
                )
            })
            .collect();

        let mut services = self.manager.services.write().await;
        if let Some((key, _)) = new_services.iter().find(|(_, entry)| entry.registration.is_retired()) {
            anyhow::bail!("prepared webhook registration for '{key}' is retired");
        }

        for (key, old) in services.iter() {
            let preserved = new_services
                .get(key)
                .is_some_and(|new| Arc::ptr_eq(&old.registration, &new.registration));
            if !preserved {
                old.registration.retire();
            }
        }
        *services = new_services;
        tracing::info!(count = services.len(), "Webhook manager full set committed");
        Ok(())
    }
}

// ============================================================
// Public call handle for plugins and gateway internals
// ============================================================

/// Handle for making outbound calls via a registered webhook connection.
pub struct WebhookCallHandle(pub(crate) Arc<ManagedOutboundClient>);

impl WebhookCallHandle {
    /// Execute an outbound HTTP call via this webhook connection.
    ///
    /// The outbound request shape is defined by the webhook resource; the caller
    /// only supplies the `session` (for origin / template lookups) and an
    /// optional `path_override` (honoured when the compiled path allows it).
    /// The caller must consume `diagnostics` into its canonical log after the
    /// call, including failures; use a fresh collector for each logical call.
    pub async fn call(
        &self,
        session: &dyn crate::plugins::runtime::PluginSession,
        path_override: Option<String>,
        diagnostics: &mut OutboundRequestDiagnostics,
    ) -> Result<OutboundResponse, OutboundCallError> {
        outbound_call(
            &self.0,
            session,
            path_override.as_deref(),
            get_webhook_global_max(),
            &[],
            diagnostics,
        )
        .await
    }
}

/// Get a handle for making generic webhook calls (used by plugins and gateway internals).
/// Returns None if no webhook with the given ref is registered.
pub async fn get_webhook_call_handle(webhook_ref: &str) -> Option<WebhookCallHandle> {
    get_webhook_manager().get(webhook_ref).await.map(WebhookCallHandle)
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_global_max_changes_value() {
        configure_webhook_global_max(65536);
        assert_eq!(get_webhook_global_max(), 65536);
        // reset to default for other tests
        configure_webhook_global_max(16384);
    }

    #[test]
    fn webhook_presence_diagnostics_follow_each_call_and_current_generation() {
        use crate::plugins::http::common::outbound::request::test_presence_failure_request;
        use crate::plugins::http::webhook_key_get::WebhookKeyGet;
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
        use crate::plugins::runtime::PluginLog;
        use edgion_resources::common::WebhookExtract;
        use edgion_resources::filters::PluginRunningResult;
        use edgion_resources::resources::common::outbound_request::PresencePolicy;
        use edgion_resources::resources::edgion_plugins::{WebhookKeyGetConfig, WebhookVar};

        // Acquire outside the runtime: no synchronous mutex is held across an
        // await inside an async task. Full-set dispatch tests share this lock.
        let _guard = crate::link_sys::runtime::store::I5_DISPATCH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let key = "presence-diagnostics/webhook";
                let plugin = WebhookKeyGet::create(&WebhookKeyGetConfig {
                    vars: [Some("fallback"), None]
                        .into_iter()
                        .enumerate()
                        .map(|(index, default)| WebhookVar {
                            name: format!("result{index}"),
                            webhook_ref: key.into(),
                            path_override: None,
                            extract: WebhookExtract::BodyText,
                            default: default.map(str::to_owned),
                        })
                        .collect(),
                });
                for policy in [
                    PresencePolicy::WarnIfMissing,
                    PresencePolicy::SkipIfMissing,
                    PresencePolicy::Required,
                ] {
                    let mut client = ManagedOutboundClient::test_with_static_target("http://example.test");
                    client.compiled_request = test_presence_failure_request(policy);
                    let client = Arc::new(client);
                    // Use one isolated registry key; replacement exercises the resolver's
                    // current-generation lookup without network or background health work.
                    get_webhook_manager().services.write().await.insert(
                        key.into(),
                        WebhookEntry {
                            registration: Arc::new(WebhookRegistration::new()),
                            client: client.clone(),
                        },
                    );
                    let mut session = MockPluginSession::new();
                    setup_header_mocks(&mut session, &[]);
                    session.expect_get_cookie().return_const(None::<String>);
                    session
                        .expect_set_ctx_var()
                        .withf(|name, value| name == "result0" && value == "fallback")
                        .times(1)
                        .returning(|_, _| Ok(()));
                    let mut log = PluginLog::new("WebhookKeyGet");
                    let result = plugin.run_request(&mut session, &mut log).await;
                    assert!(matches!(result, PluginRunningResult::GoodNext));
                    let rendered = log.render_for_test();
                    assert_eq!(
                        rendered.matches("warn outbound field missing").count(),
                        if matches!(policy, PresencePolicy::WarnIfMissing) {
                            2
                        } else {
                            0
                        }
                    );
                    assert!(rendered.contains("default"));
                    assert!(rendered.contains("skip"));
                    assert!(!rendered.contains("private"));

                    // Generic callers receive the same evidence even when the call fails.
                    let mut diagnostics = OutboundRequestDiagnostics::default();
                    assert!(WebhookCallHandle(client)
                        .call(&session, None, &mut diagnostics)
                        .await
                        .is_err());
                    let mut log = PluginLog::new("test");
                    diagnostics.emit(&mut log);
                    assert_eq!(
                        log.render_for_test(),
                        if matches!(policy, PresencePolicy::WarnIfMissing) {
                            "warn outbound field missing"
                        } else {
                            ""
                        }
                    );
                }
                get_webhook_manager().services.write().await.remove(key);
            });
    }

    #[tokio::test]
    async fn test_manager_upsert_and_get() {
        use edgion_resources::resources::link_sys::webhook::{
            ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
        };
        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    service: WebhookServiceRef {
                        kind: None,
                        name: Some("resolve-svc".to_string()),
                        namespace: Some("default".to_string()),
                        port: Some(8080),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        manager
            .upsert("test/webhook", config, None)
            .await
            .expect("upsert should succeed without TLS config");

        let client = manager.get("test/webhook").await;
        assert!(client.is_some());
        assert!(client.unwrap().health.healthy.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_manager_remove() {
        use edgion_resources::resources::link_sys::webhook::{
            ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
        };
        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    service: WebhookServiceRef {
                        kind: None,
                        name: Some("resolve-svc".to_string()),
                        namespace: Some("default".to_string()),
                        port: Some(8080),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        manager
            .upsert("test/webhook", config, None)
            .await
            .expect("upsert should succeed without TLS config");
        manager.remove("test/webhook").await;

        let client = manager.get("test/webhook").await;
        assert!(client.is_none());
    }

    #[tokio::test]
    async fn test_manager_service_cluster_ip_builds_dns_url() {
        use crate::plugins::http::common::outbound::target::ResolvedTarget;
        use edgion_resources::resources::link_sys::webhook::{
            ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
        };

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    service: WebhookServiceRef {
                        kind: Some("ServiceClusterIp".to_string()),
                        name: Some("auth-svc".to_string()),
                        namespace: Some("prod".to_string()),
                        port: Some(8080),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        manager
            .upsert("prod/auth", config, None)
            .await
            .expect("upsert should succeed");

        let client = manager.get("prod/auth").await.expect("client should exist");
        match &client.resolved_target {
            ResolvedTarget::Static(url) => {
                assert_eq!(url, "http://auth-svc.prod.svc.cluster.local:8080");
            }
            other => panic!("expected Static, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_manager_service_cluster_ip_uses_linksys_ns_fallback() {
        use crate::plugins::http::common::outbound::target::ResolvedTarget;
        use edgion_resources::resources::link_sys::webhook::{
            ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
        };

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    service: WebhookServiceRef {
                        kind: Some("ServiceClusterIp".to_string()),
                        name: Some("svc".to_string()),
                        namespace: None, // absent — should use linksys_ns
                        port: Some(9090),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        manager
            .upsert("ns/wh", config, Some("my-ns"))
            .await
            .expect("upsert should succeed");

        let client = manager.get("ns/wh").await.expect("client should exist");
        match &client.resolved_target {
            ResolvedTarget::Static(url) => {
                assert_eq!(url, "http://svc.my-ns.svc.cluster.local:9090");
            }
            other => panic!("expected Static, got {other:?}"),
        }
    }

    /// A URL target must not be given a namespace by the linksys_ns backfill —
    /// doing so would make `kind()` see a service field and reject the target.
    #[tokio::test]
    async fn upsert_url_target_does_not_backfill_namespace() {
        use edgion_resources::resources::link_sys::webhook::{ExternalHttpConnection, WebhookTarget};

        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    url: Some("https://api.example.com".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let mgr = WebhookManager::new();
        mgr.upsert("prod/url-hook", config, Some("prod"))
            .await
            .expect("url target must register");
        assert!(mgr.get("prod/url-hook").await.is_some());
    }

    #[tokio::test]
    async fn test_manager_unsupported_service_kind_returns_error() {
        use edgion_resources::resources::link_sys::webhook::{
            ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
        };

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    service: WebhookServiceRef {
                        kind: Some("ServiceImport".to_string()),
                        name: Some("svc".to_string()),
                        namespace: None,
                        port: Some(80),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let result = manager.upsert("ns/wh", config, None).await;
        assert!(result.is_err(), "ServiceImport should be rejected by manager");
    }

    #[tokio::test]
    async fn service_kind_none_builds_service_target() {
        use crate::plugins::http::common::outbound::target::ResolvedTarget;
        use edgion_resources::resources::link_sys::webhook::{
            ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
        };

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    service: WebhookServiceRef {
                        kind: None, // None → ServiceBackendKind::Service → ResolvedTarget::Service
                        name: Some("backend".to_string()),
                        namespace: Some("ns1".to_string()),
                        port: Some(8080),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        manager
            .upsert("ns1/backend", config, None)
            .await
            .expect("upsert with kind=None should succeed");

        let client = manager.get("ns1/backend").await.expect("client should exist");
        match &client.resolved_target {
            ResolvedTarget::Service {
                service_key,
                port,
                scheme,
                ..
            } => {
                assert_eq!(service_key, "ns1/backend");
                assert_eq!(*port, Some(8080));
                assert_eq!(*scheme, "http");
            }
            other => panic!("expected Service variant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn service_target_uses_linksys_ns_fallback() {
        use crate::plugins::http::common::outbound::target::ResolvedTarget;
        use edgion_resources::resources::link_sys::webhook::{
            ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
        };

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    service: WebhookServiceRef {
                        kind: None,
                        name: Some("backend".to_string()),
                        namespace: None, // absent — should fall back to linksys_ns
                        port: Some(8080),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        manager
            .upsert("fallback-ns/backend", config, Some("fallback-ns"))
            .await
            .expect("upsert with namespace=None and linksys_ns should succeed");

        let client = manager.get("fallback-ns/backend").await.expect("client should exist");
        match &client.resolved_target {
            ResolvedTarget::Service {
                service_key,
                port,
                scheme,
                ..
            } => {
                assert_eq!(service_key, "fallback-ns/backend");
                assert_eq!(*port, Some(8080));
                assert_eq!(*scheme, "http");
            }
            other => panic!("expected Service variant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_manager_webhook_with_shim_auth_succeeds() {
        use edgion_resources::resources::link_sys::webhook::{
            ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
        };

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    service: WebhookServiceRef {
                        kind: None,
                        name: Some("resolve-svc".to_string()),
                        namespace: Some("default".to_string()),
                        port: Some(8080),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            auth: None, // Deprecated shim field; not validated at runtime
            ..Default::default()
        };
        // Auth field is now a deprecated shim field that is not used at runtime.
        // Auth validation will be re-implemented in Wave B with the new request field.
        // This test verifies that webhook manager accepts configs with no auth.
        let result = manager.upsert("ns/wh", config, None).await;
        assert!(result.is_ok(), "webhook with no auth should be accepted by manager");
    }

    // ---- New tests for B3: compiled_request integration ----

    /// Helper: build a minimal valid WebhookServiceConfig with a default request spec.
    fn make_config_with_target(name: &str) -> WebhookServiceConfig {
        use edgion_resources::resources::link_sys::webhook::{
            ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
        };
        WebhookServiceConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    service: WebhookServiceRef {
                        kind: None,
                        name: Some(name.to_string()),
                        namespace: Some("default".to_string()),
                        port: Some(8080),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// upsert() stores a compiled_request on the client (default spec succeeds).
    #[tokio::test]
    async fn upsert_stores_compiled_request_on_runtime() {
        let manager = WebhookManager::new();
        manager
            .upsert("ns/wh-compiled", make_config_with_target("svc"), None)
            .await
            .expect("upsert with default request should succeed");

        let client = manager.get("ns/wh-compiled").await.expect("client must exist");
        // The default spec has no path, method, args, headers, or cookies — all None/empty.
        // Verify the compiled_request is present by accessing it (no panic).
        let _ = &client.compiled_request;
    }

    /// upsert() with an invalid template in request.path returns Err; nothing is registered.
    #[tokio::test]
    async fn upsert_rejects_invalid_template_in_request() {
        use edgion_resources::resources::link_sys::webhook::{PathSpec, WebhookOutboundRequest};

        let manager = WebhookManager::new();
        let mut config = make_config_with_target("svc");
        // Unclosed `${` is a compile-time error in CompiledTemplate::compile.
        config.request = WebhookOutboundRequest {
            path: Some(PathSpec {
                template: "/api/${ctx:missing_close".to_string(), // unclosed template variable
                allow_override: false,
            }),
            ..WebhookOutboundRequest::default()
        };

        let result = manager.upsert("ns/bad-template", config, None).await;
        assert!(result.is_err(), "upsert with unclosed template must return Err");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unclosed") || err.contains("compile"),
            "error should mention template compile failure, got: {err}"
        );

        // No client must have been registered.
        assert!(
            manager.get("ns/bad-template").await.is_none(),
            "no client should be registered after a compile error"
        );
    }

    /// Calling upsert() a second time with a changed request resets health state.
    ///
    /// This verifies that health counters are not carried over across config changes:
    /// a new `ManagedOutboundClient` (and thus fresh `HealthState` atomics) is
    /// created on every call to `upsert()`.
    #[tokio::test]
    async fn upsert_resets_health_on_request_change() {
        use edgion_resources::resources::link_sys::webhook::{MethodSpec, WebhookOutboundRequest};

        let manager = WebhookManager::new();

        // Initial registration.
        manager
            .upsert("ns/health-reset", make_config_with_target("svc"), None)
            .await
            .expect("initial upsert should succeed");

        // Simulate degraded health on the first client.
        let client1 = manager.get("ns/health-reset").await.expect("client must exist");
        client1.health.healthy.store(false, Ordering::Relaxed);
        client1.health.passive_failures.store(5, Ordering::Relaxed);
        let ptr1 = Arc::as_ptr(&client1);

        // Re-upsert with a changed request spec.
        let mut config2 = make_config_with_target("svc");
        config2.request = WebhookOutboundRequest {
            method: Some(MethodSpec {
                template: "POST".to_string(),
            }),
            ..WebhookOutboundRequest::default()
        };
        manager
            .upsert("ns/health-reset", config2, None)
            .await
            .expect("second upsert should succeed");

        let client2 = manager.get("ns/health-reset").await.expect("client must exist");

        // A brand-new Arc is stored: the old client has been replaced.
        assert_ne!(Arc::as_ptr(&client2), ptr1, "upsert must replace the client Arc");
        // Health is reset on the new client.
        assert!(
            client2.health.healthy.load(Ordering::Relaxed),
            "healthy must be reset to true on re-upsert"
        );
        assert_eq!(
            client2.health.passive_failures.load(Ordering::Relaxed),
            0,
            "passive_failures must be reset to 0 on re-upsert"
        );
    }

    /// EndpointSlice churn does NOT call upsert(), so health state is preserved.
    ///
    /// Regression test: the `ResolvedTarget::Service` variant reads the
    /// global EpSliceStore at call time. Endpoint add/remove/re-add must never
    /// rebuild the `ManagedOutboundClient` or reset its health atomics.
    #[tokio::test]
    async fn endpoint_churn_preserves_health_state() {
        use crate::backends::discovery::endpoint_slice::get_roundrobin_store;
        use edgion_resources::constants::labels::k8s::SERVICE_NAME;
        use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
        use std::collections::{BTreeMap, HashMap, HashSet};

        let manager = WebhookManager::new();
        let config = make_config_with_target("ep-churn-svc");
        manager
            .upsert("ns/ep-churn", config, None)
            .await
            .expect("upsert should succeed");

        // Record the Arc identity and degrade the client health.
        let client = manager.get("ns/ep-churn").await.expect("client must exist");
        let ptr_before = Arc::as_ptr(&client);
        client.health.healthy.store(false, Ordering::Relaxed);
        client.health.passive_failures.store(3, Ordering::Relaxed);

        // Simulate endpoint churn by updating the EpSliceStore directly (bypassing upsert).
        let store = get_roundrobin_store();
        let mut labels = BTreeMap::new();
        labels.insert(SERVICE_NAME.to_string(), "ep-churn-svc".to_string());
        let ep_slice = EndpointSlice {
            metadata: ObjectMeta {
                name: Some("ep-churn-slice".to_string()),
                namespace: Some("default".to_string()),
                labels: Some(labels),
                ..Default::default()
            },
            address_type: "IPv4".to_string(),
            endpoints: vec![Endpoint {
                addresses: vec!["10.0.99.1".to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ports: Some(vec![EndpointPort {
                port: Some(8080),
                ..Default::default()
            }]),
        };
        let slice_key = "default/ep-churn-svc-slice";
        store.update_data_only(
            HashMap::from([(slice_key.to_string(), ep_slice)]),
            HashMap::new(),
            &HashSet::new(),
        );

        // The client fetched from the manager must be the SAME Arc — upsert was not called.
        let client_after = manager.get("ns/ep-churn").await.expect("client must exist");
        assert_eq!(
            Arc::as_ptr(&client_after),
            ptr_before,
            "endpoint churn must not replace the client Arc"
        );
        assert!(
            !client_after.health.healthy.load(Ordering::Relaxed),
            "healthy must remain false after endpoint churn"
        );
        assert_eq!(
            client_after.health.passive_failures.load(Ordering::Relaxed),
            3,
            "passive_failures must remain 3 after endpoint churn"
        );

        // Cleanup.
        store.update_data_only(HashMap::new(), HashMap::new(), &HashSet::from([slice_key.to_string()]));
    }

    fn config_with_port(port: i32) -> WebhookServiceConfig {
        let mut config = make_config_with_target("ordered-svc");
        config.conn.target.service.port = Some(port);
        config
    }

    #[tokio::test]
    async fn disabled_tls_preparation_ignores_material_and_preserves_target_scheme() {
        use crate::plugins::http::common::outbound::target::ResolvedTarget;
        use edgion_resources::resources::common::WebhookTarget;
        use serde_json::json;

        let manager = WebhookManager::new();
        let mutation = manager.begin_mutation(1).await.unwrap();
        for url in [None, Some("http://example.com"), Some("https://example.com")] {
            for injected in [false, true] {
                let mut config = config_with_port(8080);
                if let Some(url) = url {
                    config.conn.target = WebhookTarget {
                        url: Some(url.to_string()),
                        ..Default::default()
                    };
                }
                let mut tls = json!({
                    "enabled": false,
                    "verify": true,
                    "validation": {"caCertificateRefs": [{"kind": "Secret", "name": "missing-ca"}]},
                    "clientCertificateRef": {"name": "missing-client"}
                });
                if injected {
                    tls["resolvedCaCertificates"] = json!([{}]);
                    tls["resolvedClientCertificate"] = json!({});
                }
                for enabled in [false, true, false] {
                    tls["enabled"] = json!(enabled);
                    config.conn.tls = Some(serde_json::from_value(tls.clone()).unwrap());
                    let result = mutation.prepare("default/tls-toggle", config.clone(), None);
                    if enabled {
                        assert!(result.is_err(), "enabled invalid TLS must not prepare: {url:?}");
                        continue;
                    }
                    let prepared = result.expect("disabled TLS must not block preparation");
                    match (&prepared.client.resolved_target, url) {
                        (ResolvedTarget::Service { scheme, .. }, None) => assert_eq!(*scheme, "http"),
                        (ResolvedTarget::Static(actual), Some(expected)) => assert_eq!(actual, expected),
                        (actual, expected) => panic!("unexpected target {actual:?} for {expected:?}"),
                    }
                }
            }
        }
    }

    async fn registered_entry(
        manager: &WebhookManager,
        key: &str,
    ) -> Option<(Arc<WebhookRegistration>, Arc<ManagedOutboundClient>)> {
        manager
            .services
            .read()
            .await
            .get(key)
            .map(|entry| (entry.registration.clone(), entry.client.clone()))
    }

    fn assert_service_port(client: &ManagedOutboundClient, expected: i32) {
        use crate::plugins::http::common::outbound::target::ResolvedTarget;

        match &client.resolved_target {
            ResolvedTarget::Service { port, .. } => assert_eq!(*port, Some(expected)),
            other => panic!("expected Service target, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn duplicate_sequence_is_rejected_after_guard_release() {
        let manager = WebhookManager::new();
        let accepted = manager.begin_mutation(7).await.expect("sequence is accepted once");
        drop(accepted);

        let stale = match manager.begin_mutation(7).await {
            Ok(_) => panic!("equal sequence must be stale"),
            Err(stale) => stale,
        };
        assert_eq!(stale.latest_accepted_sequence, 7);
    }

    #[tokio::test]
    async fn full_commit_replaces_the_registry_as_one_snapshot() {
        let manager = WebhookManager::new();
        let first = manager.begin_mutation(1).await.expect("first sequence is accepted");
        let old_a = first
            .prepare("ns/old-a", config_with_port(8001), None)
            .expect("old-a prepares");
        let old_b = first
            .prepare("ns/old-b", config_with_port(8002), None)
            .expect("old-b prepares");
        let old_a_registration = old_a.registration.clone();
        let old_b_registration = old_b.registration.clone();
        first
            .commit_full(HashMap::from([
                ("ns/old-a".to_string(), old_a),
                ("ns/old-b".to_string(), old_b),
            ]))
            .await
            .expect("initial full set commits");
        drop(first);

        let second = manager.begin_mutation(2).await.expect("second sequence is accepted");
        let new_a = second
            .prepare("ns/new-a", config_with_port(9001), None)
            .expect("new-a prepares");
        let new_b = second
            .prepare("ns/new-b", config_with_port(9002), None)
            .expect("new-b prepares");
        second
            .commit_full(HashMap::from([
                ("ns/new-a".to_string(), new_a),
                ("ns/new-b".to_string(), new_b),
            ]))
            .await
            .expect("replacement full set commits");

        let services = manager.services.read().await;
        assert_eq!(services.len(), 2);
        assert!(services.contains_key("ns/new-a"));
        assert!(services.contains_key("ns/new-b"));
        assert!(old_a_registration.is_retired());
        assert!(old_b_registration.is_retired());
    }

    #[tokio::test]
    async fn full_commit_carries_over_the_exact_registration() {
        let manager = WebhookManager::new();
        let first = manager.begin_mutation(1).await.expect("first sequence is accepted");
        let prepared = first
            .prepare("ns/stable", config_with_port(8080), None)
            .expect("stable webhook prepares");
        let carried = prepared.clone();
        let registration = prepared.registration.clone();
        let client = prepared.client.clone();
        first
            .commit_one("ns/stable", prepared)
            .await
            .expect("stable webhook commits");
        drop(first);

        let second = manager.begin_mutation(2).await.expect("second sequence is accepted");
        second
            .commit_full(HashMap::from([("ns/stable".to_string(), carried)]))
            .await
            .expect("carry-over full set commits");

        let (published_registration, published_client) = registered_entry(&manager, "ns/stable")
            .await
            .expect("carried webhook remains registered");
        assert!(Arc::ptr_eq(&registration, &published_registration));
        assert!(Arc::ptr_eq(&client, &published_client));
        assert!(!registration.is_retired());
    }

    #[tokio::test]
    async fn full_commit_prevalidates_every_entry_before_replacing_snapshot() {
        let manager = WebhookManager::new();
        let first = manager.begin_mutation(1).await.expect("first sequence is accepted");
        let stable = first
            .prepare("ns/stable", config_with_port(8080), None)
            .expect("stable webhook prepares");
        first
            .commit_one("ns/stable", stable)
            .await
            .expect("stable webhook commits");
        drop(first);

        let second = manager.begin_mutation(2).await.expect("second sequence is accepted");
        let candidate = second
            .prepare("ns/replaced", config_with_port(8081), None)
            .expect("candidate webhook prepares");
        let delayed_candidate = candidate.clone();
        second
            .commit_one("ns/replaced", candidate)
            .await
            .expect("candidate webhook commits");
        drop(second);

        let third = manager.begin_mutation(3).await.expect("third sequence is accepted");
        let replacement = third
            .prepare("ns/replaced", config_with_port(8082), None)
            .expect("replacement webhook prepares");
        let replacement_client = replacement.client.clone();
        third
            .commit_one("ns/replaced", replacement)
            .await
            .expect("replacement webhook commits");
        drop(third);
        assert!(delayed_candidate.registration.is_retired());

        let fourth = manager.begin_mutation(4).await.expect("fourth sequence is accepted");
        let valid = fourth
            .prepare("ns/new", config_with_port(9090), None)
            .expect("new webhook prepares");
        assert!(
            fourth
                .commit_full(HashMap::from([
                    ("ns/replaced".to_string(), delayed_candidate),
                    ("ns/new".to_string(), valid),
                ]))
                .await
                .is_err(),
            "one retired entry rejects the whole full-set candidate"
        );

        assert!(registered_entry(&manager, "ns/stable").await.is_some());
        let (_, still_replacement) = registered_entry(&manager, "ns/replaced")
            .await
            .expect("current replacement remains registered");
        assert!(Arc::ptr_eq(&replacement_client, &still_replacement));
        assert!(registered_entry(&manager, "ns/new").await.is_none());
    }

    #[tokio::test]
    async fn prepare_failure_preserves_live_entry_and_advances_sequence() {
        use edgion_resources::resources::link_sys::webhook::{PathSpec, WebhookOutboundRequest};

        let manager = WebhookManager::new();
        let first = manager.begin_mutation(1).await.expect("first sequence is accepted");
        let prepared = first
            .prepare("ns/stable", config_with_port(8080), None)
            .expect("stable webhook prepares");
        first
            .commit_one("ns/stable", prepared)
            .await
            .expect("stable webhook commits");
        drop(first);
        let before = registered_entry(&manager, "ns/stable")
            .await
            .expect("stable webhook is live");

        let failed = manager.begin_mutation(2).await.expect("failing sequence is accepted");
        let mut invalid = config_with_port(9090);
        invalid.request = WebhookOutboundRequest {
            path: Some(PathSpec {
                template: "/${ctx:unclosed".to_string(),
                allow_override: false,
            }),
            ..Default::default()
        };
        assert!(failed.prepare("ns/stable", invalid.clone(), None).is_err());
        assert!(failed.prepare("ns/cold", invalid, None).is_err());
        drop(failed);

        let after = registered_entry(&manager, "ns/stable")
            .await
            .expect("failed replacement preserves the live webhook");
        assert!(Arc::ptr_eq(&before.0, &after.0));
        assert!(Arc::ptr_eq(&before.1, &after.1));
        assert!(registered_entry(&manager, "ns/cold").await.is_none());
        assert!(
            manager.begin_mutation(2).await.is_err(),
            "failed preparation does not roll sequence back"
        );
        assert!(manager.begin_mutation(3).await.is_ok(), "next sequence remains usable");
    }

    #[tokio::test]
    async fn omission_retires_old_registration_and_blocks_delayed_republication() {
        let manager = WebhookManager::new();
        let first = manager.begin_mutation(1).await.expect("first sequence is accepted");
        let prepared = first
            .prepare("ns/recreated", config_with_port(8081), None)
            .expect("initial webhook prepares");
        let delayed_old = prepared.clone();
        let old_registration = prepared.registration.clone();
        first
            .commit_one("ns/recreated", prepared)
            .await
            .expect("initial webhook commits");
        drop(first);

        let omitted = manager.begin_mutation(2).await.expect("omission sequence is accepted");
        omitted
            .commit_full(HashMap::new())
            .await
            .expect("empty authoritative full set commits");
        drop(omitted);
        assert!(old_registration.is_retired());
        assert!(registered_entry(&manager, "ns/recreated").await.is_none());

        let recreated = manager.begin_mutation(3).await.expect("recreate sequence is accepted");
        let replacement = recreated
            .prepare("ns/recreated", config_with_port(8083), None)
            .expect("replacement webhook prepares");
        recreated
            .commit_one("ns/recreated", replacement)
            .await
            .expect("replacement webhook commits");
        drop(recreated);

        let delayed = manager.begin_mutation(4).await.expect("later guard is accepted");
        assert!(
            delayed.commit_one("ns/recreated", delayed_old).await.is_err(),
            "a retired registration cannot erase a recreated webhook"
        );
        let (_, client) = registered_entry(&manager, "ns/recreated")
            .await
            .expect("replacement remains callable");
        assert_service_port(&client, 8083);
    }

    #[tokio::test]
    async fn partial_remove_retires_and_deletes_registration() {
        let manager = WebhookManager::new();
        let first = manager.begin_mutation(1).await.expect("first sequence is accepted");
        let prepared = first
            .prepare("ns/deleted", config_with_port(8080), None)
            .expect("webhook prepares");
        let registration = prepared.registration.clone();
        first.commit_one("ns/deleted", prepared).await.expect("webhook commits");
        drop(first);

        let removed = manager.begin_mutation(2).await.expect("remove sequence is accepted");
        removed.commit_remove("ns/deleted").await;

        assert!(registration.is_retired());
        assert!(registered_entry(&manager, "ns/deleted").await.is_none());
    }
}
