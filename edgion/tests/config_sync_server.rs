use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify};
use tokio::time::{interval, sleep};

use edgion::core::conf_sync::config_server::{ConfigServer, GatewayClassKey};
use edgion::core::conf_sync::grpc_server::ConfigSyncServer;
use edgion::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use edgion::types::ResourceKind;

const DEFAULT_GATEWAY_CLASS_KEY: &str = "test-gateway-class";
const DEFAULT_GRPC_ADDR: &str = "0.0.0.0:50051";
const DEFAULT_HTTP_ADDR: &str = "0.0.0.0:8080";

#[derive(Clone, Copy)]
pub enum RunMode {
    UntilCtrlC,
    For(Duration),
}

pub async fn run_config_sync_server(mode: RunMode) -> anyhow::Result<()> {
    println!("[SERVER] Starting Config Sync server example");

    let config_center = Arc::new(Mutex::new(ConfigServer::new()));
    let version_counters = Arc::new(Mutex::new(HashMap::<ResourceKind, u64>::new()));
    let known_gateway_keys = Arc::new(Mutex::new(HashSet::new()));

    {
        let mut keys = known_gateway_keys.lock().await;
        keys.insert(DEFAULT_GATEWAY_CLASS_KEY.to_string());
    }

    let state = ServerState {
        config_center: config_center.clone(),
        version_counters,
        known_gateway_keys: known_gateway_keys.clone(),
    };

    let grpc_addr: SocketAddr = DEFAULT_GRPC_ADDR.parse()?;
    let http_addr: SocketAddr = DEFAULT_HTTP_ADDR.parse()?;

    let shutdown = Arc::new(Notify::new());

    let grpc_shutdown = shutdown.clone();
    let grpc_center = config_center.clone();
    let grpc_handle = tokio::spawn(async move {
        let server = ConfigSyncServer::new_with_shared(grpc_center.clone());
        println!("[SERVER] gRPC endpoint listening on {}", grpc_addr);
        tokio::select! {
            res = server.serve(grpc_addr) => {
                if let Err(err) = res {
                    eprintln!("[SERVER] gRPC server error: {}", err);
                }
            }
            _ = grpc_shutdown.notified() => {
                println!("[SERVER] gRPC server shutting down");
            }
        }
    });

    let http_shutdown = shutdown.clone();
    let http_state = state.clone();
    let http_handle = tokio::spawn(async move {
        let app = Router::new()
            .route("/configs", post(add_config))
            .with_state(http_state);

        let listener = match TcpListener::bind(http_addr).await {
            Ok(listener) => listener,
            Err(err) => {
                eprintln!("[SERVER] Failed to bind HTTP listener: {}", err);
                return;
            }
        };
        println!("[SERVER] HTTP endpoint listening on http://{}", http_addr);

        if let Err(err) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                http_shutdown.notified().await;
            })
            .await
        {
            eprintln!("[SERVER] HTTP server error: {}", err);
        }
    });

    let status_shutdown = shutdown.clone();
    let status_center = config_center.clone();
    let status_keys = known_gateway_keys.clone();
    let status_handle = tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let keys: Vec<String> = {
                        let guard = status_keys.lock().await;
                        guard.iter().cloned().collect()
                    };

                    for key in keys {
                        let center = status_center.lock().await;
                        log_center_summary(&center, &key).await;
                    }
                }
                _ = status_shutdown.notified() => {
                    println!("[SERVER] Status logger shutting down");
                    break;
                }
            }
        }
    });

    match mode {
        RunMode::UntilCtrlC => {
            println!("[SERVER] Running... press Ctrl+C to exit");
            tokio::signal::ctrl_c().await?;
        }
        RunMode::For(duration) => {
            println!("[SERVER] Running for {:?}", duration);
            sleep(duration).await;
        }
    }

    println!("[SERVER] Shutdown signal received. Stopping services...");

    shutdown.notify_waiters();
    grpc_handle.abort();
    http_handle.abort();
    status_handle.abort();

    let _ = grpc_handle.await;
    let _ = http_handle.await;
    let _ = status_handle.await;

    Ok(())
}

#[derive(Clone)]
struct ServerState {
    config_center: Arc<Mutex<ConfigServer>>,
    version_counters: Arc<Mutex<HashMap<ResourceKind, u64>>>,
    known_gateway_keys: Arc<Mutex<HashSet<GatewayClassKey>>>,
}

#[derive(Deserialize)]
struct AddConfigRequest {
    kind: String,
    #[serde(default)]
    operation: Operation,
    #[serde(default)]
    gateway_class_key: Option<String>,
    #[serde(default)]
    resource_version: Option<u64>,
    data: serde_json::Value,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Operation {
    Add,
    Update,
    Delete,
}

impl Default for Operation {
    fn default() -> Self {
        Operation::Add
    }
}

#[derive(serde::Serialize)]
struct AddConfigResponse {
    status: &'static str,
    kind: String,
    operation: String,
    gateway_class_key: String,
    resource_version: u64,
}

async fn add_config(
    State(state): State<ServerState>,
    Json(payload): Json<AddConfigRequest>,
) -> impl IntoResponse {
    let kind_name = payload.kind.clone();
    let kind = match parse_kind(&kind_name) {
        Some(kind) => kind,
        None => {
            return (StatusCode::BAD_REQUEST, "Unknown resource kind").into_response();
        }
    };

    let data_str = match serde_json::to_string(&payload.data) {
        Ok(json) => json,
        Err(err) => {
            eprintln!("[SERVER] Failed to serialize payload: {}", err);
            return (
                StatusCode::BAD_REQUEST,
                format!("Failed to serialize data: {}", err),
            )
                .into_response();
        }
    };

    let gateway_class_key = payload
        .gateway_class_key
        .clone()
        .unwrap_or_else(|| DEFAULT_GATEWAY_CLASS_KEY.to_string());

    {
        let mut keys = state.known_gateway_keys.lock().await;
        keys.insert(gateway_class_key.clone());
    }

    let version = match payload.resource_version {
        Some(version) => version,
        None => next_version(&state, kind).await,
    };

    let mut center = state.config_center.lock().await;

    let operation = payload.operation;

    match operation {
        Operation::Add => {
            <ConfigServer as EventDispatcher>::apply_resource_change(
                &mut *center,
                ResourceChange::EventAdd,
                Some(kind),
                data_str.clone(),
                Some(version),
            );
        }
        Operation::Update => {
            <ConfigServer as EventDispatcher>::apply_resource_change(
                &mut *center,
                ResourceChange::EventUpdate,
                Some(kind),
                data_str.clone(),
                Some(version),
            );
        }
        Operation::Delete => {
            <ConfigServer as EventDispatcher>::apply_resource_change(
                &mut *center,
                ResourceChange::EventDelete,
                Some(kind),
                data_str,
                Some(version),
            );
        }
    }

    let response = AddConfigResponse {
        status: "ok",
        kind: kind_name,
        operation: format_operation(operation),
        gateway_class_key,
        resource_version: version,
    };

    (StatusCode::OK, Json(response)).into_response()
}

async fn next_version(state: &ServerState, kind: ResourceKind) -> u64 {
    let mut counters = state.version_counters.lock().await;
    let counter = counters.entry(kind).or_insert(0);
    *counter += 1;
    *counter
}

fn parse_kind(kind: &str) -> Option<ResourceKind> {
    match kind {
        "GatewayClass" => Some(ResourceKind::GatewayClass),
        "GatewayClassSpec" => Some(ResourceKind::EdgionGatewayConfig),
        "Gateway" => Some(ResourceKind::Gateway),
        "HTTPRoute" | "HttpRoute" => Some(ResourceKind::HTTPRoute),
        "Service" => Some(ResourceKind::Service),
        "EndpointSlice" => Some(ResourceKind::EndpointSlice),
        "EdgionTls" => Some(ResourceKind::EdgionTls),
        "Secret" => Some(ResourceKind::Secret),
        _ => None,
    }
}

fn format_operation(operation: Operation) -> String {
    match operation {
        Operation::Add => "add".to_string(),
        Operation::Update => "update".to_string(),
        Operation::Delete => "delete".to_string(),
    }
}

async fn log_center_summary(center: &ConfigServer, key: &str) {
    let key_string = key.to_string();
    let gc_count = center
        .list_gateway_classes(&key_string)
        .await
        .map(|d| d.data.len())
        .unwrap_or(0);
    let spec_count = center
        .list_edgion_gateway_configs(&key_string)
        .await
        .map(|d| d.data.len())
        .unwrap_or(0);
    let gateway_count = center
        .list_gateways(&key_string)
        .await
        .map(|d| d.data.len())
        .unwrap_or(0);
    let route_count = center
        .list_routes(&key_string)
        .await
        .map(|d| d.data.len())
        .unwrap_or(0);
    let svc_count = center
        .list_services(&key_string)
        .await
        .map(|d| d.data.len())
        .unwrap_or(0);
    let endpoint_count = center
        .list_endpoint_slices(&key_string)
        .await
        .map(|d| d.data.len())
        .unwrap_or(0);
    let tls_count = center
        .list_edgion_tls(&key_string)
        .await
        .map(|d| d.data.len())
        .unwrap_or(0);
    let secret_count = center
        .list_secrets(&key_string)
        .await
        .map(|d| d.data.len())
        .unwrap_or(0);

    println!(
        "[SERVER] Summary key={} GatewayClass={} GatewayClassSpec={} Gateway={} HTTPRoute={} Service={} EndpointSlice={} EdgionTls={} Secret={}",
        key,
        gc_count,
        spec_count,
        gateway_count,
        route_count,
        svc_count,
        endpoint_count,
        tls_count,
        secret_count
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual integration harness; exposes Config Sync server endpoints"]
async fn config_sync_server_manual() -> anyhow::Result<()> {
    run_config_sync_server(RunMode::UntilCtrlC).await
}
