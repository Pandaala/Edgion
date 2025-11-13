use crate::core::conf_sync::config_client::ConfigClient;
use crate::core::conf_sync::config_server::ConfigServer;
use crate::types::ResourceKind;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde::Serialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::core::model::edgion_op::DEFAULT_GATEWAY_CLASS_KEY;

pub(crate) struct OperatorAdminHandle {
    handle: JoinHandle<()>,
}

impl OperatorAdminHandle {
    pub async fn shutdown(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

pub(crate) struct GatewayAdminHandle {
    handle: JoinHandle<()>,
}

impl GatewayAdminHandle {
    pub async fn shutdown(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

#[derive(Clone)]
struct OperatorAdminState {
    server: Arc<tokio::sync::Mutex<ConfigServer>>,
}

#[derive(Clone)]
struct GatewayAdminState {
    client: Arc<tokio::sync::Mutex<ConfigClient>>,
}

#[derive(Deserialize)]
struct OperatorQuery {
    kind: String,
    key: Option<String>,
}

#[derive(Deserialize)]
struct GatewayQuery {
    kind: String,
}

pub(crate) fn spawn_operator_admin_server(
    server: Arc<tokio::sync::Mutex<ConfigServer>>,
    addr: SocketAddr,
) -> OperatorAdminHandle {
    let state = OperatorAdminState { server };
    let app = Router::new()
        .route("/healthz", get(health_handler))
        .route("/configs", get(operator_config_handler))
        .with_state(state);

    let handle = tokio::spawn(async move {
        match TcpListener::bind(addr).await {
            Ok(listener) => {
                if let Err(err) = axum::serve(listener, app.into_make_service()).await {
                    eprintln!("operator admin server exited: {}", err);
                }
            }
            Err(err) => eprintln!("failed to bind operator admin server: {}", err),
        }
    });

    OperatorAdminHandle { handle }
}

pub(crate) fn spawn_gateway_admin_server(
    client: Arc<tokio::sync::Mutex<ConfigClient>>,
    addr: SocketAddr,
) -> GatewayAdminHandle {
    let state = GatewayAdminState { client };
    let app = Router::new()
        .route("/healthz", get(health_handler))
        .route("/configs", get(gateway_config_handler))
        .with_state(state);

    let handle = tokio::spawn(async move {
        match TcpListener::bind(addr).await {
            Ok(listener) => {
                if let Err(err) = axum::serve(listener, app.into_make_service()).await {
                    eprintln!("gateway admin server exited: {}", err);
                }
            }
            Err(err) => eprintln!("failed to bind gateway admin server: {}", err),
        }
    });

    GatewayAdminHandle { handle }
}

async fn health_handler() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn operator_config_handler(
    State(state): State<OperatorAdminState>,
    Query(query): Query<OperatorQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let kind = parse_resource_kind(&query.kind).ok_or((
        StatusCode::BAD_REQUEST,
        format!("unknown resource kind '{}'", query.kind),
    ))?;
    let key = query
        .key
        .clone()
        .unwrap_or_else(|| DEFAULT_GATEWAY_CLASS_KEY.to_string());

    let server = state.server.lock().await;
    let list = server.list(&key, &kind)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err))?;

    let data: Value = serde_json::from_str(&list.data)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(Json(json!({
        "key": key,
        "kind": query.kind,
        "resource_version": list.resource_version,
        "data": data,
    })))
}

async fn gateway_config_handler(
    State(state): State<GatewayAdminState>,
    Query(query): Query<GatewayQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let kind = parse_resource_kind(&query.kind).ok_or((
        StatusCode::BAD_REQUEST,
        format!("unknown resource kind '{}'", query.kind),
    ))?;

    let client = state.client.lock().await;
    let response = match kind {
        ResourceKind::GatewayClass => {
            let list = client.list_gateway_classes();
            list_refs_to_json(list.data, list.resource_version)?
        }
        ResourceKind::EdgionGatewayConfig => {
            let list = client.list_edgion_gateway_config();
            list_refs_to_json(list.data, list.resource_version)?
        }
        ResourceKind::Gateway => {
            let list = client.list_gateways();
            list_refs_to_json(list.data, list.resource_version)?
        }
        ResourceKind::HTTPRoute => {
            let list = client.list_routes();
            list_refs_to_json(list.data, list.resource_version)?
        }
        ResourceKind::Service => {
            let list = client.list_services();
            list_refs_to_json(list.data, list.resource_version)?
        }
        ResourceKind::EndpointSlice => {
            let list = client.list_endpoint_slices();
            list_refs_to_json(list.data, list.resource_version)?
        }
        ResourceKind::EdgionTls => {
            let list = client.list_edgion_tls();
            list_refs_to_json(list.data, list.resource_version)?
        }
        ResourceKind::Secret => {
            let list = client.list_secrets();
            list_refs_to_json(list.data, list.resource_version)?
        }
    };

    Ok(Json(json!({
        "kind": query.kind,
        "resource_version": response.1,
        "data": response.0,
    })))
}

fn parse_resource_kind(kind: &str) -> Option<ResourceKind> {
    match kind.to_ascii_lowercase().as_str() {
        "gatewayclass" => Some(ResourceKind::GatewayClass),
        "edgiongatewayconfig" | "gatewayclassspec" => Some(ResourceKind::EdgionGatewayConfig),
        "gateway" => Some(ResourceKind::Gateway),
        "httproute" => Some(ResourceKind::HTTPRoute),
        "service" => Some(ResourceKind::Service),
        "endpointslice" => Some(ResourceKind::EndpointSlice),
        "edgiontls" => Some(ResourceKind::EdgionTls),
        "secret" => Some(ResourceKind::Secret),
        _ => None,
    }
}

fn list_refs_to_json<T>(
    data: Vec<T>,
    resource_version: u64,
) -> Result<(Value, u64), (StatusCode, String)>
where
    T: Serialize,
{
    let json = serde_json::to_value(data)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok((json, resource_version))
}
