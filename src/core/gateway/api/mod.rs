use crate::core::gateway::conf_sync::ConfigClient;
use crate::types::ResourceKind;
use axum::{
    extract::{Path, Query, State},
    response::Json,
    routing::get,
    Router,
};
use kube::ResourceExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
    fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    fn error(message: String) -> Self {
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
    Json(ApiResponse::success("OK".to_string()))
}

/// Readiness check endpoint - returns OK only when all caches are ready
async fn readiness_check(State(client): State<Arc<ConfigClient>>) -> axum::http::StatusCode {
    match client.is_ready() {
        Ok(()) => axum::http::StatusCode::OK,
        Err(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Get server info including the current server_id from Controller
///
/// This returns the server_id that Gateway received from Controller.
/// After a reload, this value should change to match the Controller's new server_id.
async fn get_server_info(State(client): State<Arc<ConfigClient>>) -> Json<ApiResponse<ServerInfoResponse>> {
    let server_id = client.current_server_id();
    let ready = client.is_ready().is_ok();
    Json(ApiResponse::success(ServerInfoResponse { server_id, ready }))
}

/// Helper macro to list all resources from ConfigClient
macro_rules! list_client_resources {
    ($client:expr, $kind:expr) => {{
        match $kind {
            ResourceKind::HTTPRoute => to_json_vec($client.routes().list().data),
            ResourceKind::GRPCRoute => to_json_vec($client.grpc_routes().list().data),
            ResourceKind::TCPRoute => to_json_vec($client.tcp_routes().list().data),
            ResourceKind::UDPRoute => to_json_vec($client.udp_routes().list().data),
            ResourceKind::TLSRoute => to_json_vec($client.tls_routes().list().data),
            ResourceKind::Service => to_json_vec($client.services().list().data),
            ResourceKind::EndpointSlice => to_json_vec($client.endpoint_slices().list().data),
            ResourceKind::Endpoint => to_json_vec($client.endpoints().list().data),
            ResourceKind::EdgionTls => to_json_vec($client.edgion_tls().list().data),
            ResourceKind::EdgionPlugins => to_json_vec($client.edgion_plugins().list().data),
            ResourceKind::EdgionStreamPlugins => to_json_vec($client.edgion_stream_plugins().list().data),
            ResourceKind::ReferenceGrant => vec![], // ReferenceGrant not synced to Gateway
            ResourceKind::BackendTLSPolicy => to_json_vec($client.backend_tls_policies().list().data),
            ResourceKind::PluginMetaData => to_json_vec($client.plugin_metadata().list().data),
            ResourceKind::LinkSys => to_json_vec($client.link_sys().list().data),
            ResourceKind::GatewayClass => to_json_vec($client.gateway_classes().list().data),
            ResourceKind::Gateway => to_json_vec($client.gateways().list().data),
            ResourceKind::EdgionGatewayConfig => to_json_vec($client.edgion_gateway_configs().list().data),
            _ => vec![],
        }
    }};
}

/// Helper macro to get a single resource from ConfigClient
macro_rules! get_client_resource {
    ($client:expr, $kind:expr, $ns:expr, $name:expr) => {{
        let ns_opt = $ns.as_deref();
        let name_str = $name.as_str();
        match $kind {
            ResourceKind::HTTPRoute => find_resource($client.routes().list().data, ns_opt, name_str),
            ResourceKind::GRPCRoute => find_resource($client.grpc_routes().list().data, ns_opt, name_str),
            ResourceKind::TCPRoute => find_resource($client.tcp_routes().list().data, ns_opt, name_str),
            ResourceKind::UDPRoute => find_resource($client.udp_routes().list().data, ns_opt, name_str),
            ResourceKind::TLSRoute => find_resource($client.tls_routes().list().data, ns_opt, name_str),
            ResourceKind::Service => find_resource($client.services().list().data, ns_opt, name_str),
            ResourceKind::EndpointSlice => find_resource($client.endpoint_slices().list().data, ns_opt, name_str),
            ResourceKind::Endpoint => find_resource($client.endpoints().list().data, ns_opt, name_str),
            ResourceKind::EdgionTls => find_resource($client.edgion_tls().list().data, ns_opt, name_str),
            ResourceKind::EdgionPlugins => find_resource_alt($client.edgion_plugins().list().data, ns_opt, name_str),
            ResourceKind::EdgionStreamPlugins => {
                find_resource_alt($client.edgion_stream_plugins().list().data, ns_opt, name_str)
            }
            ResourceKind::ReferenceGrant => None, // ReferenceGrant not synced to Gateway
            ResourceKind::BackendTLSPolicy => {
                find_resource_alt($client.backend_tls_policies().list().data, ns_opt, name_str)
            }
            ResourceKind::PluginMetaData => find_resource($client.plugin_metadata().list().data, ns_opt, name_str),
            ResourceKind::LinkSys => find_resource($client.link_sys().list().data, ns_opt, name_str),
            ResourceKind::GatewayClass => find_cluster_resource($client.gateway_classes().list().data, name_str),
            ResourceKind::Gateway => find_resource($client.gateways().list().data, ns_opt, name_str),
            ResourceKind::EdgionGatewayConfig => {
                find_cluster_resource($client.edgion_gateway_configs().list().data, name_str)
            }
            _ => None,
        }
    }};
}

/// Convert a list of resources to JSON values
fn to_json_vec<T: serde::Serialize>(items: Vec<T>) -> Vec<serde_json::Value> {
    items
        .into_iter()
        .filter_map(|item| serde_json::to_value(item).ok())
        .collect()
}

/// Find a namespaced resource by namespace and name (returns Option<&str> for namespace)
fn find_resource<T: ResourceExt + serde::Serialize>(
    items: Vec<T>,
    namespace: Option<&str>,
    name: &str,
) -> Option<serde_json::Value> {
    items
        .into_iter()
        .find(|r| r.name_any() == name && r.namespace().as_deref() == namespace)
        .and_then(|r| serde_json::to_value(r).ok())
}

/// Find a namespaced resource (alternative namespace() return type - returns Option<String>)
fn find_resource_alt<T: ResourceExt + serde::Serialize>(
    items: Vec<T>,
    namespace: Option<&str>,
    name: &str,
) -> Option<serde_json::Value> {
    items
        .into_iter()
        .find(|r| r.name_any() == name && r.namespace().as_deref() == namespace)
        .and_then(|r| serde_json::to_value(r).ok())
}

/// Find a cluster-scoped resource by name only
fn find_cluster_resource<T: ResourceExt + serde::Serialize>(items: Vec<T>, name: &str) -> Option<serde_json::Value> {
    items
        .into_iter()
        .find(|r| r.name_any() == name)
        .and_then(|r| serde_json::to_value(r).ok())
}

/// Dynamic list endpoint - list all resources of a kind
async fn list_resources(State(client): State<Arc<ConfigClient>>, Path(kind_str): Path<String>) -> Json<ListResponse> {
    let kind = ResourceKind::from_kind_name(&kind_str).unwrap_or(ResourceKind::Unspecified);
    let data = list_client_resources!(&client, kind);
    Json(ListResponse::success(data))
}

/// Dynamic get endpoint - get a single resource by namespace and name
async fn get_resource(
    State(client): State<Arc<ConfigClient>>,
    Path(kind_str): Path<String>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<serde_json::Value>> {
    let Some(name) = query.name else {
        return Json(ApiResponse::error("Missing required parameter: name".to_string()));
    };

    let kind = ResourceKind::from_kind_name(&kind_str).unwrap_or(ResourceKind::Unspecified);
    let resource = get_client_resource!(&client, kind, query.namespace, name);

    match resource {
        Some(r) => Json(ApiResponse::success(r)),
        None => Json(ApiResponse::error(format!("{} not found", kind_str))),
    }
}

/// Create the admin API router with dynamic endpoints
pub fn create_admin_router(config_client: Arc<ConfigClient>) -> Router {
    let mut router = Router::new()
        .route("/health", get(health_check))
        .route("/ready", get(readiness_check))
        // Server info endpoint (for reload testing)
        .route("/api/v1/server-info", get(get_server_info))
        // Dynamic endpoints for all resource types
        .route("/configclient/{kind}", get(get_resource))
        .route("/configclient/{kind}/list", get(list_resources))
        .with_state(config_client);

    // Add testing endpoints only when integration testing mode is enabled
    if crate::core::common::config::is_integration_testing_mode() {
        router = router.merge(create_testing_router());
    }

    router
}

// ==================== Integration Testing Endpoints ====================

/// Testing status response
#[derive(Serialize)]
struct TestingStatusResponse {
    integration_testing_mode: bool,
    access_log_store: crate::core::gateway::observe::access_log_store::AccessLogStoreStatus,
}

/// Get testing subsystem status
async fn testing_status() -> Json<ApiResponse<TestingStatusResponse>> {
    let store = crate::core::gateway::observe::access_log_store::get_access_log_store();
    Json(ApiResponse::success(TestingStatusResponse {
        integration_testing_mode: true,
        access_log_store: store.status(),
    }))
}

/// Get access log by trace_id
async fn get_access_log(Path(trace_id): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let store = crate::core::gateway::observe::access_log_store::get_access_log_store();
    match store.get(&trace_id) {
        Some(json) => {
            // Parse JSON string back to Value
            match serde_json::from_str::<serde_json::Value>(&json) {
                Ok(value) => Json(ApiResponse::success(value)),
                Err(_) => Json(ApiResponse::success(serde_json::Value::String(json))),
            }
        }
        None => Json(ApiResponse::error(format!(
            "Access log not found for trace_id: {}",
            trace_id
        ))),
    }
}

/// Delete access log by trace_id
async fn delete_access_log(Path(trace_id): Path<String>) -> Json<ApiResponse<String>> {
    let store = crate::core::gateway::observe::access_log_store::get_access_log_store();
    if store.delete(&trace_id) {
        Json(ApiResponse::success("Deleted".to_string()))
    } else {
        Json(ApiResponse::error(format!(
            "Access log not found for trace_id: {}",
            trace_id
        )))
    }
}

/// List all stored access logs
async fn list_access_logs() -> Json<crate::core::gateway::observe::access_log_store::AccessLogListResponse> {
    let store = crate::core::gateway::observe::access_log_store::get_access_log_store();
    let items = store.list();
    Json(crate::core::gateway::observe::access_log_store::AccessLogListResponse {
        success: true,
        count: items.len(),
        data: items,
    })
}

/// Delete all stored access logs
async fn clear_access_logs() -> Json<ApiResponse<String>> {
    let store = crate::core::gateway::observe::access_log_store::get_access_log_store();
    store.clear();
    Json(ApiResponse::success("All access logs cleared".to_string()))
}

// ==================== Debug Store Stats Endpoint ====================

/// Response for /api/v1/debug/store-stats
#[derive(Serialize)]
struct StoreStatsResponse {
    http_routes: crate::core::gateway::routes::http::HttpRouteManagerStats,
    grpc_routes: crate::core::gateway::routes::grpc::GrpcRouteManagerStats,
    tcp_routes: crate::core::gateway::routes::tcp::TcpRouteManagerStats,
    udp_routes: crate::core::gateway::routes::udp::UdpRouteManagerStats,
    tls_routes: crate::core::gateway::routes::tls::TlsRouteManagerStats,
    gateway_config: GatewayConfigStats,
    port_gateway_info: crate::core::gateway::runtime::store::port_gateway_info::PortGatewayInfoStats,
    gateway_tls_matcher: crate::core::gateway::runtime::matching::tls::GatewayTlsMatcherStats,
    tls_store: TlsStoreStats,
    tls_cert_matcher: TlsCertMatcherStats,
    plugin_store: PluginStoreStats,
    stream_plugin_store: StreamPluginStoreStats,
    link_sys_store: LinkSysStoreStats,
    policy_store: crate::core::gateway::lb::lb_policy::PolicyStoreStats,
    backend_tls_policy: crate::core::gateway::backends::policy::BackendTLSPolicyStoreStats,
    gateway_class_store: SimpleCountStats,
    edgion_gateway_config_store: SimpleCountStats,
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
    port_count: usize,
}

#[derive(Serialize)]
struct PluginStoreStats {
    plugins: usize,
}

#[derive(Serialize)]
struct StreamPluginStoreStats {
    plugins: usize,
}

#[derive(Serialize)]
struct LinkSysStoreStats {
    resources: usize,
}

#[derive(Serialize)]
struct SimpleCountStats {
    count: usize,
}

/// Collect statistics from all derived stores for config-sync leak detection.
async fn store_stats() -> Json<ApiResponse<StoreStatsResponse>> {
    let http_routes = crate::core::gateway::routes::http::get_global_route_manager().stats();
    let grpc_routes = crate::core::gateway::routes::grpc::get_global_grpc_route_manager().stats();
    let tcp_routes = crate::core::gateway::routes::tcp::get_global_tcp_route_managers().stats();
    let udp_routes = crate::core::gateway::routes::udp::get_global_udp_route_managers().stats();
    let tls_routes = crate::core::gateway::routes::tls::get_global_tls_route_managers().stats();

    let gateway_config = GatewayConfigStats {
        gateways: crate::core::gateway::runtime::store::config::get_global_gateway_config_store().gateway_count(),
    };
    let port_gateway_info =
        crate::core::gateway::runtime::store::port_gateway_info::get_port_gateway_info_store().stats();
    let gateway_tls_matcher = crate::core::gateway::runtime::matching::tls::get_gateway_tls_matcher().stats();

    let tls = crate::core::gateway::tls::store::tls_store::get_global_tls_store();
    let (valid, invalid) = tls.get_cert_stats();
    let tls_store = TlsStoreStats {
        entries: tls.entry_count(),
        valid,
        invalid,
    };

    let tls_cert_matcher = TlsCertMatcherStats {
        port_count: crate::core::gateway::tls::store::cert_matcher::get_tls_cert_matcher().port_count(),
    };

    let plugin_store = PluginStoreStats {
        plugins: crate::core::gateway::plugins::http::get_global_plugin_store().count(),
    };

    let stream_plugin_store = StreamPluginStoreStats {
        plugins: crate::core::gateway::plugins::stream::get_global_stream_plugin_store().count(),
    };

    let link_sys_store = LinkSysStoreStats {
        resources: crate::core::gateway::link_sys::runtime::store::get_global_link_sys_store().count(),
    };

    let policy_store = crate::core::gateway::lb::lb_policy::get_global_policy_store().stats();

    let backend_tls_policy = crate::core::gateway::backends::policy::get_global_backend_tls_policy_store().stats();

    let gateway_class_store = SimpleCountStats {
        count: crate::core::gateway::config::gateway_class::get_gateway_class_store()
            .read()
            .unwrap()
            .len(),
    };
    let edgion_gateway_config_store = SimpleCountStats {
        count: crate::core::gateway::config::edgion_gateway::get_edgion_gateway_config_store()
            .read()
            .unwrap()
            .len(),
    };

    Json(ApiResponse::success(StoreStatsResponse {
        http_routes,
        grpc_routes,
        tcp_routes,
        udp_routes,
        tls_routes,
        gateway_config,
        port_gateway_info,
        gateway_tls_matcher,
        tls_store,
        tls_cert_matcher,
        plugin_store,
        stream_plugin_store,
        link_sys_store,
        policy_store,
        backend_tls_policy,
        gateway_class_store,
        edgion_gateway_config_store,
    }))
}

/// Create testing-specific router (only added when integration testing mode is enabled)
fn create_testing_router() -> Router {
    Router::new()
        .route("/api/v1/testing/status", get(testing_status))
        .route("/api/v1/debug/store-stats", get(store_stats))
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
) -> Result<
    std::sync::Arc<crate::core::gateway::link_sys::providers::redis::RedisLinkClient>,
    Json<ApiResponse<serde_json::Value>>,
> {
    // URL path uses underscore as separator: "default_redis-main" → "default/redis-main"
    let key = name.replacen('_', "/", 1);
    crate::core::gateway::link_sys::get_redis_client(&key).ok_or_else(|| {
        Json(ApiResponse::error(format!(
            "Redis client '{}' not found. Available: {:?}",
            key,
            crate::core::gateway::link_sys::runtime::store::list_redis_clients()
        )))
    })
}

/// List all registered Redis clients
async fn redis_list_clients() -> Json<ApiResponse<Vec<String>>> {
    let clients = crate::core::gateway::link_sys::runtime::store::list_redis_clients();
    Json(ApiResponse::success(clients))
}

/// Health check all Redis clients
async fn redis_health_all() -> Json<ApiResponse<Vec<crate::core::gateway::link_sys::providers::redis::LinkSysHealth>>> {
    let health = crate::core::gateway::link_sys::runtime::store::health_check_all_redis().await;
    Json(ApiResponse::success(health))
}

/// Health check a single Redis client
async fn redis_health_one(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let health = client.health_status().await;
    match serde_json::to_value(health) {
        Ok(v) => Json(ApiResponse::success(v)),
        Err(e) => Json(ApiResponse::error(format!("Serialize error: {}", e))),
    }
}

/// PING a Redis client, returns latency in ms
async fn redis_ping(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.ping().await {
        Ok(latency_ms) => Json(ApiResponse::success(serde_json::json!({
            "latency_ms": latency_ms,
            "healthy": client.healthy(),
        }))),
        Err(e) => Json(ApiResponse::error(format!("PING failed: {}", e))),
    }
}

/// GET a key from Redis
async fn redis_get(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.get(&key).await {
        Ok(val) => Json(ApiResponse::success(serde_json::json!({
            "key": key,
            "value": val,
        }))),
        Err(e) => Json(ApiResponse::error(format!("GET failed: {}", e))),
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
        Ok(()) => Json(ApiResponse::success(serde_json::json!({
            "key": body.key,
            "value": body.value,
            "ttl_seconds": body.ttl_seconds,
        }))),
        Err(e) => Json(ApiResponse::error(format!("SET failed: {}", e))),
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
        Ok(count) => Json(ApiResponse::success(serde_json::json!({
            "deleted": count,
        }))),
        Err(e) => Json(ApiResponse::error(format!("DEL failed: {}", e))),
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
        Ok(count) => Json(ApiResponse::success(serde_json::json!({
            "key": body.key,
            "field": body.field,
            "added": count,
        }))),
        Err(e) => Json(ApiResponse::error(format!("HSET failed: {}", e))),
    }
}

/// HGET from Redis
async fn redis_hget(Path((name, key, field)): Path<(String, String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.hget(&key, &field).await {
        Ok(val) => Json(ApiResponse::success(serde_json::json!({
            "key": key,
            "field": field,
            "value": val,
        }))),
        Err(e) => Json(ApiResponse::error(format!("HGET failed: {}", e))),
    }
}

/// HGETALL from Redis
async fn redis_hgetall(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.hgetall(&key).await {
        Ok(map) => Json(ApiResponse::success(serde_json::json!({
            "key": key,
            "fields": map,
        }))),
        Err(e) => Json(ApiResponse::error(format!("HGETALL failed: {}", e))),
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
        Ok(len) => Json(ApiResponse::success(serde_json::json!({
            "key": body.key,
            "length": len,
        }))),
        Err(e) => Json(ApiResponse::error(format!("RPUSH failed: {}", e))),
    }
}

/// LPOP from a Redis list
async fn redis_lpop(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.lpop(&key, Some(1)).await {
        Ok(vals) => Json(ApiResponse::success(serde_json::json!({
            "key": key,
            "values": vals,
        }))),
        Err(e) => Json(ApiResponse::error(format!("LPOP failed: {}", e))),
    }
}

/// LLEN of a Redis list
async fn redis_llen(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.llen(&key).await {
        Ok(len) => Json(ApiResponse::success(serde_json::json!({
            "key": key,
            "length": len,
        }))),
        Err(e) => Json(ApiResponse::error(format!("LLEN failed: {}", e))),
    }
}

/// INCR a Redis key
async fn redis_incr(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_redis_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.incr(&key).await {
        Ok(val) => Json(ApiResponse::success(serde_json::json!({
            "key": key,
            "value": val,
        }))),
        Err(e) => Json(ApiResponse::error(format!("INCR failed: {}", e))),
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
    let opts = crate::core::gateway::link_sys::providers::redis::LockOptions {
        ttl: std::time::Duration::from_secs(body.ttl_seconds.unwrap_or(5)),
        max_wait: std::time::Duration::from_secs(body.max_wait_seconds.unwrap_or(3)),
        retry_interval: std::time::Duration::from_millis(50),
    };
    let start = std::time::Instant::now();
    match client.lock(&body.key, opts).await {
        Ok(guard) => {
            let acquire_ms = start.elapsed().as_millis() as u64;
            // Explicitly release
            let released = guard.unlock().await.unwrap_or(false);
            Json(ApiResponse::success(serde_json::json!({
                "key": body.key,
                "acquired": true,
                "acquire_ms": acquire_ms,
                "released": released,
            })))
        }
        Err(e) => Json(ApiResponse::error(format!("LOCK failed: {}", e))),
    }
}

// ==================== LinkSys Etcd Testing Endpoints ====================

/// Helper to get an Etcd client by name, returning error response if not found.
fn get_etcd_client_by_name(
    name: &str,
) -> Result<
    std::sync::Arc<crate::core::gateway::link_sys::providers::etcd::EtcdLinkClient>,
    Json<ApiResponse<serde_json::Value>>,
> {
    let key = name.replacen('_', "/", 1);
    crate::core::gateway::link_sys::get_etcd_client(&key).ok_or_else(|| {
        Json(ApiResponse::error(format!(
            "Etcd client '{}' not found. Available: {:?}",
            key,
            crate::core::gateway::link_sys::runtime::store::list_etcd_clients()
        )))
    })
}

/// List all registered Etcd clients
async fn etcd_list_clients() -> Json<ApiResponse<Vec<String>>> {
    let clients = crate::core::gateway::link_sys::runtime::store::list_etcd_clients();
    Json(ApiResponse::success(clients))
}

/// Health check all Etcd clients
async fn etcd_health_all() -> Json<ApiResponse<Vec<crate::core::gateway::link_sys::providers::redis::LinkSysHealth>>> {
    let health = crate::core::gateway::link_sys::runtime::store::health_check_all_etcd().await;
    Json(ApiResponse::success(health))
}

/// Health check a single Etcd client
async fn etcd_health_one(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let health = client.health_status().await;
    match serde_json::to_value(health) {
        Ok(v) => Json(ApiResponse::success(v)),
        Err(e) => Json(ApiResponse::error(format!("Serialize error: {}", e))),
    }
}

/// PING an Etcd client (status check), returns latency in ms
async fn etcd_ping(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.ping().await {
        Ok(latency_ms) => Json(ApiResponse::success(serde_json::json!({
            "latency_ms": latency_ms,
            "healthy": client.healthy(),
        }))),
        Err(e) => Json(ApiResponse::error(format!("PING (status) failed: {}", e))),
    }
}

/// GET a key from Etcd
async fn etcd_get(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.get_string(&key).await {
        Ok(val) => Json(ApiResponse::success(serde_json::json!({
            "key": key,
            "value": val,
        }))),
        Err(e) => Json(ApiResponse::error(format!("GET failed: {}", e))),
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
        Ok(()) => Json(ApiResponse::success(serde_json::json!({
            "key": body.key,
            "value": body.value,
        }))),
        Err(e) => Json(ApiResponse::error(format!("PUT failed: {}", e))),
    }
}

/// DELETE a key from Etcd
async fn etcd_delete(Path((name, key)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.delete(&key).await {
        Ok(deleted) => Json(ApiResponse::success(serde_json::json!({
            "key": key,
            "deleted": deleted,
        }))),
        Err(e) => Json(ApiResponse::error(format!("DELETE failed: {}", e))),
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
            Json(ApiResponse::success(serde_json::json!({
                "prefix": prefix,
                "count": items.len(),
                "entries": items,
            })))
        }
        Err(e) => Json(ApiResponse::error(format!("GET prefix failed: {}", e))),
    }
}

/// DELETE with prefix from Etcd
async fn etcd_delete_prefix(Path((name, prefix)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.delete_prefix(&prefix).await {
        Ok(deleted) => Json(ApiResponse::success(serde_json::json!({
            "prefix": prefix,
            "deleted": deleted,
        }))),
        Err(e) => Json(ApiResponse::error(format!("DELETE prefix failed: {}", e))),
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
        Ok(lease_id) => Json(ApiResponse::success(serde_json::json!({
            "lease_id": lease_id,
            "ttl_seconds": body.ttl_seconds,
        }))),
        Err(e) => Json(ApiResponse::error(format!("LEASE GRANT failed: {}", e))),
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
        Err(_) => return Json(ApiResponse::error(format!("Invalid lease_id: {}", lease_id))),
    };
    match client.lease_revoke(lease_id).await {
        Ok(()) => Json(ApiResponse::success(serde_json::json!({
            "lease_id": lease_id,
            "revoked": true,
        }))),
        Err(e) => Json(ApiResponse::error(format!("LEASE REVOKE failed: {}", e))),
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
        Err(_) => return Json(ApiResponse::error(format!("Invalid lease_id: {}", lease_id))),
    };
    match client.lease_time_to_live(lease_id).await {
        Ok(ttl) => Json(ApiResponse::success(serde_json::json!({
            "lease_id": lease_id,
            "ttl": ttl,
        }))),
        Err(e) => Json(ApiResponse::error(format!("LEASE TTL failed: {}", e))),
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
            let acquire_ms = start.elapsed().as_millis() as u64;
            let released = guard.unlock().await.is_ok();
            Json(ApiResponse::success(serde_json::json!({
                "name": body.name,
                "acquired": true,
                "acquire_ms": acquire_ms,
                "released": released,
            })))
        }
        Ok(None) => Json(ApiResponse::success(serde_json::json!({
            "name": body.name,
            "acquired": false,
            "reason": "timeout",
        }))),
        Err(e) => Json(ApiResponse::error(format!("LOCK failed: {}", e))),
    }
}

/// Get Etcd client info (name, namespace, health)
async fn etcd_info(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_etcd_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    Json(ApiResponse::success(serde_json::json!({
        "name": client.name(),
        "namespace": client.namespace(),
        "healthy": client.healthy(),
    })))
}

// ==================== LinkSys Elasticsearch Testing Endpoints ====================

/// Helper to get an ES client by name, returning error response if not found.
fn get_es_client_by_name(
    name: &str,
) -> Result<
    std::sync::Arc<crate::core::gateway::link_sys::providers::elasticsearch::EsLinkClient>,
    Json<ApiResponse<serde_json::Value>>,
> {
    let key = name.replacen('_', "/", 1);
    crate::core::gateway::link_sys::get_es_client(&key).ok_or_else(|| {
        Json(ApiResponse::error(format!(
            "ES client '{}' not found. Available: {:?}",
            key,
            crate::core::gateway::link_sys::runtime::store::list_es_clients()
        )))
    })
}

/// List all ES clients
async fn es_list_clients() -> Json<ApiResponse<serde_json::Value>> {
    let clients = crate::core::gateway::link_sys::runtime::store::list_es_clients();
    Json(ApiResponse::success(serde_json::json!(clients)))
}

/// Health check all ES clients
async fn es_health_all() -> Json<ApiResponse<serde_json::Value>> {
    let results = crate::core::gateway::link_sys::runtime::store::health_check_all_es().await;
    Json(ApiResponse::success(serde_json::json!(results)))
}

/// Health check a single ES client
async fn es_health_one(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let health = client.health_status().await;
    Json(ApiResponse::success(serde_json::json!({
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
        Ok(latency_ms) => Json(ApiResponse::success(serde_json::json!({
            "latency_ms": latency_ms,
        }))),
        Err(e) => Json(ApiResponse::error(format!("PING failed: {}", e))),
    }
}

/// Get ES client info (name, endpoints, healthy)
async fn es_info(Path(name): Path<String>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    Json(ApiResponse::success(serde_json::json!({
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
        Ok(doc_id) => Json(ApiResponse::success(serde_json::json!({
            "index": index,
            "doc_id": doc_id,
        }))),
        Err(e) => Json(ApiResponse::error(format!("INDEX DOC failed: {}", e))),
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
        Ok(Some(source)) => Json(ApiResponse::success(serde_json::json!({
            "index": index,
            "doc_id": doc_id,
            "source": source,
        }))),
        Ok(None) => Json(ApiResponse::success(serde_json::json!({
            "index": index,
            "doc_id": doc_id,
            "source": null,
        }))),
        Err(e) => Json(ApiResponse::error(format!("GET DOC failed: {}", e))),
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
        Ok(deleted) => Json(ApiResponse::success(serde_json::json!({
            "index": index,
            "doc_id": doc_id,
            "deleted": deleted,
        }))),
        Err(e) => Json(ApiResponse::error(format!("DELETE DOC failed: {}", e))),
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
        Ok(result) => Json(ApiResponse::success(serde_json::json!({
            "took": result.took,
            "timed_out": result.timed_out,
            "total": result.hits.total.value,
            "hits": result.hits.hits.iter().map(|h| serde_json::json!({
                "id": h.id,
                "score": h.score,
                "source": h.source,
            })).collect::<Vec<_>>(),
        }))),
        Err(e) => Json(ApiResponse::error(format!("SEARCH failed: {}", e))),
    }
}

/// Create an index (no settings body needed for basic creation)
async fn es_create_index(Path((client_name, index)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.create_index(&index, None).await {
        Ok(()) => Json(ApiResponse::success(serde_json::json!({
            "index": index,
            "created": true,
        }))),
        Err(e) => Json(ApiResponse::error(format!("CREATE INDEX failed: {}", e))),
    }
}

/// Delete an index
async fn es_delete_index(Path((client_name, index)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.delete_index(&index).await {
        Ok(()) => Json(ApiResponse::success(serde_json::json!({
            "index": index,
            "deleted": true,
        }))),
        Err(e) => Json(ApiResponse::error(format!("DELETE INDEX failed: {}", e))),
    }
}

/// Check if an index exists
async fn es_index_exists(Path((client_name, index)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.index_exists(&index).await {
        Ok(exists) => Json(ApiResponse::success(serde_json::json!({
            "index": index,
            "exists": exists,
        }))),
        Err(e) => Json(ApiResponse::error(format!("INDEX EXISTS failed: {}", e))),
    }
}

/// Refresh an index (make recent writes visible to search)
async fn es_refresh_index(Path((client_name, index)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.refresh_index(&index).await {
        Ok(()) => Json(ApiResponse::success(serde_json::json!({
            "index": index,
            "refreshed": true,
        }))),
        Err(e) => Json(ApiResponse::error(format!("REFRESH failed: {}", e))),
    }
}

/// Count documents in an index
async fn es_count(Path((client_name, index)): Path<(String, String)>) -> Json<ApiResponse<serde_json::Value>> {
    let client = match get_es_client_by_name(&client_name) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.count(&index, None).await {
        Ok(count) => Json(ApiResponse::success(serde_json::json!({
            "index": index,
            "count": count,
        }))),
        Err(e) => Json(ApiResponse::error(format!("COUNT failed: {}", e))),
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
    Json(ApiResponse::success(serde_json::json!({
        "sent": count - errors,
        "errors": errors,
    })))
}

/// Serve the admin API on the specified port
pub async fn serve(config_client: Arc<ConfigClient>, port: u16) -> anyhow::Result<()> {
    let app = create_admin_router(config_client);
    let addr_str = format!("0.0.0.0:{}", port);
    let addr: std::net::SocketAddr = addr_str.parse()?;

    tracing::info!(
        component = "admin_api_gateway",
        event = "server_starting",
        addr = %addr,
        integration_testing = crate::core::common::config::is_integration_testing_mode(),
        "Gateway Admin API server listening"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
