// Edgion Unified Test Server
// Supports all protocols: HTTP/HTTPS, gRPC, WebSocket, TCP, UDP

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{ConnectInfo, Extension, Path, Request as AxumRequest},
    response::IntoResponse,
    routing::get,
    Router,
};
use clap::Parser;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(name = "test-server")]
#[command(about = "Edgion Unified Test Server - Supports all protocols")]
struct Cli {
    /// HTTP server ports (comma-separated)
    #[arg(long, default_value = "30001")]
    http_ports: String,

    /// gRPC server ports (comma-separated)
    #[arg(long, default_value = "30021")]
    grpc_ports: String,

    /// WebSocket server port
    #[arg(long, default_value = "30005")]
    websocket_port: u16,

    /// TCP server port
    #[arg(long, default_value = "30010")]
    tcp_port: u16,

    /// UDP server port
    #[arg(long, default_value = "30011")]
    udp_port: u16,

    /// Fake auth server port (for ForwardAuth plugin testing)
    #[arg(long)]
    auth_port: Option<u16>,

    /// HTTPS backend server port (for Backend TLS testing)
    #[arg(long)]
    https_backend_port: Option<u16>,

    /// HTTPS backend mTLS server port (for Backend upstream mTLS testing)
    #[arg(long)]
    https_backend_mtls_port: Option<u16>,

    /// TLS certificate file path
    #[arg(long)]
    cert_file: Option<String>,

    /// TLS private key file path
    #[arg(long)]
    key_file: Option<String>,

    /// Client CA file path for mTLS backend server
    #[arg(long)]
    client_ca_file: Option<String>,

    /// Log level
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize rustls crypto provider (required for TLS)
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = Cli::parse();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(match cli.log_level.as_str() {
            "trace" => tracing::Level::TRACE,
            "debug" => tracing::Level::DEBUG,
            "info" => tracing::Level::INFO,
            "warn" => tracing::Level::WARN,
            "error" => tracing::Level::ERROR,
            _ => tracing::Level::INFO,
        })
        .init();

    info!("========================================");
    info!("Edgion Unified Test Server");
    info!("========================================");
    info!("");

    let mut handles = Vec::new();

    // Start HTTP server
    let http_ports: Vec<u16> = cli
        .http_ports
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    for port in http_ports {
        let handle = tokio::spawn(start_http_server(port));
        handles.push(handle);
    }

    // Start gRPC server
    let grpc_ports: Vec<u16> = cli
        .grpc_ports
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    for port in grpc_ports {
        let handle = tokio::spawn(start_grpc_server(port));
        handles.push(handle);
    }

    // Start WebSocket server
    let handle = tokio::spawn(start_websocket_server(cli.websocket_port));
    handles.push(handle);

    // Start TCP server
    let handle = tokio::spawn(start_tcp_server(cli.tcp_port));
    handles.push(handle);

    // Start UDP server
    let handle = tokio::spawn(start_udp_server(cli.udp_port));
    handles.push(handle);

    // Start Fake Auth server (if configured)
    if let Some(auth_port) = cli.auth_port {
        let handle = tokio::spawn(start_auth_server(auth_port));
        handles.push(handle);
    }

    // Start HTTPS backend server (if configured)
    if let Some(https_port) = cli.https_backend_port {
        if let (Some(cert), Some(key)) = (cli.cert_file.as_ref(), cli.key_file.as_ref()) {
            let handle = tokio::spawn(start_https_backend_server(https_port, cert.clone(), key.clone()));
            handles.push(handle);
        } else {
            error!("HTTPS backend port specified but cert_file or key_file missing");
        }
    }

    // Start HTTPS backend mTLS server (if configured)
    if let Some(https_mtls_port) = cli.https_backend_mtls_port {
        if let (Some(cert), Some(key), Some(client_ca)) = (
            cli.cert_file.as_ref(),
            cli.key_file.as_ref(),
            cli.client_ca_file.as_ref(),
        ) {
            let handle = tokio::spawn(start_https_backend_mtls_server(
                https_mtls_port,
                cert.clone(),
                key.clone(),
                client_ca.clone(),
            ));
            handles.push(handle);
        } else {
            error!("HTTPS backend mTLS port specified but cert_file, key_file, or client_ca_file missing");
        }
    }

    info!("");
    info!("========================================");
    info!("All servers started, press Ctrl+C to stop");
    info!("========================================");

    // Wait for all servers
    futures::future::join_all(handles).await;

    Ok(())
}

// ============================================================================
// HTTP Server
// ============================================================================

async fn start_http_server(port: u16) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let server_addr_str = format!("0.0.0.0:{}", port);
    let app = create_echo_router(server_addr_str.clone());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("✓ HTTP server listening on http://{}", addr);

    let app_with_connect_info = app.into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app_with_connect_info).await?;

    Ok(())
}

fn create_echo_router(server_addr_str: String) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/echo", get(echo_handler).post(echo_post_handler))
        .route("/request-header-test", get(header_echo_handler))
        .route("/both-headers-test", get(header_echo_handler))
        .route("/multi-request-headers", get(header_echo_handler))
        .route("/response-header-test", get(header_echo_handler))
        .route("/headers", get(headers_handler))
        .route("/auth-header-probe", get(auth_header_probe_handler))
        .route(
            "/webhook/resolve",
            get(webhook_resolve_handler).post(webhook_resolve_handler),
        )
        .route(
            "/webhook/resolve-body",
            get(webhook_resolve_body_handler).post(webhook_resolve_body_handler),
        )
        .route("/webhook/healthz", get(health_handler))
        .route("/status/{code}", get(status_handler))
        .route("/delay/{seconds}", get(delay_handler))
        .route("/{*path}", get(catch_all_handler))
        .layer(Extension(server_addr_str))
}

fn display_header_name(name: &str) -> String {
    name
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

async fn header_echo_handler(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(server_addr): Extension<String>,
    req: AxumRequest<Body>,
) -> impl IntoResponse {
    let mut resp = String::with_capacity(1024);
    resp.push_str(&format!("Server: {}\n", server_addr));
    resp.push_str(&format!("Client: {}\n", addr));
    resp.push_str(&format!("Method: {}\n", req.method()));
    resp.push_str(&format!("Path: {}\n", req.uri().path()));
    resp.push_str("\nHeaders:\n");

    for (key, value) in req.headers() {
        let display = display_header_name(key.as_str());
        resp.push_str(&format!("  {}: {}\n", display, value.to_str().unwrap_or("<invalid>")));
    }

    resp
}

async fn health_handler() -> impl IntoResponse {
    "OK"
}

async fn echo_handler(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(server_addr): Extension<String>,
    req: AxumRequest<Body>,
) -> impl IntoResponse {
    let mut resp = String::with_capacity(1024);
    resp.push_str(&format!("Server: {}\n", server_addr));
    resp.push_str(&format!("Client: {}\n", addr));
    resp.push_str(&format!("Method: {}\n", req.method()));
    resp.push_str(&format!("Path: {}\n", req.uri().path()));

    resp.push_str("\nHeaders:\n");
    for (key, value) in req.headers() {
        resp.push_str(&format!("  {}: {}\n", key, value.to_str().unwrap_or("<invalid>")));
    }

    resp
}

async fn echo_post_handler(Extension(server_addr): Extension<String>, body: String) -> impl IntoResponse {
    format!("Server: {}\nEcho: {}", server_addr, body)
}

async fn headers_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;

    let headers = req.headers();
    let mut headers_map = serde_json::Map::new();

    // Return all headers (convert header names to lowercase for consistency)
    for (key, value) in headers.iter() {
        if let Ok(val) = value.to_str() {
            headers_map.insert(key.as_str().to_lowercase(), json!(val));
        }
    }

    let response = json!({
        "headers": headers_map
    });

    (StatusCode::OK, Json(response))
}

/// Auth-header probe handler.
///
/// This endpoint is designed specifically to test the `hide_credentials` feature of auth plugins.
/// It checks whether the incoming request contains an `Authorization` header and
/// reflects that information back to the client via response headers.
///
/// Response headers returned:
///   - `X-Auth-Header-Present`: `"yes"` if Authorization header was received, `"no"` otherwise.
///   - `X-Auth-Header-Value-Prefix`: The first 6 characters of the Authorization header value
///     (e.g., "Bearer" or "Basic "), or `"none"` if the header is absent.
///     This is useful to confirm the type of credential without leaking the full secret.
///
/// Usage in tests:
///   1. Route a request with an Authorization header through an auth plugin with
///      `hideCredentials: true`.
///   2. Check `X-Auth-Header-Present == "no"` in the response to verify Gateway did remove it.
///   3. Without `hideCredentials`, `X-Auth-Header-Present` should be `"yes"`.
async fn auth_header_probe_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::body::Body as RespBody;
    use axum::http::StatusCode;
    use axum::response::Response;

    let auth_present = req.headers().contains_key("authorization");
    let value_prefix = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            // Return first 6 chars to indicate type ("Bearer", "Basic ") without leaking the full secret
            let prefix_len = s.len().min(6);
            s[..prefix_len].to_string()
        })
        .unwrap_or_else(|| "none".to_string());

    Response::builder()
        .status(StatusCode::OK)
        .header("X-Auth-Header-Present", if auth_present { "yes" } else { "no" })
        .header("X-Auth-Header-Value-Prefix", &value_prefix)
        .body(RespBody::from(if auth_present {
            "Authorization header was received by upstream"
        } else {
            "Authorization header was NOT received by upstream (removed by gateway)"
        }))
        .unwrap()
}

/// Webhook resolve handler — returns resolved key value via response header and JSON body.
///
/// Simulates an external key-resolution service. Reads the `X-Tenant-Id` header
/// from the forwarded request and returns:
/// - Response header `X-Resolved-Key` with the resolved value
/// - Response header `X-Webhook-Status` = "resolved"
/// - Set-Cookie `webhook_session=wh-sess-<tenant>`
/// - JSON body: `{"data":{"user_id":"uid-<tenant>","tenant":"<tenant>"}}`
///
/// If `X-Tenant-Id` is missing, returns a default resolution.
async fn webhook_resolve_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;

    let headers = req.headers();
    let tenant = headers
        .get("x-tenant-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    let resolved_key = format!("resolved-{}", tenant);
    let user_id = format!("uid-{}", tenant);
    let cookie_val = format!("webhook_session=wh-sess-{}", tenant);

    let body = json!({
        "data": {
            "user_id": user_id,
            "tenant": tenant,
        }
    });

    (
        StatusCode::OK,
        [
            ("X-Resolved-Key", resolved_key.as_str()),
            ("X-Webhook-Status", "resolved"),
            ("Set-Cookie", cookie_val.as_str()),
        ],
        Json(body),
    )
        .into_response()
}

/// Webhook resolve handler for body-text extraction testing.
/// Returns plain text body with the resolved key value.
async fn webhook_resolve_body_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    let headers = req.headers();
    let tenant = headers
        .get("x-tenant-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    format!("  body-key-{}  ", tenant)
}

async fn status_handler(Path(code): Path<u16>) -> impl IntoResponse {
    (
        axum::http::StatusCode::from_u16(code).unwrap_or(axum::http::StatusCode::OK),
        format!("Status: {}", code),
    )
}

async fn delay_handler(Path(seconds): Path<u64>) -> impl IntoResponse {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    format!("Delayed {} seconds", seconds)
}

async fn catch_all_handler(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(server_addr): Extension<String>,
    req: AxumRequest<Body>,
) -> impl IntoResponse {
    format!(
        "Server: {}\nClient: {}\nMethod: {}\nPath: {}\n",
        server_addr,
        addr,
        req.method(),
        req.uri().path()
    )
}

// ============================================================================
// gRPC Server
// ============================================================================
// Use pre-generated proto code instead of build-time generation
#[path = "../proto_gen/test.rs"]
pub mod test;

use test::test_service_server::{TestService, TestServiceServer};
use test::{HelloRequest, HelloResponse, NumberRequest, NumberResponse};
use tonic::{transport::Server, Request, Response, Status};

#[derive(Debug, Clone)]
pub struct TestServiceImpl {
    server_addr: String,
}

impl TestServiceImpl {
    fn new(server_addr: String) -> Self {
        Self { server_addr }
    }
}

#[tonic::async_trait]
impl TestService for TestServiceImpl {
    async fn say_hello(&self, request: Request<HelloRequest>) -> Result<Response<HelloResponse>, Status> {
        let name = request.into_inner().name;

        let response = HelloResponse {
            message: format!("Hello, {}!", name),
            server_addr: self.server_addr.clone(),
        };

        Ok(Response::new(response))
    }

    type StreamNumbersStream = tokio_stream::wrappers::ReceiverStream<Result<NumberResponse, Status>>;

    async fn stream_numbers(
        &self,
        request: Request<NumberRequest>,
    ) -> Result<Response<Self::StreamNumbersStream>, Status> {
        let count = request.into_inner().count;
        let (tx, rx) = tokio::sync::mpsc::channel(10);

        tokio::spawn(async move {
            for i in 1..=count {
                let response = NumberResponse { number: i };
                if tx.send(Ok(response)).await.is_err() {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

async fn start_grpc_server(port: u16) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let server_addr_str = format!("0.0.0.0:{}", port);

    let service = TestServiceImpl::new(server_addr_str);

    info!("✓ gRPC server listening on http://{}", addr);

    Server::builder()
        .add_service(TestServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}

// ============================================================================
// WebSocket Server
// ============================================================================

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};

async fn start_websocket_server(port: u16) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let app = Router::new().route("/ws", get(ws_handler));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("✓ WebSocket server listening on ws://{}/ws", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    while let Some(msg) = socket.recv().await {
        if let Ok(msg) = msg {
            match msg {
                Message::Text(text) => {
                    let echo = format!("Echo: {}", text);
                    if socket.send(Message::Text(echo.into())).await.is_err() {
                        break;
                    }
                }
                Message::Binary(data) => {
                    if socket.send(Message::Binary(data)).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        } else {
            break;
        }
    }
}

// ============================================================================
// TCP Server
// ============================================================================

async fn start_tcp_server(port: u16) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;

    info!("✓ TCP server listening on tcp://{}", addr);

    loop {
        match listener.accept().await {
            Ok((mut socket, _peer_addr)) => {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];

                    loop {
                        match socket.read(&mut buf).await {
                            Ok(0) => break, // Connection closed
                            Ok(n) => {
                                // Echo back
                                if socket.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                });
            }
            Err(e) => {
                error!("TCP accept error: {}", e);
            }
        }
    }
}

// ============================================================================
// UDP Server
// ============================================================================

async fn start_udp_server(port: u16) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let socket = UdpSocket::bind(addr).await?;

    info!("✓ UDP server listening on udp://{}", addr);

    let mut buf = vec![0u8; 4096];

    loop {
        match socket.recv_from(&mut buf).await {
            Ok((n, peer_addr)) => {
                // Echo back
                if let Err(e) = socket.send_to(&buf[..n], peer_addr).await {
                    error!("UDP send error: {}", e);
                }
            }
            Err(e) => {
                error!("UDP recv error: {}", e);
            }
        }
    }
}

// ============================================================================
// Fake Auth Server (for ForwardAuth plugin testing)
// ============================================================================
//
// This server simulates an external authentication service.
// It validates requests and returns appropriate responses:
//
// Authentication logic:
//   - Authorization: Bearer valid-token   → 200 + user identity headers
//   - Authorization: Bearer admin-token   → 200 + admin identity headers
//   - Authorization: Bearer forbidden     → 403 + error body
//   - No/invalid Authorization            → 401 + WWW-Authenticate header
//
// On success (2xx), returns headers that ForwardAuth should copy to upstream:
//   - X-User-ID, X-User-Role, X-User-Email
//
// On failure (non-2xx), returns headers that ForwardAuth should copy to client:
//   - WWW-Authenticate, X-Auth-Error-Code
//
// Also validates that X-Forwarded-* headers are correctly set by ForwardAuth plugin.

async fn start_auth_server(port: u16) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let app = Router::new()
        .route("/verify", get(auth_verify_handler).post(auth_verify_handler))
        .route("/oidc/.well-known/openid-configuration", get(oidc_discovery_handler))
        .route("/oidc/jwks", get(oidc_jwks_handler))
        .route(
            "/oidc/introspect",
            get(oidc_introspect_handler).post(oidc_introspect_handler),
        )
        .route("/health", get(health_handler));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("✓ Auth server listening on http://{}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn oidc_discovery_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;

    let discovery_host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("127.0.0.1:30040");

    let issuer = "http://127.0.0.1:30040/oidc";
    let endpoint_base = format!("http://{}/oidc", discovery_host);
    let body = json!({
        "issuer": issuer,
        "jwks_uri": format!("{}/jwks", endpoint_base),
        "introspection_endpoint": format!("{}/introspect", endpoint_base),
    });

    (StatusCode::OK, Json(body))
}

async fn oidc_jwks_handler() -> impl IntoResponse {
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;

    // Public key of OIDC test RSA key pair (RS256), kid = oidc-rs256-test-key
    let body = json!({
        "keys": [{
            "kty": "RSA",
            "kid": "oidc-rs256-test-key",
            "alg": "RS256",
            "use": "sig",
            "n": "0UmLCqLGqy-oTAMXpajmd411_JmJ7s5ObbwVbWN7uviddI96Yg5NtObwmXcTuDeI2cfyjRgDDLFAE7gO7BYbX3qCGw1fDPxU--Gp7FwdqrOFOcgRjqwkzC9Ynw_C9X_qe0pkkNrP5qGFyTazcUTfTbhtNACqCmIPI_kH2vvwpbOlJ1a04-OUoUvG_kKvyrFAP2RX5ow38DDxDgzX0xaxhr1gIupGrrg3_y4oCe8xvQ5kM3MRl_Xybywr_jjDigEy5jUIkGjabVo1VEBV5Q0UxZbaPZAT2_lgR5_zz9yrqnscIFosZ03EL0piOjTP2qroJrmv2J1gENfu5bz_8HyS9Q",
            "e": "AQAB"
        }]
    });

    (StatusCode::OK, Json(body))
}

fn extract_form_token(body: &str) -> Option<String> {
    for pair in body.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == "token" {
            return Some(v.to_string());
        }
    }
    None
}

async fn oidc_introspect_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;

    let body_bytes = match to_bytes(req.into_body(), 8 * 1024).await {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"active": false}))),
    };
    let body = String::from_utf8_lossy(&body_bytes);
    let token = extract_form_token(&body);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let issuer = "http://127.0.0.1:30040/oidc";
    let response = match token.as_deref() {
        Some("oidc-active-token") => json!({
            "active": true,
            "iss": issuer,
            "sub": "oidc-user-123",
            "scope": "api:read profile",
            "exp": now + 3600
        }),
        Some("oidc-auto-fallback-token") | Some("oidc.auto.fallback") => json!({
            "active": true,
            "iss": issuer,
            "sub": "oidc-user-auto",
            "scope": "api:read",
            "exp": now + 3600
        }),
        Some("oidc-insufficient-scope-token") => json!({
            "active": true,
            "iss": issuer,
            "sub": "oidc-user-456",
            "scope": "profile",
            "exp": now + 3600
        }),
        Some("oidc-expired-token") => json!({
            "active": true,
            "iss": issuer,
            "sub": "oidc-user-expired",
            "scope": "api:read",
            "exp": now.saturating_sub(3600)
        }),
        _ => json!({ "active": false }),
    };

    (StatusCode::OK, Json(response))
}

/// Auth verification handler.
///
/// Validates the Authorization header and returns:
/// - 200 + identity headers on success
/// - 401 + WWW-Authenticate on missing/invalid auth
/// - 403 + error body on forbidden token
///
/// Also echoes back X-Forwarded-* headers in response body for validation.
async fn auth_verify_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;

    let headers = req.headers();

    // Collect X-Forwarded-* headers for response body (so client can verify them)
    let forwarded_host = headers
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let forwarded_uri = headers
        .get("x-forwarded-uri")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let forwarded_method = headers
        .get("x-forwarded-method")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Collect all request headers for debugging
    let mut received_headers = serde_json::Map::new();
    for (key, value) in headers.iter() {
        if let Ok(val) = value.to_str() {
            received_headers.insert(key.as_str().to_lowercase(), json!(val));
        }
    }

    // Check Authorization header
    let auth_header = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");

    match auth_header {
        // Valid regular user token
        "Bearer valid-token" => {
            let body = json!({
                "status": "ok",
                "user": "test-user",
                "forwarded_host": forwarded_host,
                "forwarded_uri": forwarded_uri,
                "forwarded_method": forwarded_method,
                "received_headers": received_headers,
            });
            (
                StatusCode::OK,
                [
                    ("X-User-ID", "user-123"),
                    ("X-User-Role", "member"),
                    ("X-User-Email", "test@example.com"),
                ],
                Json(body),
            )
                .into_response()
        }
        // Valid admin token
        "Bearer admin-token" => {
            let body = json!({
                "status": "ok",
                "user": "admin-user",
                "forwarded_host": forwarded_host,
                "forwarded_uri": forwarded_uri,
                "forwarded_method": forwarded_method,
                "received_headers": received_headers,
            });
            (
                StatusCode::OK,
                [
                    ("X-User-ID", "admin-001"),
                    ("X-User-Role", "admin"),
                    ("X-User-Email", "admin@example.com"),
                ],
                Json(body),
            )
                .into_response()
        }
        // Forbidden token
        "Bearer forbidden" => {
            let body = json!({
                "error": "forbidden",
                "message": "Access denied by auth service"
            });
            (
                StatusCode::FORBIDDEN,
                [("X-Auth-Error-Code", "FORBIDDEN_ROLE")],
                Json(body),
            )
                .into_response()
        }
        // No auth or invalid token → 401
        _ => {
            let body = json!({
                "error": "unauthorized",
                "message": "Invalid or missing authentication token"
            });
            (
                StatusCode::UNAUTHORIZED,
                [
                    ("WWW-Authenticate", "Bearer realm=\"test\""),
                    ("X-Auth-Error-Code", "INVALID_TOKEN"),
                ],
                Json(body),
            )
                .into_response()
        }
    }
}

// ============================================================================
// HTTPS Backend Server (for BackendTLSPolicy testing)
// ============================================================================

use axum_server::tls_rustls::RustlsConfig;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::RootCertStore;
use std::sync::Arc;

async fn start_https_backend_server(port: u16, cert_path: String, key_path: String) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let server_addr_str = format!("0.0.0.0:{}", port);

    // Load TLS configuration
    let tls_config = match RustlsConfig::from_pem_file(&cert_path, &key_path).await {
        Ok(config) => config,
        Err(e) => {
            error!("Failed to load TLS certificates: {}", e);
            error!("  Cert file: {}", cert_path);
            error!("  Key file: {}", key_path);
            return Err(anyhow::anyhow!("TLS configuration error: {}", e));
        }
    };

    let app = create_echo_router(server_addr_str.clone());

    info!("✓ HTTPS backend server listening on https://{}", addr);
    info!("  Certificate: {}", cert_path);
    info!("  Private key: {}", key_path);

    // Start HTTPS server
    let app_with_connect_info = app.into_make_service_with_connect_info::<SocketAddr>();
    axum_server::bind_rustls(addr, tls_config)
        .serve(app_with_connect_info)
        .await?;

    Ok(())
}

async fn start_https_backend_mtls_server(
    port: u16,
    cert_path: String,
    key_path: String,
    client_ca_path: String,
) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let server_addr_str = format!("0.0.0.0:{}", port);

    let tls_config = load_mtls_rustls_config(&cert_path, &key_path, &client_ca_path)?;
    let tls_config = RustlsConfig::from_config(Arc::new(tls_config));
    let app = create_echo_router(server_addr_str.clone());

    info!("✓ HTTPS backend mTLS server listening on https://{}", addr);
    info!("  Certificate: {}", cert_path);
    info!("  Private key: {}", key_path);
    info!("  Client CA: {}", client_ca_path);

    let app_with_connect_info = app.into_make_service_with_connect_info::<SocketAddr>();
    axum_server::bind_rustls(addr, tls_config)
        .serve(app_with_connect_info)
        .await?;

    Ok(())
}

fn load_mtls_rustls_config(cert_path: &str, key_path: &str, client_ca_path: &str) -> Result<rustls::ServerConfig> {
    let cert_chain = CertificateDer::pem_file_iter(cert_path)
        .with_context(|| format!("failed to read certificate file {}", cert_path))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("failed to parse certificate chain {}", cert_path))?;

    let private_key =
        PrivateKeyDer::from_pem_file(key_path).with_context(|| format!("failed to parse private key {}", key_path))?;

    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(client_ca_path)
        .with_context(|| format!("failed to read client CA file {}", client_ca_path))?
    {
        let cert = cert.with_context(|| format!("failed to parse client CA file {}", client_ca_path))?;
        roots
            .add(cert)
            .with_context(|| format!("failed to load client CA certificate from {}", client_ca_path))?;
    }

    let client_verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .context("failed to build client certificate verifier")?;

    let mut config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(cert_chain, private_key)
        .context("failed to build rustls server config for mTLS backend")?;

    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}
