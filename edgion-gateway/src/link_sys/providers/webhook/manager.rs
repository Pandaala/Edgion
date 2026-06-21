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
use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;

use super::compiled_request::CompiledOutboundRequest;
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
        // Note: auth field is now a deprecated shim field (Option<serde_json::Value>)
        // that is accepted for backward compatibility but not used at runtime.
        // Auth validation and injection will be re-implemented in Wave B with the new request field.
        // Skip auth validation for shim fields.

        let tls_on = config.tls.as_ref().is_some_and(|t| t.enabled);
        let scheme = if tls_on { "https" } else { "http" };

        let backend_ref = &config.target.backend_ref;
        let ns = backend_ref.namespace.as_deref().or(linksys_ns).unwrap_or("default");
        let resolved_target = match EdgionService::from_kind(backend_ref.kind.as_ref()) {
            EdgionService::ServiceClusterIp => {
                let port_suffix = backend_ref.port.map(|p| format!(":{p}")).unwrap_or_default();
                WebhookResolvedTarget::Static(format!(
                    "{scheme}://{}.{}.svc.cluster.local{}",
                    backend_ref.name, ns, port_suffix
                ))
            }
            EdgionService::Service | EdgionService::ServiceEndpointSlice => WebhookResolvedTarget::Service {
                service_key: format!("{}/{}", ns, backend_ref.name),
                port: backend_ref.port,
                scheme,
                cursor: AtomicUsize::new(0),
            },
            _ => {
                return Err(anyhow::anyhow!("unsupported service kind: {:?}", backend_ref.kind));
            }
        };

        // Compile the outbound request spec at load time. Templates are parsed
        // once; `${secretRef:...}` segments are resolved at call time from
        // `config.resolved_secrets` (carried inline by the controller).
        let compiled_request = CompiledOutboundRequest::from_spec(&config.request)
            .map_err(|e| anyhow::anyhow!("failed to compile webhook request for '{}': {}", webhook_ref, e))?;

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

        // Health state is always reset on upsert because config.request.* may have
        // changed.  EndpointSlice churn never calls upsert(), so health is preserved
        // across pod turnover (the Service variant of WebhookResolvedTarget reads the
        // global EpSliceStore fresh at call time without rebuilding the runtime).
        let runtime = Arc::new(WebhookRuntime {
            config,
            http_client,
            resolved_target,
            compiled_request,
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
    ///
    /// The outbound request shape is defined by the webhook resource; the caller
    /// only supplies the `session` (for origin / template lookups) and an
    /// optional `path_override` (honoured when the compiled path allows it).
    pub async fn call(
        &self,
        session: &dyn crate::plugins::runtime::PluginSession,
        path_override: Option<String>,
    ) -> Result<super::call::WebhookCallResult, super::call::WebhookCallError> {
        super::call::webhook_call(&self.0, session, path_override, get_webhook_global_max()).await
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
        use edgion_resources::resources::http_route::BackendObjectReference;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;
        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: None,
                    name: "resolve-svc".to_string(),
                    namespace: Some("default".to_string()),
                    port: Some(8080),
                },
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
        use edgion_resources::resources::http_route::BackendObjectReference;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;
        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: None,
                    name: "resolve-svc".to_string(),
                    namespace: Some("default".to_string()),
                    port: Some(8080),
                },
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
        use edgion_resources::resources::http_route::BackendObjectReference;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: Some("ServiceClusterIp".to_string()),
                    name: "auth-svc".to_string(),
                    namespace: Some("prod".to_string()),
                    port: Some(8080),
                },
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
        use edgion_resources::resources::http_route::BackendObjectReference;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: Some("ServiceClusterIp".to_string()),
                    name: "svc".to_string(),
                    namespace: None, // absent — should use linksys_ns
                    port: Some(9090),
                },
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
        use edgion_resources::resources::http_route::BackendObjectReference;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: Some("ServiceImport".to_string()),
                    name: "svc".to_string(),
                    namespace: None,
                    port: Some(80),
                },
            },
            ..Default::default()
        };
        let result = manager.upsert("ns/wh", config, None).await;
        assert!(result.is_err(), "ServiceImport should be rejected by manager");
    }

    #[tokio::test]
    async fn service_kind_none_builds_service_target() {
        use crate::link_sys::providers::webhook::runtime::WebhookResolvedTarget;
        use edgion_resources::resources::http_route::BackendObjectReference;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: None, // None → EdgionService::Service → WebhookResolvedTarget::Service
                    name: "backend".to_string(),
                    namespace: Some("ns1".to_string()),
                    port: Some(8080),
                },
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
        use edgion_resources::resources::http_route::BackendObjectReference;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: None,
                    name: "backend".to_string(),
                    namespace: None, // absent — should fall back to linksys_ns
                    port: Some(8080),
                },
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
    async fn test_manager_webhook_with_shim_auth_succeeds() {
        use edgion_resources::resources::http_route::BackendObjectReference;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;

        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: None,
                    name: "resolve-svc".to_string(),
                    namespace: Some("default".to_string()),
                    port: Some(8080),
                },
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
        use edgion_resources::resources::http_route::BackendObjectReference;
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;
        WebhookServiceConfig {
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: None,
                    name: name.to_string(),
                    namespace: Some("default".to_string()),
                    port: Some(8080),
                },
            },
            ..Default::default()
        }
    }

    /// upsert() stores a compiled_request on the runtime (default spec succeeds).
    #[tokio::test]
    async fn upsert_stores_compiled_request_on_runtime() {
        let manager = WebhookManager::new();
        manager
            .upsert("ns/wh-compiled", make_config_with_target("svc"), None)
            .await
            .expect("upsert with default request should succeed");

        let runtime = manager.get("ns/wh-compiled").await.expect("runtime must exist");
        // The default spec has no path, method, args, headers, or cookies — all None/empty.
        // Verify the compiled_request is present by accessing it (no panic).
        let _ = &runtime.compiled_request;
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

        // No runtime must have been registered.
        assert!(
            manager.get("ns/bad-template").await.is_none(),
            "no runtime should be registered after a compile error"
        );
    }

    /// Calling upsert() a second time with a changed request resets health state.
    ///
    /// This verifies that health counters are not carried over across config changes:
    /// a new `WebhookRuntime` (and thus fresh `AtomicBool`/`AtomicU32` health fields)
    /// is created on every call to `upsert()`.
    #[tokio::test]
    async fn upsert_resets_health_on_request_change() {
        use edgion_resources::resources::link_sys::webhook::{MethodSpec, WebhookOutboundRequest};

        let manager = WebhookManager::new();

        // Initial registration.
        manager
            .upsert("ns/health-reset", make_config_with_target("svc"), None)
            .await
            .expect("initial upsert should succeed");

        // Simulate degraded health on the first runtime.
        let runtime1 = manager.get("ns/health-reset").await.expect("runtime must exist");
        runtime1.healthy.store(false, Ordering::Relaxed);
        runtime1.passive_failures.store(5, Ordering::Relaxed);
        let ptr1 = Arc::as_ptr(&runtime1);

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

        let runtime2 = manager.get("ns/health-reset").await.expect("runtime must exist");

        // A brand-new Arc is stored: the old runtime has been replaced.
        assert_ne!(Arc::as_ptr(&runtime2), ptr1, "upsert must replace the runtime Arc");
        // Health is reset on the new runtime.
        assert!(
            runtime2.healthy.load(Ordering::Relaxed),
            "healthy must be reset to true on re-upsert"
        );
        assert_eq!(
            runtime2.passive_failures.load(Ordering::Relaxed),
            0,
            "passive_failures must be reset to 0 on re-upsert"
        );
    }

    /// EndpointSlice churn does NOT call upsert(), so health state is preserved.
    ///
    /// Regression test: the `WebhookResolvedTarget::Service` variant reads the
    /// global EpSliceStore at call time.  Endpoint add/remove/re-add must never
    /// rebuild the `WebhookRuntime` or reset its health atomics.
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

        // Record the Arc identity and degrade the runtime health.
        let runtime = manager.get("ns/ep-churn").await.expect("runtime must exist");
        let ptr_before = Arc::as_ptr(&runtime);
        runtime.healthy.store(false, Ordering::Relaxed);
        runtime.passive_failures.store(3, Ordering::Relaxed);

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

        // The runtime fetched from the manager must be the SAME Arc — upsert was not called.
        let runtime_after = manager.get("ns/ep-churn").await.expect("runtime must exist");
        assert_eq!(
            Arc::as_ptr(&runtime_after),
            ptr_before,
            "endpoint churn must not replace the runtime Arc"
        );
        assert!(
            !runtime_after.healthy.load(Ordering::Relaxed),
            "healthy must remain false after endpoint churn"
        );
        assert_eq!(
            runtime_after.passive_failures.load(Ordering::Relaxed),
            3,
            "passive_failures must remain 3 after endpoint churn"
        );

        // Cleanup.
        store.update_data_only(HashMap::new(), HashMap::new(), &HashSet::from([slice_key.to_string()]));
    }
}
