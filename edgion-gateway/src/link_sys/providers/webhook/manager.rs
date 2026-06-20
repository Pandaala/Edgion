//! WebhookManager — global webhook service registry.
//!
//! Manages webhook service connections resolved from LinkSys Webhook resources.
//! Keyed by "namespace/name" matching webhook_ref in KeyGet::Webhook.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::backends::service_kind::EdgionService;
use crate::plugins::http::common::http_client::{build_webhook_client, get_http_client_arc};
use edgion_resources::resources::link_sys::webhook::{WebhookAuth, WebhookServiceConfig, WebhookTarget};

use super::health::spawn_active_health_check;
use super::runtime::{SlidingWindowCounter, WebhookResolvedTarget, WebhookRuntime};

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
struct WebhookEntry {
    runtime: Arc<WebhookRuntime>,
    health_task: Option<tokio::task::JoinHandle<()>>,
}

pub struct WebhookManager {
    /// Registered webhook services: webhook_ref → runtime state
    services: RwLock<HashMap<String, WebhookEntry>>,
}

impl WebhookManager {
    fn new() -> Self {
        Self {
            services: RwLock::new(HashMap::new()),
        }
    }

    /// Look up a webhook by its ref ("namespace/name")
    pub async fn get(&self, webhook_ref: &str) -> Option<Arc<WebhookRuntime>> {
        self.services.read().await.get(webhook_ref).map(|e| e.runtime.clone())
    }

    /// Register or update a webhook from a LinkSys resource change.
    ///
    /// `linksys_ns` is the namespace of the LinkSys resource, used to resolve
    /// a `WebhookTarget::Service` whose `namespace` field is absent.
    ///
    /// Returns `Err` when `build_webhook_client` rejects the config — currently
    /// when `tls.insecure_skip_verify=true` is set with `EDGION_ENV=production`,
    /// or when the supplied PEM material is malformed. The webhook is **not**
    /// registered in that case, so the LinkSys's `init()` surfaces the failure
    /// to the resource processor (matching the Elasticsearch / Redis / Etcd
    /// providers, which already propagate build-time errors). This avoids
    /// silently downgrading a misconfigured webhook to the default secure
    /// client (which has no private CA wired in and would fail every call).
    pub async fn upsert(
        &self,
        webhook_ref: &str,
        config: WebhookServiceConfig,
        linksys_ns: Option<&str>,
    ) -> anyhow::Result<()> {
        // Reject auth shapes whose runtime injection is not yet implemented, so the
        // failure surfaces at apply time instead of as a silent unauthenticated request.
        if let Some(WebhookAuth::ApiKey { r#in, .. }) = config.auth.as_ref() {
            if r#in == "query" {
                return Err(anyhow::anyhow!(
                    "webhook '{}': auth.apiKey.in='query' is not yet implemented; use 'header'",
                    webhook_ref
                ));
            }
        }
        if let Some(WebhookAuth::Hmac { .. }) = config.auth.as_ref() {
            return Err(anyhow::anyhow!(
                "webhook '{}': auth.hmac is not yet implemented; remove the auth field or use bearer/basic/apiKey(header)",
                webhook_ref
            ));
        }

        let tls_on = config.tls.as_ref().is_some_and(|t| t.enabled);
        let scheme = if tls_on { "https" } else { "http" };

        let resolved_target = match &config.target {
            WebhookTarget::Uri { uri } => WebhookResolvedTarget::Static(uri.clone()),

            WebhookTarget::Service {
                name,
                namespace,
                port,
                kind,
                ..
            } => {
                let ns = namespace.as_deref().or(linksys_ns).unwrap_or("default");
                match EdgionService::from_kind(kind.as_ref()) {
                    EdgionService::ServiceClusterIp => {
                        let port_suffix = port.map(|p| format!(":{p}")).unwrap_or_default();
                        WebhookResolvedTarget::Static(format!("{scheme}://{name}.{ns}.svc.cluster.local{port_suffix}"))
                    }
                    EdgionService::Service | EdgionService::ServiceEndpointSlice => WebhookResolvedTarget::Service {
                        service_key: format!("{ns}/{name}"),
                        port: *port,
                        scheme,
                        cursor: AtomicUsize::new(0),
                    },
                    _ => {
                        return Err(anyhow::anyhow!("unsupported service kind: {:?}", kind));
                    }
                }
            }
        };

        let rate_counter = config
            .rate_limit
            .as_ref()
            .map(|rl| SlidingWindowCounter::new(rl.rate, Duration::from_secs(rl.window_sec.max(1))));

        let http_client = match build_webhook_client(&config) {
            None => get_http_client_arc(),
            Some(Ok(client)) => Arc::new(client),
            Some(Err(e)) => {
                tracing::error!(
                    webhook = %webhook_ref,
                    error = %e,
                    "Failed to build webhook HTTP client with TLS config; refusing to register"
                );
                return Err(e.context(format!("build_webhook_client failed for webhook '{}'", webhook_ref)));
            }
        };

        let runtime = Arc::new(WebhookRuntime {
            config,
            http_client,
            resolved_target,
            healthy: AtomicBool::new(true),
            passive_failures: AtomicU32::new(0),
            last_halfopen: AtomicU64::new(0),
            backoff_sec: AtomicU64::new(0),
            rate_counter,
        });

        let health_task = if runtime
            .config
            .health_check
            .as_ref()
            .and_then(|hc| hc.active.as_ref())
            .is_some()
        {
            let ref_clone = webhook_ref.to_string();
            let rt_clone = runtime.clone();
            Some(tokio::spawn(async move {
                spawn_active_health_check(ref_clone, rt_clone).await;
            }))
        } else {
            None
        };

        let mut services = self.services.write().await;
        if let Some(old) = services.remove(webhook_ref) {
            if let Some(handle) = old.health_task {
                handle.abort();
            }
        }
        services.insert(webhook_ref.to_string(), WebhookEntry { runtime, health_task });
        tracing::info!(webhook = %webhook_ref, "Webhook registered/updated in manager");
        Ok(())
    }

    /// Remove a webhook (LinkSys resource deleted)
    pub async fn remove(&self, webhook_ref: &str) {
        if let Some(entry) = self.services.write().await.remove(webhook_ref) {
            if let Some(handle) = entry.health_task {
                handle.abort();
            }
        }
        tracing::info!(webhook = %webhook_ref, "Webhook removed from manager");
    }
}

// ============================================================
// WebhookManagerOps — abstraction for testability
// ============================================================

/// Abstraction over the webhook manager's mutating operations, so that
/// `WebhookLinkClient` can be unit-tested with a fake implementation.
///
/// Production wiring uses `StaticWebhookManagerAdapter` to delegate to the
/// global `get_webhook_manager()`. Tests provide their own implementor.
#[async_trait]
pub trait WebhookManagerOps: Send + Sync + 'static {
    async fn upsert(&self, key: &str, config: WebhookServiceConfig, linksys_ns: Option<&str>) -> anyhow::Result<()>;
    async fn remove(&self, key: &str);
}

/// Zero-sized adapter that bridges the `&'static WebhookManager` returned by
/// `get_webhook_manager()` into the `Arc<dyn WebhookManagerOps>` shape that
/// `WebhookLinkClient` holds.
pub struct StaticWebhookManagerAdapter;

#[async_trait]
impl WebhookManagerOps for StaticWebhookManagerAdapter {
    async fn upsert(&self, key: &str, config: WebhookServiceConfig, linksys_ns: Option<&str>) -> anyhow::Result<()> {
        get_webhook_manager().upsert(key, config, linksys_ns).await
    }

    async fn remove(&self, key: &str) {
        get_webhook_manager().remove(key).await
    }
}

// ============================================================
// Public call handle for plugins and gateway internals
// ============================================================

/// Handle for making outbound calls via a registered webhook connection.
pub struct WebhookCallHandle(pub(crate) std::sync::Arc<super::runtime::WebhookRuntime>);

impl WebhookCallHandle {
    /// Execute an outbound HTTP call via this webhook connection.
    pub async fn call(
        &self,
        req: super::call::WebhookCallRequest,
    ) -> Result<super::call::WebhookCallResult, super::call::WebhookCallError> {
        super::call::webhook_call(&self.0, req, get_webhook_global_max()).await
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

    #[tokio::test]
    async fn test_manager_upsert_and_get() {
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;
        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget::Uri {
                uri: "http://localhost:8080/resolve".to_string(),
            },
            ..Default::default()
        };
        manager
            .upsert("test/webhook", config, None)
            .await
            .expect("upsert should succeed without TLS config");

        let runtime = manager.get("test/webhook").await;
        assert!(runtime.is_some());
        assert!(runtime.unwrap().healthy.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_manager_remove() {
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;
        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget::Uri {
                uri: "http://localhost:8080/resolve".to_string(),
            },
            ..Default::default()
        };
        manager
            .upsert("test/webhook", config, None)
            .await
            .expect("upsert should succeed without TLS config");
        manager.remove("test/webhook").await;

        let runtime = manager.get("test/webhook").await;
        assert!(runtime.is_none());
    }

    #[tokio::test]
    async fn test_manager_service_cluster_ip_builds_dns_url() {
        use crate::link_sys::providers::webhook::runtime::WebhookResolvedTarget;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget::Service {
                name: "auth-svc".to_string(),
                namespace: Some("prod".to_string()),
                port: Some(8080),
                group: None,
                kind: Some("ServiceClusterIp".to_string()),
            },
            ..Default::default()
        };
        manager
            .upsert("prod/auth", config, None)
            .await
            .expect("upsert should succeed");

        let runtime = manager.get("prod/auth").await.expect("runtime should exist");
        match &runtime.resolved_target {
            WebhookResolvedTarget::Static(url) => {
                assert_eq!(url, "http://auth-svc.prod.svc.cluster.local:8080");
            }
            other => panic!("expected Static, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_manager_service_cluster_ip_uses_linksys_ns_fallback() {
        use crate::link_sys::providers::webhook::runtime::WebhookResolvedTarget;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget::Service {
                name: "svc".to_string(),
                namespace: None, // absent — should use linksys_ns
                port: Some(9090),
                group: None,
                kind: Some("ServiceClusterIp".to_string()),
            },
            ..Default::default()
        };
        manager
            .upsert("ns/wh", config, Some("my-ns"))
            .await
            .expect("upsert should succeed");

        let runtime = manager.get("ns/wh").await.expect("runtime should exist");
        match &runtime.resolved_target {
            WebhookResolvedTarget::Static(url) => {
                assert_eq!(url, "http://svc.my-ns.svc.cluster.local:9090");
            }
            other => panic!("expected Static, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_manager_unsupported_service_kind_returns_error() {
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget::Service {
                name: "svc".to_string(),
                namespace: None,
                port: Some(80),
                group: None,
                kind: Some("ServiceImport".to_string()),
            },
            ..Default::default()
        };
        let result = manager.upsert("ns/wh", config, None).await;
        assert!(result.is_err(), "ServiceImport should be rejected by manager");
    }

    #[tokio::test]
    async fn service_kind_none_builds_service_target() {
        use crate::link_sys::providers::webhook::runtime::WebhookResolvedTarget;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget::Service {
                name: "backend".to_string(),
                namespace: Some("ns1".to_string()),
                port: Some(8080),
                group: None,
                kind: None, // None → EdgionService::Service → WebhookResolvedTarget::Service
            },
            ..Default::default()
        };
        manager
            .upsert("ns1/backend", config, None)
            .await
            .expect("upsert with kind=None should succeed");

        let runtime = manager.get("ns1/backend").await.expect("runtime should exist");
        match &runtime.resolved_target {
            WebhookResolvedTarget::Service {
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
        use crate::link_sys::providers::webhook::runtime::WebhookResolvedTarget;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget::Service {
                name: "backend".to_string(),
                namespace: None, // absent — should fall back to linksys_ns
                port: Some(8080),
                group: None,
                kind: None,
            },
            ..Default::default()
        };
        manager
            .upsert("fallback-ns/backend", config, Some("fallback-ns"))
            .await
            .expect("upsert with namespace=None and linksys_ns should succeed");

        let runtime = manager.get("fallback-ns/backend").await.expect("runtime should exist");
        match &runtime.resolved_target {
            WebhookResolvedTarget::Service {
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
    async fn test_manager_hmac_auth_returns_error() {
        use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
        use edgion_resources::resources::link_sys::webhook::{WebhookAuth, WebhookTarget};

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget::Uri {
                uri: "http://localhost:8080/resolve".to_string(),
            },
            auth: Some(WebhookAuth::Hmac {
                slot: SecretSlot {
                    secret_ref: SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "hmac-secret".to_string(),
                        namespace: None,
                    },
                    secret: None,
                },
            }),
            ..Default::default()
        };
        // auth.hmac is not yet implemented; upsert must fail closed rather than
        // register a webhook that silently sends unsigned requests.
        let result = manager.upsert("ns/wh", config, None).await;
        assert!(result.is_err(), "auth.hmac should be rejected by manager");
    }
}
