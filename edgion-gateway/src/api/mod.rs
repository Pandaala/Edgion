use crate::conf_sync::ConfigClient;
use crate::runtime::server::proxy_protocol_policy::get_global_proxy_protocol_policy_runtime;
use axum::{
    extract::{Path, Query, State},
    response::Json,
    routing::{get, post},
    Router,
};
use edgion_resources::ResourceKind;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod admin_auth;

/// Query parameters for resource lookup
#[derive(Deserialize)]
struct ResourceQuery {
    namespace: Option<String>,
    name: Option<String>,
}

/// Standard API response format
#[derive(Serialize)]
struct ApiResponse<T> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T> ApiResponse<T> {
    fn ok_body(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    fn err_body(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
        }
    }
}

/// List response format
#[derive(Serialize)]
struct ListResponse {
    success: bool,
    data: Vec<serde_json::Value>,
    count: usize,
}

impl ListResponse {
    fn success(data: Vec<serde_json::Value>) -> Self {
        let count = data.len();
        Self {
            success: true,
            data,
            count,
        }
    }
}

/// Server info response
#[derive(Serialize)]
struct ServerInfoResponse {
    server_id: String,
    ready: bool,
}

/// Health check endpoint (liveness)
async fn health_check() -> Json<ApiResponse<String>> {
    Json(ApiResponse::ok_body("OK".to_string()))
}

/// Readiness check endpoint - returns OK only when all caches are ready
async fn readiness_check(State(client): State<Arc<ConfigClient>>) -> axum::http::StatusCode {
    readiness_status(
        client.is_ready().is_ok(),
        get_global_proxy_protocol_policy_runtime().is_fatal(),
    )
}

fn readiness_status(config_ready: bool, policy_fatal: bool) -> axum::http::StatusCode {
    if config_ready && !policy_fatal {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Get server info including the current server_id from Controller
///
/// This returns the server_id that Gateway received from Controller.
/// After a reload, this value should change to match the Controller's new server_id.
async fn get_server_info(State(client): State<Arc<ConfigClient>>) -> Json<ApiResponse<ServerInfoResponse>> {
    let server_id = client.current_server_id();
    let ready = client.is_ready().is_ok() && !get_global_proxy_protocol_policy_runtime().is_fatal();
    Json(ApiResponse::ok_body(ServerInfoResponse { server_id, ready }))
}

/// Backfill response payload.
#[derive(Serialize)]
struct BackfillResponse {
    summary: crate::observe::access_log::BackfillSummary,
}

/// Single-flight guard for the backfill endpoint: two concurrent runs would
/// double-ingest every line and race on the `.backfilled-` rename. A second
/// concurrent trigger is rejected with 409.
static BACKFILL_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Clears `BACKFILL_RUNNING` on drop, so the flag is released even on an early
/// return or a panic in the handler.
struct BackfillRunningGuard;
impl Drop for BackfillRunningGuard {
    fn drop(&mut self) {
        BACKFILL_RUNNING.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// Trigger access-log backfill: re-ingest the bounded LocalFile fallback into
/// the configured ES primary. ES-only; requires ES healthy. Idempotent at file
/// granularity (already-backfilled files are skipped). At-least-once semantics.
/// Single-flight: a concurrent trigger returns 409.
async fn access_log_backfill() -> (axum::http::StatusCode, Json<ApiResponse<BackfillResponse>>) {
    use axum::http::StatusCode;

    let Some(cfg) = crate::observe::access_log::get_backfill_config() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::err_body("access logger not initialized".into())),
        );
    };
    let Some(es_key) = cfg.es_key.clone() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err_body(
                "backfill applies only when the access-log primary is Elasticsearch".into(),
            )),
        );
    };
    let Some(base) = cfg.fallback_base_path.clone() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err_body(
                "no LocalFile fallback configured for access log".into(),
            )),
        );
    };

    // Health gate: refuse while ES is still down.
    match crate::link_sys::get_es_client(&es_key) {
        Some(client) if client.healthy() => {}
        Some(_) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::err_body(
                    "ES is not healthy yet; retry after recovery".into(),
                )),
            );
        }
        None => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::err_body(format!("ES client '{es_key}' not found"))),
            );
        }
    }

    // Single-flight: refuse a concurrent backfill (it would double-ingest every
    // line and race on the `.backfilled-` rename). The guard releases the flag
    // on drop, covering early returns and panics.
    if BACKFILL_RUNNING
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_err()
    {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::err_body("a backfill is already in progress".into())),
        );
    }
    let _running = BackfillRunningGuard;

    // Epoch-seconds rename stamp. SystemTime is read here (the API edge), not in
    // the engine, so the engine stays unit-testable.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let summary = crate::observe::access_log::run_backfill(&es_key, &base, ts).await;
    (StatusCode::OK, Json(ApiResponse::ok_body(BackfillResponse { summary })))
}

/// List all resources of `kind` from ConfigClient as JSON, with secret fields
/// redacted for admin output.
///
/// An [`AdminRedactGuard`] is held for the entire (synchronous) serialization, so
/// every secret-bearing field annotated with the `redact_opt` / `redact_required`
/// serde adapters emits `"[redacted]"`. Both the list endpoint and the get-by-name
/// endpoint funnel through here, so both are covered by the single guard.
///
/// INVARIANT: keep this path synchronous and single-threaded — the guard is a
/// thread-local and would not apply if serialization moved to another thread.
/// Kinds not cached on the gateway (e.g. `ReferenceGrant`, `Secret`) resolve to
/// `None` and yield an empty list.
fn list_client_resources(client: &ConfigClient, kind: ResourceKind) -> Vec<serde_json::Value> {
    let _guard = edgion_resources::redact::AdminRedactGuard::new();
    client
        .get_dyn_cache(kind)
        .map(|cache| cache.list_json_dyn())
        .unwrap_or_default()
}

/// Get a single resource of `kind` by name (and namespace, unless cluster-scoped).
///
/// Reuses [`list_client_resources`], so it inherits the same admin-scope secret
/// redaction, then filters the JSON by `metadata.name` / `metadata.namespace`.
fn get_client_resource(
    client: &ConfigClient,
    kind: ResourceKind,
    namespace: Option<&str>,
    name: &str,
) -> Option<serde_json::Value> {
    let cluster_scoped = edgion_resources::resource::is_kind_cluster_scoped(kind);
    list_client_resources(client, kind).into_iter().find(|value| {
        let meta = value.get("metadata");
        let name_matches = meta.and_then(|m| m.get("name")).and_then(|n| n.as_str()) == Some(name);
        if !name_matches {
            return false;
        }
        if cluster_scoped {
            return true;
        }
        meta.and_then(|m| m.get("namespace")).and_then(|n| n.as_str()) == namespace
    })
}

/// Dynamic list endpoint - list all resources of a kind
async fn list_resources(State(client): State<Arc<ConfigClient>>, Path(kind_str): Path<String>) -> Json<ListResponse> {
    let kind = ResourceKind::from_kind_name(&kind_str).unwrap_or(ResourceKind::Unspecified);
    let data = list_client_resources(&client, kind);
    Json(ListResponse::success(data))
}

/// Dynamic get endpoint - get a single resource by namespace and name
async fn get_resource(
    State(client): State<Arc<ConfigClient>>,
    Path(kind_str): Path<String>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<serde_json::Value>> {
    let Some(name) = query.name else {
        return Json(ApiResponse::err_body("Missing required parameter: name".to_string()));
    };

    let kind = ResourceKind::from_kind_name(&kind_str).unwrap_or(ResourceKind::Unspecified);
    let resource = get_client_resource(&client, kind, query.namespace.as_deref(), &name);

    match resource {
        Some(r) => Json(ApiResponse::ok_body(r)),
        None => Json(ApiResponse::err_body(format!("{} not found", kind_str))),
    }
}

/// Create the admin API router with dynamic endpoints, guarded by the
/// bearer-token auth middleware.
///
/// `admin_token` is the resolved shared token (env > config), or `None` when no
/// token is configured (fail-close). The auth layer wraps the fully-merged
/// router (including the integration-testing routes) so no admin route is left
/// unauthenticated.
pub fn create_admin_router(config_client: Arc<ConfigClient>, admin_token: Option<String>) -> Router {
    let mut router = Router::new()
        // Dynamic endpoints for all resource types (secrets redacted via the
        // AdminRedactGuard inside list_client_resources).
        .route("/configclient/{kind}", get(get_resource))
        .route("/configclient/{kind}/list", get(list_resources))
        .route("/api/v1/access-log/backfill", post(access_log_backfill))
        .with_state(config_client.clone());

    // Add testing endpoints (incl. /api/v1/server-info) only in integration testing mode.
    if edgion_common::test_mode::is_integration_testing_mode() {
        router = router.merge(create_testing_router(config_client));
    }

    // Auth layer LAST, on the fully-merged router — covers the testing/destructive
    // routes as well as /configclient.
    let auth_state = admin_auth::AdminAuthState::from_token(admin_token);
    router.layer(axum::middleware::from_fn_with_state(
        auth_state,
        admin_auth::admin_auth_middleware,
    ))
}

/// Create the probe router: only the non-sensitive liveness/readiness endpoints.
/// Mounted on a dedicated outward-facing port so the admin port can stay loopback.
pub fn create_probe_router(config_client: Arc<ConfigClient>) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/ready", get(readiness_check))
        .with_state(config_client)
}

// ==================== Integration Testing Endpoints ====================

/// Testing status response
#[derive(Serialize)]
struct TestingStatusResponse {
    integration_testing_mode: bool,
    access_log_store: crate::observe::access_log_store::AccessLogStoreStatus,
}

/// Get testing subsystem status
async fn testing_status() -> Json<ApiResponse<TestingStatusResponse>> {
    let store = crate::observe::access_log_store::get_access_log_store();
    Json(ApiResponse::ok_body(TestingStatusResponse {
        integration_testing_mode: true,
        access_log_store: store.status(),
    }))
}

/// Get access log by trace_id
async fn get_access_log(Path(trace_id): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let store = crate::observe::access_log_store::get_access_log_store();
    match store.get(&trace_id) {
        Some(json) => {
            // Parse JSON string back to Value
            match serde_json::from_str::<serde_json::Value>(&json) {
                Ok(value) => Json(ApiResponse::ok_body(value)),
                Err(_) => Json(ApiResponse::ok_body(serde_json::Value::String(json))),
            }
        }
        None => Json(ApiResponse::err_body(format!(
            "Access log not found for trace_id: {}",
            trace_id
        ))),
    }
}

/// Delete access log by trace_id
async fn delete_access_log(Path(trace_id): Path<String>) -> Json<ApiResponse<String>> {
    let store = crate::observe::access_log_store::get_access_log_store();
    if store.delete(&trace_id) {
        Json(ApiResponse::ok_body("Deleted".to_string()))
    } else {
        Json(ApiResponse::err_body(format!(
            "Access log not found for trace_id: {}",
            trace_id
        )))
    }
}

/// List all stored access logs
async fn list_access_logs() -> Json<crate::observe::access_log_store::AccessLogListResponse> {
    let store = crate::observe::access_log_store::get_access_log_store();
    let items = store.list();
    Json(crate::observe::access_log_store::AccessLogListResponse {
        success: true,
        count: items.len(),
        data: items,
    })
}

/// Delete all stored access logs
async fn clear_access_logs() -> Json<ApiResponse<String>> {
    let store = crate::observe::access_log_store::get_access_log_store();
    store.clear();
    Json(ApiResponse::ok_body("All access logs cleared".to_string()))
}

// ==================== Debug Store Stats Endpoint ====================

/// Response for /api/v1/debug/store-stats
#[derive(Serialize)]
struct StoreStatsResponse {
    wasm_shared_data_metrics: crate::observe::metrics::registry::WasmSharedDataMetricStats,
    plugin_publication: crate::plugins::publication::PublicationStats,
    plugin_publication_inventory: crate::plugins::publication::PublicationInventory,
    wasm_oci_cache: crate::plugins::http::wasm::oci::cache::OciMemoryCacheStats,
    http_routes: crate::routes::http::HttpRouteManagerStats,
    grpc_routes: crate::routes::grpc::GrpcRouteManagerStats,
    tcp_routes: crate::routes::tcp::TcpRouteManagerStats,
    udp_routes: crate::routes::udp::UdpRouteManagerStats,
    tls_routes: crate::routes::tls::TlsRouteManagerStats,
    gateway_config: GatewayConfigStats,
    listener_gateway_info: crate::runtime::store::listener_gateway_info::ListenerGatewayInfoStats,
    gateway_tls_matcher: crate::runtime::matching::tls::GatewayTlsMatcherStats,
    tls_store: TlsStoreStats,
    tls_cert_matcher: TlsCertMatcherStats,
    link_sys_store: LinkSysStoreStats,
    lb_policies: std::collections::BTreeMap<String, String>,
    backend_tls_policy: crate::backends::policy::BackendTLSPolicyStoreStats,
    gateway_class_store: SimpleCountStats,
    edgion_gateway_config_store: SimpleCountStats,
    egc_bindings: crate::config::edgion_gateway::GatewayConfigBindingStats,
    client_caches: ClientCachesStats,
    service_discovery: crate::backends::discovery::ServiceDiscoveryStatsSnapshot,
    health_status_store: SimpleCountStats,
    outlier_detection_store: SimpleCountStats,
    health_check_config_store: SimpleCountStats,
    gateway_backend_tls_store: SimpleCountStats,
    edgion_config_data_store: SimpleCountStats,
    edgion_backend_store: EdgionBackendStoreStats,
    ai_credential_outlier_store: SimpleCountStats,
    adaptive_telemetry_store: SimpleCountStats,
    link_sys_runtime: LinkSysRuntimeStats,
    plugin_store_v2: PluginStoreStatsV2,
    stream_plugin_store_v2: PluginStoreStatsV2,
}

#[derive(Serialize)]
struct GatewayConfigStats {
    gateways: usize,
}

#[derive(Serialize)]
struct TlsStoreStats {
    entries: usize,
    valid: usize,
    invalid: usize,
}

#[derive(Serialize)]
struct TlsCertMatcherStats {
    listener_count: usize,
}

#[derive(Serialize)]
struct LinkSysStoreStats {
    resources: usize,
}

#[derive(Serialize)]
struct SimpleCountStats {
    count: usize,
}

/// Unified EdgionBackend runtime index stats.
///
/// The store is process-global and shared by every AI backend resource, so a
/// bare total cannot tell a caller whether one specific backend compiled into
/// the activatable index. `keys` names the resident entries so a test can
/// assert its own backend directly instead of doing arithmetic on a total that
/// other resources move underneath it.
#[derive(Serialize)]
struct EdgionBackendStoreStats {
    count: usize,
    keys: Vec<String>,
}

fn collect_ai_runtime_store_counts(
    credential_outliers: &crate::ai::AiCredentialOutlierStore,
    adaptive_telemetry: &crate::ai::AdaptiveTelemetryStore,
) -> (SimpleCountStats, SimpleCountStats) {
    (
        SimpleCountStats {
            count: credential_outliers.count(),
        },
        SimpleCountStats {
            count: adaptive_telemetry.count(),
        },
    )
}

#[derive(Serialize)]
struct ClientCachesStats {
    by_kind: std::collections::BTreeMap<String, usize>,
}

#[derive(Serialize)]
struct LinkSysRuntimeStats {
    count: usize,
    keys: Vec<String>,
    redis_clients: usize,
    etcd_clients: usize,
    es_clients: usize,
    kafka_clients: usize,
    webhook_clients: usize,
    otlp_clients: usize,
    otlp_retiring_clients: usize,
}

#[derive(Serialize)]
struct PluginStoreStatsV2 {
    total: usize,
    by_kind: std::collections::BTreeMap<String, usize>,
}

/// Collect statistics from all derived stores for config-sync leak detection.
async fn store_stats(State(client): State<Arc<ConfigClient>>) -> Json<ApiResponse<StoreStatsResponse>> {
    let wasm_oci_cache = crate::plugins::http::wasm::oci::cache::memory_cache_stats();
    let http_routes = crate::routes::http::get_global_http_route_managers().stats();
    let grpc_routes = crate::routes::grpc::get_global_grpc_route_managers().stats();
    let tcp_routes = crate::routes::tcp::get_global_tcp_route_managers().stats();
    let udp_routes = crate::routes::udp::get_global_udp_route_managers().stats();
    let tls_routes = crate::routes::tls::get_global_tls_route_managers().stats();

    let gateway_config = GatewayConfigStats {
        gateways: crate::runtime::store::get_global_gateway_store().read().count(),
    };
    let listener_gateway_info = crate::runtime::store::listener_gateway_info::get_listener_gateway_info_store().stats();
    let gateway_tls_matcher = crate::runtime::matching::tls::get_gateway_tls_matcher().stats();

    let tls = crate::tls::store::tls_store::get_global_tls_store().snapshot();
    let (valid, invalid) = tls.get_cert_stats();
    let tls_store = TlsStoreStats {
        entries: tls.entry_count(),
        valid,
        invalid,
    };

    let tls_cert_matcher = TlsCertMatcherStats {
        listener_count: tls.listener_count(),
    };

    let http_plugin_store = crate::plugins::http::get_global_plugin_store();
    let stream_plugin_store_ref = crate::plugins::stream::get_global_stream_plugin_store();

    let link_sys_store = LinkSysStoreStats {
        resources: crate::link_sys::runtime::store::get_global_link_sys_store().count(),
    };

    let lb_policies = crate::backends::policy::get_backend_traffic_policy_store().lb_policy_snapshot();

    let backend_tls_policy = crate::backends::policy::get_global_backend_tls_policy_store().stats();

    let gateway_class_store = SimpleCountStats {
        count: crate::config::gateway_class::count_gateway_classes(),
    };
    let edgion_gateway_config_store = SimpleCountStats {
        count: crate::config::edgion_gateway::count_edgion_gateway_configs(),
    };

    let client_caches = ClientCachesStats {
        by_kind: client.client_cache_stats(),
    };

    let service_discovery = crate::backends::get_service_discovery_coordinator().stats();

    let hs_g = crate::backends::health::check::status_store::get_health_status_store();
    let health_status_store = SimpleCountStats {
        count: hs_g.tracked_service_count(),
    };

    let cb_g = crate::backends::outlier_detection::store::get_outlier_detection_store();
    let outlier_detection_store = SimpleCountStats {
        count: cb_g.service_count(),
    };

    let hcc_g = crate::backends::health::check::config_store::get_hc_config_store();
    let health_check_config_store = SimpleCountStats { count: hcc_g.count() };

    let gbt_g = crate::backends::gateway_backend_tls_store::get_global_gateway_backend_tls_store();
    let gateway_backend_tls_store = SimpleCountStats { count: gbt_g.count() };

    let edgion_config_data_store = SimpleCountStats {
        count: crate::plugins::runtime::edgion_config_data::count_edgion_config_data(),
    };

    let edgion_backend_store = {
        let mut keys = crate::ai::global_edgion_backend_store().keys_all();
        keys.sort_unstable();
        EdgionBackendStoreStats {
            count: keys.len(),
            keys,
        }
    };
    let (ai_credential_outlier_store, adaptive_telemetry_store) = collect_ai_runtime_store_counts(
        crate::ai::get_ai_credential_outlier_store().as_ref(),
        crate::ai::get_adaptive_telemetry_store().as_ref(),
    );

    let link_sys_runtime = LinkSysRuntimeStats {
        count: crate::link_sys::runtime::store::runtime_client_count(),
        keys: crate::link_sys::runtime::store::list_runtime_clients(),
        redis_clients: crate::link_sys::runtime::store::redis_client_count(),
        etcd_clients: crate::link_sys::runtime::store::etcd_client_count(),
        es_clients: crate::link_sys::runtime::store::es_client_count(),
        kafka_clients: crate::link_sys::runtime::store::kafka_client_count(),
        webhook_clients: crate::link_sys::runtime::store::webhook_client_count(),
        otlp_clients: crate::link_sys::runtime::store::otlp_client_count(),
        otlp_retiring_clients: crate::link_sys::runtime::store::otlp_retiring_client_count(),
    };

    let plugin_store_v2 = PluginStoreStatsV2 {
        total: http_plugin_store.count(),
        by_kind: http_plugin_store.count_by_kind(),
    };

    let stream_plugin_store_v2 = PluginStoreStatsV2 {
        total: stream_plugin_store_ref.count(),
        by_kind: stream_plugin_store_ref.count_by_kind(),
    };

    let publication = crate::plugins::publication::global_publisher().snapshot();
    Json(ApiResponse::ok_body(StoreStatsResponse {
        wasm_shared_data_metrics: crate::observe::metrics::registry::wasm_shared_data_metric_stats(),
        plugin_publication: publication.stats(),
        plugin_publication_inventory: publication.inventory().clone(),
        wasm_oci_cache,
        http_routes,
        grpc_routes,
        tcp_routes,
        udp_routes,
        tls_routes,
        gateway_config,
        listener_gateway_info,
        gateway_tls_matcher,
        tls_store,
        tls_cert_matcher,
        link_sys_store,
        lb_policies,
        backend_tls_policy,
        gateway_class_store,
        edgion_gateway_config_store,
        egc_bindings: crate::config::edgion_gateway::gateway_config_binding_stats(),
        client_caches,
        service_discovery,
        health_status_store,
        outlier_detection_store,
        health_check_config_store,
        gateway_backend_tls_store,
        edgion_config_data_store,
        edgion_backend_store,
        ai_credential_outlier_store,
        adaptive_telemetry_store,
        link_sys_runtime,
        plugin_store_v2,
        stream_plugin_store_v2,
    }))
}

/// Create testing-specific router (only added when integration testing mode is enabled)
fn create_testing_router(config_client: Arc<ConfigClient>) -> Router {
    Router::new()
        .route("/api/v1/testing/status", get(testing_status))
        .route("/api/v1/debug/store-stats", get(store_stats))
        // server-info needs State<Arc<ConfigClient>>, so it must precede .with_state.
        .route("/api/v1/server-info", get(get_server_info))
        .with_state(config_client)
        .route(
            "/api/v1/testing/access-log/{trace_id}",
            get(get_access_log).delete(delete_access_log),
        )
        .route(
            "/api/v1/testing/access-logs",
            get(list_access_logs).delete(clear_access_logs),
        )
        // LinkSys Redis testing endpoints
        .route("/api/v1/testing/link-sys/redis/health", get(redis_health_all))
        .route("/api/v1/testing/link-sys/redis/{name}/health", get(redis_health_one))
        .route("/api/v1/testing/link-sys/redis/{name}/ping", get(redis_ping))
        .route("/api/v1/testing/link-sys/redis/{name}/get/{key}", get(redis_get))
        .route(
            "/api/v1/testing/link-sys/redis/{name}/set",
            axum::routing::post(redis_set),
        )
        .route(
            "/api/v1/testing/link-sys/redis/{name}/del",
            axum::routing::post(redis_del),
        )
        .route(
            "/api/v1/testing/link-sys/redis/{name}/hset",
            axum::routing::post(redis_hset),
        )
        .route(
            "/api/v1/testing/link-sys/redis/{name}/hget/{key}/{field}",
            get(redis_hget),
        )
        .route(
            "/api/v1/testing/link-sys/redis/{name}/hgetall/{key}",
            get(redis_hgetall),
        )
        .route(
            "/api/v1/testing/link-sys/redis/{name}/rpush",
            axum::routing::post(redis_rpush),
        )
        .route("/api/v1/testing/link-sys/redis/{name}/lpop/{key}", get(redis_lpop))
        .route("/api/v1/testing/link-sys/redis/{name}/llen/{key}", get(redis_llen))
        .route(
            "/api/v1/testing/link-sys/redis/{name}/incr/{key}",
            axum::routing::post(redis_incr),
        )
        .route(
            "/api/v1/testing/link-sys/redis/{name}/lock",
            axum::routing::post(redis_lock),
        )
        .route("/api/v1/testing/link-sys/redis/clients", get(redis_list_clients))
        // LinkSys Etcd testing endpoints
        .route("/api/v1/testing/link-sys/etcd/health", get(etcd_health_all))
        .route("/api/v1/testing/link-sys/etcd/{name}/health", get(etcd_health_one))
        .route("/api/v1/testing/link-sys/etcd/{name}/ping", get(etcd_ping))
        .route("/api/v1/testing/link-sys/etcd/{name}/get/{key}", get(etcd_get))
        .route(
            "/api/v1/testing/link-sys/etcd/{name}/put",
            axum::routing::post(etcd_put),
        )
        .route(
            "/api/v1/testing/link-sys/etcd/{name}/delete/{key}",
            axum::routing::post(etcd_delete),
        )
        .route(
            "/api/v1/testing/link-sys/etcd/{name}/get-prefix/{prefix}",
            get(etcd_get_prefix),
        )
        .route(
            "/api/v1/testing/link-sys/etcd/{name}/delete-prefix/{prefix}",
            axum::routing::post(etcd_delete_prefix),
        )
        .route(
            "/api/v1/testing/link-sys/etcd/{name}/lease-grant",
            axum::routing::post(etcd_lease_grant),
        )
        .route(
            "/api/v1/testing/link-sys/etcd/{name}/lease-revoke/{lease_id}",
            axum::routing::post(etcd_lease_revoke),
        )
        .route(
            "/api/v1/testing/link-sys/etcd/{name}/lease-ttl/{lease_id}",
            get(etcd_lease_ttl),
        )
        .route(
            "/api/v1/testing/link-sys/etcd/{name}/lock",
            axum::routing::post(etcd_lock),
        )
        .route("/api/v1/testing/link-sys/etcd/clients", get(etcd_list_clients))
        .route("/api/v1/testing/link-sys/etcd/{name}/info", get(etcd_info))
        // LinkSys Elasticsearch testing endpoints
        .route("/api/v1/testing/link-sys/es/health", get(es_health_all))
        .route("/api/v1/testing/link-sys/es/{name}/health", get(es_health_one))
        .route("/api/v1/testing/link-sys/es/{name}/ping", get(es_ping))
        .route("/api/v1/testing/link-sys/es/{name}/info", get(es_info))
        .route("/api/v1/testing/link-sys/es/clients", get(es_list_clients))
        .route(
            "/api/v1/testing/link-sys/es/{name}/index-doc/{index}",
            axum::routing::post(es_index_doc),
        )
        .route(
            "/api/v1/testing/link-sys/es/{name}/get-doc/{index}/{doc_id}",
            get(es_get_doc),
        )
        .route(
            "/api/v1/testing/link-sys/es/{name}/delete-doc/{index}/{doc_id}",
            axum::routing::post(es_delete_doc),
        )
        .route(
            "/api/v1/testing/link-sys/es/{name}/search/{index}",
            axum::routing::post(es_search),
        )
        .route(
            "/api/v1/testing/link-sys/es/{name}/create-index/{index}",
            axum::routing::post(es_create_index),
        )
        .route(
            "/api/v1/testing/link-sys/es/{name}/delete-index/{index}",
            axum::routing::post(es_delete_index),
        )
        .route(
            "/api/v1/testing/link-sys/es/{name}/index-exists/{index}",
            get(es_index_exists),
        )
        .route(
            "/api/v1/testing/link-sys/es/{name}/refresh/{index}",
            axum::routing::post(es_refresh_index),
        )
        .route("/api/v1/testing/link-sys/es/{name}/count/{index}", get(es_count))
        .route(
            "/api/v1/testing/link-sys/es/{name}/bulk",
            axum::routing::post(es_bulk_send),
        )
}

// ==================== LinkSys Redis Testing Endpoints ====================

/// Helper to get a Redis client by name, returning error response if not found.
/// Name format: "namespace/name" encoded as "namespace_name" in URL path.
fn get_redis_client_by_name(
    name: &str,
) -> Result<std::sync::Arc<crate::link_sys::providers::redis::RedisLinkClient>, Json<ApiResponse<serde_json::Value>>> {
    // URL path uses underscore as separator: "default_redis-main" → "default/redis-main"
    let key = name.replacen('_', "/", 1);
    crate::link_sys::get_redis_client(&key).ok_or_else(|| {
        if crate::link_sys::runtime::store::redis_runner_exists(&key) {
            Json(ApiResponse::err_body(format!(
                "Redis client '{key}' not found (registered, not yet connected)"
            )))
        } else {
            Json(ApiResponse::err_body(format!(
                "Redis client '{}' not found. Available: {:?}",
                key,
                crate::link_sys::runtime::store::list_redis_clients()
            )))
        }
    })
}

/// List all registered Redis clients
async fn redis_list_clients() -> Json<ApiResponse<Vec<String>>> {
    let clients = crate::link_sys::runtime::store::list_redis_clients();
    Json(ApiResponse::ok_body(clients))
}

/// Health check all Redis clients
async fn redis_health_all() -> Json<ApiResponse<Vec<crate::link_sys::LinkSysHealth>>> {
    let health = crate::link_sys::runtime::store::health_check_all_redis().await;
    Json(ApiResponse::ok_body(health))
}

/// Health check a single Redis client
async fn redis_health_one(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let health = client.health_status().await;
    match serde_json::to_value(health) {
        Ok(v) => Json(ApiResponse::ok_body(v)),
        Err(e) => Json(ApiResponse::err_body(format!("Serialize error: {}", e))),
    }
}

/// PING a Redis client, returns latency in ms
async fn redis_ping(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.ping().await {
        Ok(latency_ms) => Json(ApiResponse::ok_body(serde_json::json!({
            "latency_ms": latency_ms,
            "healthy": client.healthy(),
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("PING failed: {}", e))),
    }
}

/// GET a key from Redis
async fn redis_get(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.get(&key).await {
        Ok(val) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": key,
            "value": val,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("GET failed: {}", e))),
    }
}

/// SET request body
#[derive(Deserialize)]
struct RedisSetRequest {
    key: String,
    value: String,
    ttl_seconds: Option<u64>,
}

/// SET a key in Redis
async fn redis_set(
    Path(name): Path<String>,
    Json(body): Json<RedisSetRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let ttl = body.ttl_seconds.map(std::time::Duration::from_secs);
    match client.set(&body.key, &body.value, ttl).await {
        Ok(()) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": body.key,
            "value": body.value,
            "ttl_seconds": body.ttl_seconds,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("SET failed: {}", e))),
    }
}

/// DEL request body
#[derive(Deserialize)]
struct RedisDelRequest {
    keys: Vec<String>,
}

/// DEL key(s) from Redis
async fn redis_del(
    Path(name): Path<String>,
    Json(body): Json<RedisDelRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let keys_ref: Vec<&str> = body.keys.iter().map(|s| s.as_str()).collect();
    match client.del(&keys_ref).await {
        Ok(count) => Json(ApiResponse::ok_body(serde_json::json!({
            "deleted": count,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("DEL failed: {}", e))),
    }
}

/// HSET request body
#[derive(Deserialize)]
struct RedisHSetRequest {
    key: String,
    field: String,
    value: String,
}

/// HSET in Redis
async fn redis_hset(
    Path(name): Path<String>,
    Json(body): Json<RedisHSetRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.hset(&body.key, &body.field, &body.value).await {
        Ok(count) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": body.key,
            "field": body.field,
            "added": count,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("HSET failed: {}", e))),
    }
}

/// HGET from Redis
async fn redis_hget(Path((name, key, field)): Path<(String, String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.hget(&key, &field).await {
        Ok(val) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": key,
            "field": field,
            "value": val,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("HGET failed: {}", e))),
    }
}

/// HGETALL from Redis
async fn redis_hgetall(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.hgetall(&key).await {
        Ok(map) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": key,
            "fields": map,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("HGETALL failed: {}", e))),
    }
}

/// RPUSH request body
#[derive(Deserialize)]
struct RedisRpushRequest {
    key: String,
    values: Vec<String>,
}

/// RPUSH to a Redis list
async fn redis_rpush(
    Path(name): Path<String>,
    Json(body): Json<RedisRpushRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.rpush(&body.key, body.values).await {
        Ok(len) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": body.key,
            "length": len,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("RPUSH failed: {}", e))),
    }
}

/// LPOP from a Redis list
async fn redis_lpop(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.lpop(&key, Some(1)).await {
        Ok(vals) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": key,
            "values": vals,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("LPOP failed: {}", e))),
    }
}

/// LLEN of a Redis list
async fn redis_llen(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.llen(&key).await {
        Ok(len) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": key,
            "length": len,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("LLEN failed: {}", e))),
    }
}

/// INCR a Redis key
async fn redis_incr(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.incr(&key).await {
        Ok(val) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": key,
            "value": val,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("INCR failed: {}", e))),
    }
}

/// Lock request body
#[derive(Deserialize)]
struct RedisLockRequest {
    key: String,
    ttl_seconds: Option<u64>,
    max_wait_seconds: Option<u64>,
}

/// Acquire and immediately release a distributed lock (tests the lock mechanism)
async fn redis_lock(
    Path(name): Path<String>,
    Json(body): Json<RedisLockRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let opts = crate::link_sys::providers::redis::LockOptions {
        ttl: std::time::Duration::from_secs(body.ttl_seconds.unwrap_or(5)),
        max_wait: std::time::Duration::from_secs(body.max_wait_seconds.unwrap_or(3)),
        retry_interval: std::time::Duration::from_millis(50),
    };
    let start = std::time::Instant::now();
    match client.lock(&body.key, opts).await {
        Ok(guard) => {
            let acquire_ms = std::time::Instant::now().saturating_duration_since(start).as_millis() as u64;
            // Explicitly release
            let released = guard.unlock().await.unwrap_or(false);
            Json(ApiResponse::ok_body(serde_json::json!({
                "key": body.key,
                "acquired": true,
                "acquire_ms": acquire_ms,
                "released": released,
            })))
        }
        Err(e) => Json(ApiResponse::err_body(format!("LOCK failed: {}", e))),
    }
}

// ==================== LinkSys Etcd Testing Endpoints ====================

/// Helper to get an Etcd client by name, returning error response if not found.
fn get_etcd_client_by_name(
    name: &str,
) -> Result<std::sync::Arc<crate::link_sys::providers::etcd::EtcdLinkClient>, Json<ApiResponse<serde_json::Value>>> {
    let key = name.replacen('_', "/", 1);
    crate::link_sys::get_etcd_client(&key).ok_or_else(|| {
        if crate::link_sys::runtime::store::etcd_runner_exists(&key) {
            Json(ApiResponse::err_body(format!(
                "Etcd client '{key}' not found (registered, not yet connected)"
            )))
        } else {
            Json(ApiResponse::err_body(format!(
                "Etcd client '{}' not found. Available: {:?}",
                key,
                crate::link_sys::runtime::store::list_etcd_clients()
            )))
        }
    })
}

/// List all registered Etcd clients
async fn etcd_list_clients() -> Json<ApiResponse<Vec<String>>> {
    let clients = crate::link_sys::runtime::store::list_etcd_clients();
    Json(ApiResponse::ok_body(clients))
}

/// Health check all Etcd clients
async fn etcd_health_all() -> Json<ApiResponse<Vec<crate::link_sys::LinkSysHealth>>> {
    let health = crate::link_sys::runtime::store::health_check_all_etcd().await;
    Json(ApiResponse::ok_body(health))
}

/// Health check a single Etcd client
async fn etcd_health_one(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let health = client.health_status().await;
    match serde_json::to_value(health) {
        Ok(v) => Json(ApiResponse::ok_body(v)),
        Err(e) => Json(ApiResponse::err_body(format!("Serialize error: {}", e))),
    }
}

/// PING an Etcd client (status check), returns latency in ms
async fn etcd_ping(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.ping().await {
        Ok(latency_ms) => Json(ApiResponse::ok_body(serde_json::json!({
            "latency_ms": latency_ms,
            "healthy": client.healthy(),
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("PING (status) failed: {}", e))),
    }
}

/// GET a key from Etcd
async fn etcd_get(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.get_string(&key).await {
        Ok(val) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": key,
            "value": val,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("GET failed: {}", e))),
    }
}

/// PUT request body
#[derive(Deserialize)]
struct EtcdPutRequest {
    key: String,
    value: String,
    lease_id: Option<i64>,
}

/// PUT a key in Etcd
async fn etcd_put(Path(name): Path<String>, Json(body): Json<EtcdPutRequest>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client
        .put(&body.key, body.value.as_bytes().to_vec(), body.lease_id)
        .await
    {
        Ok(()) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": body.key,
            "value": body.value,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("PUT failed: {}", e))),
    }
}

/// DELETE a key from Etcd
async fn etcd_delete(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.delete(&key).await {
        Ok(deleted) => Json(ApiResponse::ok_body(serde_json::json!({
            "key": key,
            "deleted": deleted,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("DELETE failed: {}", e))),
    }
}

/// GET with prefix from Etcd
async fn etcd_get_prefix(Path((name, prefix)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.get_prefix(&prefix).await {
        Ok(entries) => {
            let items: Vec<serde_json::Value> = entries
                .iter()
                .map(|(k, v, rev)| {
                    serde_json::json!({
                        "key": k,
                        "value": String::from_utf8_lossy(v),
                        "mod_revision": rev,
                    })
                })
                .collect();
            Json(ApiResponse::ok_body(serde_json::json!({
                "prefix": prefix,
                "count": items.len(),
                "entries": items,
            })))
        }
        Err(e) => Json(ApiResponse::err_body(format!("GET prefix failed: {}", e))),
    }
}

/// DELETE with prefix from Etcd
async fn etcd_delete_prefix(Path((name, prefix)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.delete_prefix(&prefix).await {
        Ok(deleted) => Json(ApiResponse::ok_body(serde_json::json!({
            "prefix": prefix,
            "deleted": deleted,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("DELETE prefix failed: {}", e))),
    }
}

/// Lease grant request body
#[derive(Deserialize)]
struct EtcdLeaseGrantRequest {
    ttl_seconds: i64,
}

/// Grant a lease
async fn etcd_lease_grant(
    Path(name): Path<String>,
    Json(body): Json<EtcdLeaseGrantRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.lease_grant(body.ttl_seconds).await {
        Ok(lease_id) => Json(ApiResponse::ok_body(serde_json::json!({
            "lease_id": lease_id,
            "ttl_seconds": body.ttl_seconds,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("LEASE GRANT failed: {}", e))),
    }
}

/// Revoke a lease
async fn etcd_lease_revoke(Path((name, lease_id)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let lease_id: i64 = match lease_id.parse() {
        Ok(id) => id,
        Err(_) => return Json(ApiResponse::err_body(format!("Invalid lease_id: {}", lease_id))),
    };
    match client.lease_revoke(lease_id).await {
        Ok(()) => Json(ApiResponse::ok_body(serde_json::json!({
            "lease_id": lease_id,
            "revoked": true,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("LEASE REVOKE failed: {}", e))),
    }
}

/// Get lease TTL
async fn etcd_lease_ttl(Path((name, lease_id)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let lease_id: i64 = match lease_id.parse() {
        Ok(id) => id,
        Err(_) => return Json(ApiResponse::err_body(format!("Invalid lease_id: {}", lease_id))),
    };
    match client.lease_time_to_live(lease_id).await {
        Ok(ttl) => Json(ApiResponse::ok_body(serde_json::json!({
            "lease_id": lease_id,
            "ttl": ttl,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("LEASE TTL failed: {}", e))),
    }
}

/// Lock request body
#[derive(Deserialize)]
struct EtcdLockRequest {
    name: String,
    ttl_seconds: Option<i64>,
    timeout_seconds: Option<u64>,
}

/// Acquire and immediately release an Etcd distributed lock (tests the lock mechanism)
async fn etcd_lock(
    Path(client_name): Path<String>,
    Json(body): Json<EtcdLockRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let ttl = body.ttl_seconds.unwrap_or(10);
    let timeout = std::time::Duration::from_secs(body.timeout_seconds.unwrap_or(5));
    let start = std::time::Instant::now();

    match client.try_lock(&body.name, ttl, timeout).await {
        Ok(Some(guard)) => {
            let acquire_ms = std::time::Instant::now().saturating_duration_since(start).as_millis() as u64;
            let released = guard.unlock().await.is_ok();
            Json(ApiResponse::ok_body(serde_json::json!({
                "name": body.name,
                "acquired": true,
                "acquire_ms": acquire_ms,
                "released": released,
            })))
        }
        Ok(None) => Json(ApiResponse::ok_body(serde_json::json!({
            "name": body.name,
            "acquired": false,
            "reason": "timeout",
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("LOCK failed: {}", e))),
    }
}

/// Get Etcd client info (name, namespace, health)
async fn etcd_info(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    Json(ApiResponse::ok_body(serde_json::json!({
        "name": client.name(),
        "namespace": client.namespace(),
        "healthy": client.healthy(),
    })))
}

// ==================== LinkSys Elasticsearch Testing Endpoints ====================

/// Helper to get an ES client by name, returning error response if not found.
fn get_es_client_by_name(
    name: &str,
) -> Result<std::sync::Arc<crate::link_sys::providers::elasticsearch::EsLinkClient>, Json<ApiResponse<serde_json::Value>>>
{
    let key = name.replacen('_', "/", 1);
    crate::link_sys::get_es_client(&key).ok_or_else(|| {
        if crate::link_sys::runtime::store::es_runner_exists(&key) {
            Json(ApiResponse::err_body(format!(
                "ES client '{key}' not found (registered, not yet connected)"
            )))
        } else {
            Json(ApiResponse::err_body(format!(
                "ES client '{}' not found. Available: {:?}",
                key,
                crate::link_sys::runtime::store::list_es_clients()
            )))
        }
    })
}

/// List all ES clients
async fn es_list_clients() -> Json<ApiResponse<serde_json::Value>> {
    let clients = crate::link_sys::runtime::store::list_es_clients();
    Json(ApiResponse::ok_body(serde_json::json!(clients)))
}

/// Health check all ES clients
async fn es_health_all() -> Json<ApiResponse<serde_json::Value>> {
    let results = crate::link_sys::runtime::store::health_check_all_es().await;
    Json(ApiResponse::ok_body(serde_json::json!(results)))
}

/// Health check a single ES client
async fn es_health_one(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let health = client.health_status().await;
    Json(ApiResponse::ok_body(serde_json::json!({
        "name": health.name,
        "connected": health.connected,
        "latency_ms": health.latency_ms,
        "error": health.error,
    })))
}

/// Ping ES client (cluster health + latency)
async fn es_ping(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.ping().await {
        Ok(latency_ms) => Json(ApiResponse::ok_body(serde_json::json!({
            "latency_ms": latency_ms,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("PING failed: {}", e))),
    }
}

/// Get ES client info (name, endpoints, healthy)
async fn es_info(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    Json(ApiResponse::ok_body(serde_json::json!({
        "name": client.name(),
        "endpoints": client.endpoints(),
        "healthy": client.healthy(),
    })))
}

/// Index a single document
async fn es_index_doc(
    Path((client_name, index)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.index_doc(&index, &body).await {
        Ok(doc_id) => Json(ApiResponse::ok_body(serde_json::json!({
            "index": index,
            "doc_id": doc_id,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("INDEX DOC failed: {}", e))),
    }
}

/// Get a document by ID
async fn es_get_doc(
    Path((client_name, index, doc_id)): Path<(String, String, String)>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.get_doc(&index, &doc_id).await {
        Ok(Some(source)) => Json(ApiResponse::ok_body(serde_json::json!({
            "index": index,
            "doc_id": doc_id,
            "source": source,
        }))),
        Ok(None) => Json(ApiResponse::ok_body(serde_json::json!({
            "index": index,
            "doc_id": doc_id,
            "source": null,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("GET DOC failed: {}", e))),
    }
}

/// Delete a document by ID
async fn es_delete_doc(
    Path((client_name, index, doc_id)): Path<(String, String, String)>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.delete_doc(&index, &doc_id).await {
        Ok(deleted) => Json(ApiResponse::ok_body(serde_json::json!({
            "index": index,
            "doc_id": doc_id,
            "deleted": deleted,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("DELETE DOC failed: {}", e))),
    }
}

/// Search request body
#[derive(Deserialize)]
struct EsSearchRequest {
    query: serde_json::Value,
}

/// Search documents
async fn es_search(
    Path((client_name, index)): Path<(String, String)>,
    Json(body): Json<EsSearchRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.search(&index, &body.query).await {
        Ok(result) => Json(ApiResponse::ok_body(serde_json::json!({
            "took": result.took,
            "timed_out": result.timed_out,
            "total": result.hits.total.value,
            "hits": result.hits.hits.iter().map(|h| serde_json::json!({
                "id": h.id,
                "score": h.score,
                "source": h.source,
            })).collect::<Vec<_>>(),
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("SEARCH failed: {}", e))),
    }
}

/// Create an index (no settings body needed for basic creation)
async fn es_create_index(Path((client_name, index)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.create_index(&index, None).await {
        Ok(()) => Json(ApiResponse::ok_body(serde_json::json!({
            "index": index,
            "created": true,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("CREATE INDEX failed: {}", e))),
    }
}

/// Delete an index
async fn es_delete_index(Path((client_name, index)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.delete_index(&index).await {
        Ok(()) => Json(ApiResponse::ok_body(serde_json::json!({
            "index": index,
            "deleted": true,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("DELETE INDEX failed: {}", e))),
    }
}

/// Check if an index exists
async fn es_index_exists(Path((client_name, index)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.index_exists(&index).await {
        Ok(exists) => Json(ApiResponse::ok_body(serde_json::json!({
            "index": index,
            "exists": exists,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("INDEX EXISTS failed: {}", e))),
    }
}

/// Refresh an index (make recent writes visible to search)
async fn es_refresh_index(Path((client_name, index)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.refresh_index(&index).await {
        Ok(()) => Json(ApiResponse::ok_body(serde_json::json!({
            "index": index,
            "refreshed": true,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("REFRESH failed: {}", e))),
    }
}

/// Count documents in an index
async fn es_count(Path((client_name, index)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.count(&index, None).await {
        Ok(count) => Json(ApiResponse::ok_body(serde_json::json!({
            "index": index,
            "count": count,
        }))),
        Err(e) => Json(ApiResponse::err_body(format!("COUNT failed: {}", e))),
    }
}

/// Bulk send request body
#[derive(Deserialize)]
struct EsBulkSendRequest {
    docs: Vec<serde_json::Value>,
}

/// Send documents to bulk ingest buffer
async fn es_bulk_send(
    Path(client_name): Path<String>,
    Json(body): Json<EsBulkSendRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let count = body.docs.len();
    let mut errors = 0;
    for doc in body.docs {
        let doc_str = serde_json::to_string(&doc).unwrap_or_default();
        if let Err(e) = client.send_bulk(doc_str).await {
            tracing::warn!("ES bulk send error: {}", e);
            errors += 1;
        }
    }
    Json(ApiResponse::ok_body(serde_json::json!({
        "sent": count - errors,
        "errors": errors,
    })))
}

/// Serve the admin API on the specified listen address with graceful shutdown support.
///
/// `admin_token` is the resolved shared bearer token (env > config), or `None`
/// when unconfigured (the admin API then fails closed with 503).
pub async fn serve_with_shutdown(
    config_client: Arc<ConfigClient>,
    listen_addr: &str,
    admin_token: Option<String>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let app = create_admin_router(config_client, admin_token);
    let addr: std::net::SocketAddr = listen_addr.parse()?;

    tracing::info!(
        component = "admin_api_gateway",
        event = "server_starting",
        addr = %addr,
        integration_testing = edgion_common::test_mode::is_integration_testing_mode(),
        "Gateway Admin API server listening"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await?;

    tracing::info!(component = "admin_api_gateway", "Admin API server stopped");
    Ok(())
}

/// Serve the probe router (liveness/readiness) on the given listen address with
/// graceful shutdown. Structurally identical to `serve_with_shutdown` but mounts
/// only the non-sensitive probe endpoints.
pub async fn serve_probe_with_shutdown(
    config_client: Arc<ConfigClient>,
    listen_addr: &str,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let app = create_probe_router(config_client);
    let addr: std::net::SocketAddr = listen_addr.parse()?;

    tracing::info!(
        component = "probe_api_gateway",
        event = "server_starting",
        addr = %addr,
        "Gateway probe API server listening"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await?;

    tracing::info!(component = "probe_api_gateway", "Probe API server stopped");
    Ok(())
}

#[cfg(test)]
mod edgion_tls_scrub_tests {
    use super::*;
    use edgion_resources::resources::common::SecretObjectReference;
    use edgion_resources::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode, EdgionTls, EdgionTlsSpec};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;
    use std::collections::BTreeMap;

    /// Build a `Secret` carrying a fake `tls.key` private key, mirroring what the
    /// Controller resolves into the gateway cache.
    fn tls_secret(key_bytes: &[u8]) -> Secret {
        let mut data = BTreeMap::new();
        data.insert("tls.crt".to_string(), ByteString(b"cert".to_vec()));
        data.insert("tls.key".to_string(), ByteString(key_bytes.to_vec()));
        Secret {
            data: Some(data),
            ..Default::default()
        }
    }

    fn edgion_tls_with_secrets() -> EdgionTls {
        EdgionTls {
            metadata: ObjectMeta {
                name: Some("tls-cert".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: EdgionTlsSpec {
                current_status: None,
                parent_refs: None,
                hosts: vec!["example.com".to_string()],
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "tls-cert".to_string(),
                    namespace: None,
                },
                client_auth: Some(ClientAuthConfig {
                    mode: ClientAuthMode::Mutual,
                    ca_secret_ref: None,
                    verify_depth: 1,
                    ca_secret: Some(tls_secret(b"CA-PRIVATE-KEY")),
                    allowed_sans: None,
                    allowed_cns: None,
                }),
                min_tls_version: None,
                ciphers: None,
                secret: Some(tls_secret(b"LEAF-PRIVATE-KEY")),
                resolved_listeners: None,
                resolved_listener_attachments: None,
                resolved_listener_attachment_index: Default::default(),
                resolved_log_labels: None,
            },
            status: None,
        }
    }

    /// The admin API must redact resolved private-key material, whether listed or
    /// fetched by name. Both `spec.secret` (leaf key) and `spec.client_auth.ca_secret`
    /// (mTLS CA key) must serialize as `"[redacted]"` through the guarded path.
    #[test]
    fn admin_serialization_redacts_private_keys() {
        let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
        let _enter = rt.enter();
        let client = ConfigClient::new("cid".to_string(), "cname".to_string());
        client.edgion_tls().reset(vec![edgion_tls_with_secrets()], 1);

        // List path: guard lives inside list_client_resources.
        let list = list_client_resources(&client, ResourceKind::EdgionTls);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["spec"]["secret"], "[redacted]", "leaf key not redacted");
        assert_eq!(
            list[0]["spec"]["clientAuth"]["caSecret"], "[redacted]",
            "CA key not redacted"
        );
        let serialized = serde_json::to_string(&list[0]).unwrap();
        assert!(!serialized.contains("tls.key"), "tls.key leaked: {serialized}");

        // Get-by-name path reuses list_client_resources, so the same guard applies.
        let got = get_client_resource(&client, ResourceKind::EdgionTls, Some("default"), "tls-cert")
            .expect("resource should be found");
        assert_eq!(got["spec"]["secret"], "[redacted]", "leaf key not redacted via get");
        assert_eq!(
            got["spec"]["clientAuth"]["caSecret"], "[redacted]",
            "CA key not redacted via get"
        );

        // Non-secret fields must survive so the endpoint stays useful.
        assert_eq!(got["spec"]["hosts"][0], "example.com");
    }
}

#[cfg(test)]
mod edgion_backend_admin_tests {
    use super::*;
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    use edgion_resources::resources::edgion_backend::{
        AiBackendDefaults, AiBackendSpec, AiCredential, AiCredentialPool, AiModel, AiProvider, EdgionBackend,
        EdgionBackendSpec,
    };
    use edgion_resources::resources::edgion_plugins::OnRedisFailure;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    fn ai_backend(name: &str, endpoint: &str, credential: Option<&[u8]>) -> EdgionBackend {
        let secret = credential.map(|value| Secret {
            data: Some(BTreeMap::from([("api-key".to_string(), ByteString(value.to_vec()))])),
            ..Default::default()
        });
        let spec = EdgionBackendSpec {
            current_status: None,
            ai: AiBackendSpec {
                provider: AiProvider::OpenAICompatible,
                endpoint: Some(endpoint.to_string()),
                allow_insecure_http: false,
                credential_pool: AiCredentialPool {
                    redis_ref: None,
                    on_redis_failure: OnRedisFailure::Deny,
                    credentials: vec![AiCredential {
                        name: "primary".to_string(),
                        weight: 1,
                        credential: SecretSlot {
                            secret_ref: SecretObjectReference {
                                group: None,
                                kind: None,
                                name: "provider-key".to_string(),
                                namespace: None,
                            },
                            secret,
                        },
                        limits: None,
                    }],
                },
                models: vec![AiModel {
                    name: "model-a".to_string(),
                    aliases: vec![],
                    public: true,
                    pricing: None,
                }],
                defaults: AiBackendDefaults::default(),
            },
            reconcile_outcome: None,
        };
        let mut service = EdgionBackend::new(name, spec);
        service.metadata.namespace = Some("ai-admin-test".to_string());
        service
    }

    #[test]
    fn configclient_redacts_ai_credential_and_retains_inactive_resources() {
        let _store_guard = crate::ai::lock_global_ai_backend_store_for_test();
        let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
        let _enter = rt.enter();
        let client = ConfigClient::new("cid-ai".to_string(), "cname-ai".to_string());
        let mut active = ai_backend("active", "https://ai.example.com/v1", Some(b"admin-private-key"));
        let mut secondary = active.spec.ai.credential_pool.credentials[0].clone();
        secondary.name = "secondary".to_string();
        secondary.credential.secret = Some(Secret {
            data: Some(BTreeMap::from([(
                "api-key".to_string(),
                ByteString(b"secondary-private-key".to_vec()),
            )])),
            ..Default::default()
        });
        active.spec.ai.credential_pool.credentials.push(secondary);

        client.edgion_backends().reset(
            vec![
                active,
                ai_backend("unresolved", "https://ai.example.com/v1", None),
                ai_backend("invalid", "http://ai.example.com/v1", Some(b"invalid-private-key")),
            ],
            1,
        );

        let listed = list_client_resources(&client, ResourceKind::EdgionBackend);
        assert_eq!(
            listed.len(),
            3,
            "ClientCache must retain inactive resources for Admin visibility"
        );
        let active = listed
            .iter()
            .find(|value| value["metadata"]["name"] == "active")
            .expect("active resource listed");
        assert_eq!(
            active["spec"]["ai"]["credentialPool"]["credentials"][0]["secret"],
            "[redacted]"
        );
        assert_eq!(
            active["spec"]["ai"]["credentialPool"]["credentials"][1]["secret"],
            "[redacted]"
        );
        let serialized = serde_json::to_string(&listed).unwrap();
        assert!(!serialized.contains("admin-private-key"));
        assert!(!serialized.contains("secondary-private-key"));
        assert!(!serialized.contains("invalid-private-key"));

        let runtime = crate::ai::get_global_ai_backend_store();
        assert_eq!(runtime.count(), 1, "only the valid, resolved resource is activatable");
        assert!(runtime.get_by_key("ai-admin-test/active").is_some());
        assert!(runtime.get_by_key("ai-admin-test/unresolved").is_none());
        assert!(runtime.get_by_key("ai-admin-test/invalid").is_none());

        client.edgion_backends().reset(Vec::new(), 2);
        assert_eq!(runtime.count(), 0, "wipe must clear the derived runtime store");
    }
}

#[cfg(test)]
mod store_stats_tests {
    use super::*;
    use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};

    #[test]
    fn ai_runtime_store_collector_reports_populated_stores() {
        let credential_outliers = crate::ai::AiCredentialOutlierStore::new();
        let adaptive_telemetry = crate::ai::AdaptiveTelemetryStore::new();
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "default".to_string(),
            name: "provider".to_string(),
        };
        credential_outliers.configure_backend(target.clone(), None, ["primary".to_string()]);
        let telemetry_key = crate::ai::AdaptiveTelemetryKey::new(
            "route-uid".to_string(),
            "rule".to_string(),
            target,
            "model".to_string(),
        )
        .unwrap();
        adaptive_telemetry.reconcile_keys(&std::collections::HashSet::from([telemetry_key]));

        let (credential_stats, telemetry_stats) =
            collect_ai_runtime_store_counts(&credential_outliers, &adaptive_telemetry);
        assert_eq!(credential_stats.count, 1);
        assert_eq!(telemetry_stats.count, 1);
    }

    #[test]
    fn store_stats_response_serializes() {
        let resp = StoreStatsResponse {
            wasm_shared_data_metrics: Default::default(),
            plugin_publication: Default::default(),
            plugin_publication_inventory: Default::default(),
            wasm_oci_cache: crate::plugins::http::wasm::oci::cache::OciMemoryCacheStats {
                entries: 0,
                keys: Vec::new(),
                retained_bytes: 0,
            },
            http_routes: crate::routes::http::HttpRouteManagerStats {
                exact_domains: 0,
                wildcard_domains: 0,
                has_catch_all: false,
                http_routes: 0,
                listener_count: 0,
            },
            grpc_routes: crate::routes::grpc::GrpcRouteManagerStats {
                grpc_routes: 0,
                resource_keys: 0,
                route_units_cache: 0,
                listener_count: 0,
            },
            tcp_routes: crate::routes::tcp::TcpRouteManagerStats {
                route_cache: 0,
                listener_count: 0,
                total_routes_in_tables: 0,
            },
            udp_routes: crate::routes::udp::UdpRouteManagerStats {
                route_cache: 0,
                listener_count: 0,
                total_routes_in_tables: 0,
            },
            tls_routes: crate::routes::tls::TlsRouteManagerStats {
                route_cache: 0,
                listener_count: 0,
                total_candidate_entries_in_tables: 0,
            },
            gateway_config: GatewayConfigStats { gateways: 0 },
            listener_gateway_info: crate::runtime::store::listener_gateway_info::ListenerGatewayInfoStats {
                listener_count: 0,
                total_entries: 0,
            },
            gateway_tls_matcher: crate::runtime::matching::tls::GatewayTlsMatcherStats {
                listener_count: 0,
                is_empty: true,
            },
            tls_store: TlsStoreStats {
                entries: 0,
                valid: 0,
                invalid: 0,
            },
            tls_cert_matcher: TlsCertMatcherStats { listener_count: 0 },
            link_sys_store: LinkSysStoreStats { resources: 0 },
            lb_policies: std::collections::BTreeMap::new(),
            backend_tls_policy: crate::backends::policy::BackendTLSPolicyStoreStats {
                policies: 0,
                reverse_index_targets: 0,
                invalid_index_targets: 0,
            },
            gateway_class_store: SimpleCountStats { count: 0 },
            edgion_gateway_config_store: SimpleCountStats { count: 0 },
            egc_bindings: Default::default(),
            client_caches: ClientCachesStats {
                by_kind: std::collections::BTreeMap::new(),
            },
            service_discovery: crate::backends::discovery::ServiceDiscoveryStatsSnapshot::default(),
            health_status_store: SimpleCountStats { count: 0 },
            outlier_detection_store: SimpleCountStats { count: 0 },
            health_check_config_store: SimpleCountStats { count: 0 },
            gateway_backend_tls_store: SimpleCountStats { count: 0 },
            edgion_config_data_store: SimpleCountStats { count: 0 },
            edgion_backend_store: EdgionBackendStoreStats {
                count: 0,
                keys: Vec::new(),
            },
            ai_credential_outlier_store: SimpleCountStats { count: 0 },
            adaptive_telemetry_store: SimpleCountStats { count: 0 },
            link_sys_runtime: LinkSysRuntimeStats {
                count: 0,
                keys: Vec::new(),
                redis_clients: 0,
                etcd_clients: 0,
                es_clients: 0,
                kafka_clients: 0,
                webhook_clients: 0,
                otlp_clients: 0,
                otlp_retiring_clients: 0,
            },
            plugin_store_v2: PluginStoreStatsV2 {
                total: 0,
                by_kind: std::collections::BTreeMap::new(),
            },
            stream_plugin_store_v2: PluginStoreStatsV2 {
                total: 0,
                by_kind: std::collections::BTreeMap::new(),
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.is_object());
        assert_eq!(json["wasm_oci_cache"]["entries"], 0);
        assert_eq!(json["wasm_oci_cache"]["retained_bytes"], 0);
        assert_eq!(json["wasm_oci_cache"]["keys"], serde_json::json!([]));
        assert_eq!(json["tls_routes"]["total_candidate_entries_in_tables"], 0);
        assert_eq!(json["udp_routes"]["total_routes_in_tables"], 0);
        assert!(json.get("client_caches").is_some());
        assert!(json.get("service_discovery").is_some());
        assert!(json.get("service_store").is_none());
        assert!(json.get("ep_slice_store").is_none());
        assert!(json.get("edgion_backend_store").is_some());
        assert_eq!(json["ai_credential_outlier_store"]["count"], 0);
        assert_eq!(json["adaptive_telemetry_store"]["count"], 0);
        assert_eq!(json["link_sys_runtime"]["otlp_clients"], 0);
        assert_eq!(json["link_sys_runtime"]["count"], 0);
        assert_eq!(json["link_sys_runtime"]["keys"], serde_json::json!([]));
        assert_eq!(json["link_sys_runtime"]["kafka_clients"], 0);
        assert_eq!(json["link_sys_runtime"]["webhook_clients"], 0);
        assert_eq!(json["link_sys_runtime"]["otlp_retiring_clients"], 0);
        assert!(json.get("plugin_store_v2").is_some());
        assert!(json.get("stream_plugin_store_v2").is_some());
        assert!(json.get("plugin_store").is_none());
        assert!(json.get("stream_plugin_store").is_none());
    }
}

#[cfg(test)]
mod router_shape_tests {
    use super::*;
    use crate::conf_sync::ConfigClient;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // for `oneshot`

    /// Token used by the admin-router shape tests. The admin router is now
    /// auth-guarded, so these routing-presence tests configure a token and send
    /// it on every request — letting the request reach (or miss) the route so we
    /// assert routing, not auth. The probe router ignores the header.
    const TEST_ADMIN_TOKEN: &str = "router-shape-test-admin-token-0123456789";

    fn test_client() -> Arc<ConfigClient> {
        Arc::new(ConfigClient::new("test".to_string(), "test".to_string()))
    }

    async fn status_of(router: Router, path: &str) -> StatusCode {
        router
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {TEST_ADMIN_TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn probe_router_serves_health_and_ready() {
        let r = create_probe_router(test_client());
        assert_eq!(status_of(r.clone(), "/health").await, StatusCode::OK);
        let ready = status_of(r, "/ready").await;
        assert!(ready == StatusCode::OK || ready == StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn policy_fatal_overrides_ready_config_cache() {
        assert_eq!(readiness_status(true, false), StatusCode::OK);
        assert_eq!(readiness_status(true, true), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(readiness_status(false, false), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn admin_router_drops_health_and_ready_but_keeps_configclient() {
        let r = create_admin_router(test_client(), Some(TEST_ADMIN_TOKEN.to_string()));
        assert_eq!(status_of(r.clone(), "/health").await, StatusCode::NOT_FOUND);
        assert_eq!(status_of(r.clone(), "/ready").await, StatusCode::NOT_FOUND);
        assert_eq!(status_of(r, "/configclient/httproute/list").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_router_drops_server_info_keeps_configclient() {
        // Production admin router (integration testing mode off in unit tests):
        // server-info is testing-only and must be absent; /configclient stays mounted.
        let r = create_admin_router(test_client(), Some(TEST_ADMIN_TOKEN.to_string()));
        assert_eq!(status_of(r.clone(), "/api/v1/server-info").await, StatusCode::NOT_FOUND);
        assert_eq!(status_of(r, "/configclient/httproute/list").await, StatusCode::OK);
    }
}
