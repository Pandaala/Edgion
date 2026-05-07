// Edgion Unified Test Server
// Supports all protocols: HTTP/HTTPS, gRPC, WebSocket, TCP, UDP

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{ConnectInfo, Extension, Path, Request as AxumRequest},
    http::Version,
    response::IntoResponse,
    routing::get,
    Router,
};
use clap::Parser;
use h2::server as h2s;
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tracing::{error, info};

// ============================================================================
// Mirror Capture Store  (global, for test validation)
// ============================================================================
//
// RequestMirror integration tests need to verify that a mirrored request was
// actually received by the mirror backend. The /mirror/capture endpoint stores
// each arriving mirrored request (keyed by the x-trace-id header echoed by the
// gateway), and /mirror/query/{trace_id} lets the test client poll for receipt.

#[derive(Debug, Clone, serde::Serialize)]
struct MirrorCapture {
    /// Timestamp (unix millis) when the mirror request was received.
    received_at_ms: u64,
    /// HTTP method of the mirrored request.
    method: String,
    /// Request path.
    path: String,
    /// All headers (lowercase keys).
    headers: std::collections::HashMap<String, String>,
    /// Request body as UTF-8 string (truncated to 4KB).
    body: String,
}

static MIRROR_STORE: std::sync::LazyLock<Mutex<std::collections::HashMap<String, Vec<MirrorCapture>>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Retry-sequence counter: tracks how many times each test-id has been called
static RETRY_SEQUENCE_COUNTER: std::sync::LazyLock<Mutex<std::collections::HashMap<String, Vec<u16>>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Circuit-breaker test counter: tracks calls per test-id
static CB_COUNTER: std::sync::LazyLock<Mutex<std::collections::HashMap<String, u32>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

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

    /// TCP server port with Proxy Protocol v2 parsing
    #[arg(long)]
    tcp_pp2_port: Option<u16>,

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

    /// H2 raw backend port that sends protocol-violating streams (for H2TrailerMalformed testing)
    #[arg(long)]
    h2_trailer_malformed_port: Option<u16>,

    /// HTTPS backend multi-SNI port (for BackendTLSSNIHostOverride testing)
    /// The server uses a per-SNI cert resolver: hostname-a.example.com -> cert-a,
    /// hostname-b.example.com -> cert-b, any other SNI -> handshake rejected.
    #[arg(long)]
    https_backend_multi_sni_port: Option<u16>,

    /// TLS certificate file for multi-SNI backend (cert A, SAN: hostname-a.example.com)
    #[arg(long)]
    multi_sni_cert_a: Option<String>,

    /// TLS private key file for multi-SNI backend (key A)
    #[arg(long)]
    multi_sni_key_a: Option<String>,

    /// TLS certificate file for multi-SNI backend (cert B, SAN: hostname-b.example.com)
    #[arg(long)]
    multi_sni_cert_b: Option<String>,

    /// TLS private key file for multi-SNI backend (key B)
    #[arg(long)]
    multi_sni_key_b: Option<String>,

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

    // Start TCP PP2 server (if configured)
    if let Some(pp2_port) = cli.tcp_pp2_port {
        let handle = tokio::spawn(start_tcp_pp2_server(pp2_port));
        handles.push(handle);
    }

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

    // Start H2 trailer-malformed raw backend server (if configured)
    if let Some(h2tm_port) = cli.h2_trailer_malformed_port {
        let handle = tokio::spawn(start_h2_trailer_malformed_server(h2tm_port));
        handles.push(handle);
    }

    // Start HTTPS multi-SNI backend server (if configured)
    if let Some(multi_sni_port) = cli.https_backend_multi_sni_port {
        match (
            cli.multi_sni_cert_a.as_ref(),
            cli.multi_sni_key_a.as_ref(),
            cli.multi_sni_cert_b.as_ref(),
            cli.multi_sni_key_b.as_ref(),
        ) {
            (Some(cert_a), Some(key_a), Some(cert_b), Some(key_b)) => {
                let handle = tokio::spawn(start_https_backend_multi_sni_server(
                    multi_sni_port,
                    cert_a.clone(),
                    key_a.clone(),
                    cert_b.clone(),
                    key_b.clone(),
                ));
                handles.push(handle);
            }
            _ => {
                error!(
                    "https-backend-multi-sni-port specified but one or more of \
                     multi-sni-cert-a / multi-sni-key-a / multi-sni-cert-b / multi-sni-key-b \
                     is missing"
                );
            }
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
        .route("/version", get(version_handler))
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
        .route(
            "/webhook/resolve/extra",
            get(webhook_resolve_extra_handler).post(webhook_resolve_extra_handler),
        )
        .route(
            "/webhook/apikey-resolve",
            get(webhook_apikey_resolve_handler).post(webhook_apikey_resolve_handler),
        )
        .route("/webhook/healthz", get(health_handler))
        .route("/status/{code}", get(status_handler))
        .route("/delay/{seconds}", get(delay_handler))
        // --- Mirror endpoints (for RequestMirror plugin integration tests) ---
        // POST/GET to /mirror/capture: gateway mirrors requests here; stored by x-trace-id
        .route(
            "/mirror/capture",
            get(mirror_capture_handler).post(mirror_capture_handler),
        )
        // GET /mirror/query/{trace_id}: poll whether a mirrored request was received
        .route("/mirror/query/{trace_id}", get(mirror_query_handler))
        // GET /mirror/slow/{ms}: respond after a delay (simulate slow mirror backend)
        .route("/mirror/slow/{ms}", get(mirror_slow_handler).post(mirror_slow_handler))
        // GET /mirror/reset: clear all stored mirror captures (between test cases)
        .route("/mirror/reset", get(mirror_reset_handler))
        // --- Retry-sequence endpoints (for retry-on-status integration tests) ---
        .route("/retry-sequence/{test_id}/{status_list}", get(retry_sequence_handler))
        .route("/retry-sequence-reset", get(retry_sequence_reset_handler))
        // --- Circuit-breaker endpoints (for circuit-breaker integration tests) ---
        .route("/cb-fail/{test_id}/{fail_count}", get(cb_fail_handler))
        .route("/cb-reset", get(cb_reset_handler))
        .route("/{*path}", get(catch_all_handler))
        .layer(Extension(server_addr_str))
}

fn display_header_name(name: &str) -> String {
    name.split('-')
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

fn format_http_version(version: Version) -> &'static str {
    match version {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2.0",
        Version::HTTP_3 => "HTTP/3.0",
        _ => "HTTP/unknown",
    }
}

async fn version_handler(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(server_addr): Extension<String>,
    req: AxumRequest<Body>,
) -> impl IntoResponse {
    format!(
        "Server: {}\nClient: {}\nVersion: {}\n",
        server_addr,
        addr,
        format_http_version(req.version())
    )
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
// ============================================================================
// Mirror handlers (for RequestMirror plugin integration tests)
// ============================================================================
/// /mirror/capture  — receive mirrored requests and store them by x-trace-id.
///
/// The gateway's RequestMirror plugin copies the original request (including its
/// x-trace-id header) to this backend. Tests send an x-trace-id with the
/// original request, and later poll /mirror/query/{trace_id} to verify receipt.
async fn mirror_capture_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    let received_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    // Collect headers
    let mut headers = std::collections::HashMap::new();
    let trace_id = req
        .headers()
        .get("x-trace-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    for (key, value) in req.headers() {
        if let Ok(val) = value.to_str() {
            headers.insert(key.as_str().to_lowercase(), val.to_string());
        }
    }

    // Read body (truncate at 4KB)
    let body_bytes = to_bytes(req.into_body(), 4096).await.unwrap_or_default();
    let body = String::from_utf8_lossy(&body_bytes).into_owned();

    let capture = MirrorCapture {
        received_at_ms,
        method,
        path,
        headers,
        body,
    };

    // Store under trace_id
    {
        let mut store = MIRROR_STORE.lock();
        store.entry(trace_id.clone()).or_default().push(capture);
        info!("mirror/capture: stored request for trace_id={}", trace_id);
    }

    (StatusCode::OK, "mirror-captured")
}

/// /mirror/query/{trace_id} — return all captured mirror requests for a trace_id.
///
/// Returns JSON: `{"found": true/false, "count": N, "captures": [...]}`.
/// The test client polls this endpoint (with retry) to verify the mirror arrived.
async fn mirror_query_handler(Path(trace_id): Path<String>) -> impl IntoResponse {
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;

    let (found, count, captures) = {
        let store = MIRROR_STORE.lock();
        if let Some(entries) = store.get(&trace_id) {
            let data: Vec<_> = entries
                .iter()
                .map(|c| serde_json::to_value(c).unwrap_or_default())
                .collect();
            (true, data.len(), data)
        } else {
            (false, 0, vec![])
        }
    };

    (
        StatusCode::OK,
        Json(json!({"found": found, "count": count, "captures": captures})),
    )
}

/// /mirror/slow/{ms} — respond after `ms` milliseconds.
///
/// Used by tests to simulate a slow mirror backend that causes the body channel
/// to fill up, triggering the channel_full_timeout_ms logic.
async fn mirror_slow_handler(Path(ms): Path<u64>, req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    // Still capture the request (so slow tests can verify receipt after the delay)
    let received_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let mut headers = std::collections::HashMap::new();
    let trace_id = req
        .headers()
        .get("x-trace-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    for (key, value) in req.headers() {
        if let Ok(val) = value.to_str() {
            headers.insert(key.as_str().to_lowercase(), val.to_string());
        }
    }
    let body_bytes = to_bytes(req.into_body(), 4096).await.unwrap_or_default();
    let body = String::from_utf8_lossy(&body_bytes).into_owned();
    let capture = MirrorCapture {
        received_at_ms,
        method,
        path,
        headers,
        body,
    };
    {
        let mut store = MIRROR_STORE.lock();
        store.entry(trace_id.clone()).or_default().push(capture);
    }

    // Simulate slow processing
    let delay_ms = ms.min(10_000); // cap at 10s for safety
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;

    (StatusCode::OK, format!("mirror-slow: delayed {}ms", delay_ms))
}

/// /mirror/reset — clear all stored mirror captures.
///
/// Call between test cases to avoid stale data from previous runs.
async fn mirror_reset_handler() -> impl IntoResponse {
    use axum::http::StatusCode;
    {
        let mut store = MIRROR_STORE.lock();
        let count = store.len();
        store.clear();
        info!("mirror/reset: cleared {} trace entries", count);
    }
    (StatusCode::OK, "mirror-store-reset")
}

/// /retry-sequence/{test_id}/{status_list}
/// Returns status codes in sequence per test_id.
/// status_list format: "502,503,200" — returns 502 on 1st call, 503 on 2nd, 200 on 3rd+
async fn retry_sequence_handler(Path((test_id, status_list)): Path<(String, String)>) -> impl IntoResponse {
    use axum::http::StatusCode;

    let statuses: Vec<u16> = status_list
        .split(',')
        .filter_map(|s| s.trim().parse::<u16>().ok())
        .collect();

    if statuses.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            [
                ("x-error".to_string(), "empty or invalid status_list".to_string()),
                ("x-attempt-number".to_string(), "0".to_string()),
                ("x-attempt-history".to_string(), String::new()),
                ("x-test-id".to_string(), test_id),
            ],
            "Bad status_list".to_string(),
        )
            .into_response();
    }

    let (attempt, current_status) = {
        let mut store = RETRY_SEQUENCE_COUNTER.lock();
        let history = store.entry(test_id.clone()).or_default();
        let idx = history.len();
        // Use last status code for out-of-range attempts
        let status = statuses.get(idx).copied().unwrap_or(*statuses.last().unwrap());
        history.push(status);
        (idx + 1, status)
    };

    let history_str = {
        let store = RETRY_SEQUENCE_COUNTER.lock();
        store
            .get(&test_id)
            .map(|h| h.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(","))
            .unwrap_or_default()
    };

    let status = StatusCode::from_u16(current_status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    (
        status,
        [
            ("x-attempt-number".to_string(), attempt.to_string()),
            ("x-attempt-history".to_string(), history_str),
            ("x-test-id".to_string(), test_id),
        ],
        format!("attempt={}, status={}", attempt, current_status),
    )
        .into_response()
}

/// /retry-sequence-reset — clear all retry sequence state
async fn retry_sequence_reset_handler() -> impl IntoResponse {
    let mut store = RETRY_SEQUENCE_COUNTER.lock();
    let count = store.len();
    store.clear();
    info!("retry-sequence/reset: cleared {} test entries", count);
    (axum::http::StatusCode::OK, format!("cleared {} entries", count))
}

/// /cb-fail/{test_id}/{fail_count}
/// Returns 502 for the first `fail_count` calls per test_id, then 200.
async fn cb_fail_handler(Path((test_id, fail_count)): Path<(String, u32)>) -> impl IntoResponse {
    use axum::http::StatusCode;

    let attempt = {
        let mut store = CB_COUNTER.lock();
        let count = store.entry(test_id.clone()).or_insert(0);
        *count += 1;
        *count
    };

    let status = if attempt <= fail_count {
        StatusCode::BAD_GATEWAY // 502
    } else {
        StatusCode::OK
    };

    (
        status,
        [("x-cb-attempt", attempt.to_string()), ("x-cb-test-id", test_id)],
        format!("attempt={}", attempt),
    )
}

/// /cb-reset — clear all CB test state
async fn cb_reset_handler() -> impl IntoResponse {
    use axum::http::StatusCode;

    let mut store = CB_COUNTER.lock();
    store.clear();
    (StatusCode::OK, "cb-reset-ok")
}

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

/// Endpoint for pathSuffix testing — returns a payload that proves the suffix routed here.
async fn webhook_resolve_extra_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::http::StatusCode;

    let tenant = req
        .headers()
        .get("x-tenant-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let body = format!(r#"{{"data":{{"path_suffix_id":"suffix-{tenant}"}}}}"#);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
}

/// Endpoint for apiKey-auth testing — returns 401 unless X-API-Key header present and correct.
async fn webhook_apikey_resolve_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::http::StatusCode;

    let api_key = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if api_key != "test-secret-key-12345" {
        return (
            StatusCode::UNAUTHORIZED,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            r#"{"error":"missing or wrong api key"}"#.to_string(),
        );
    }
    let tenant = req
        .headers()
        .get("x-tenant-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let body = format!(r#"{{"data":{{"apikey_user":"apikey-{tenant}"}}}}"#);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
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

// --------------------------------------------------------------------------
// gRPC Health Check Protocol (grpc.health.v1)
// Hand-written prost definitions — standard protocol, no proto file needed.
// --------------------------------------------------------------------------
mod grpc_health {
    #[derive(Clone, prost::Message)]
    pub struct HealthCheckRequest {
        #[prost(string, tag = "1")]
        pub service: String,
    }

    #[derive(Clone, prost::Message)]
    pub struct HealthCheckResponse {
        #[prost(int32, tag = "1")]
        pub status: i32,
    }

    pub const SERVING: i32 = 1;
    pub const NOT_SERVING: i32 = 2;
}

/// gRPC Health service implementation.
/// Service name "unhealthy" returns NOT_SERVING; everything else returns SERVING.
#[derive(Debug, Clone)]
struct GrpcHealthService;

impl GrpcHealthService {
    async fn check(
        &self,
        request: Request<grpc_health::HealthCheckRequest>,
    ) -> Result<Response<grpc_health::HealthCheckResponse>, Status> {
        let service_name = request.into_inner().service;
        let status = if service_name == "unhealthy" {
            grpc_health::NOT_SERVING
        } else {
            grpc_health::SERVING
        };
        Ok(Response::new(grpc_health::HealthCheckResponse { status }))
    }
}

/// Server-side implementation of grpc.health.v1.Health as a tonic service.
/// Follows the same pattern as tonic-generated server code.
#[derive(Debug, Clone)]
struct GrpcHealthServer {
    inner: std::sync::Arc<GrpcHealthService>,
}

impl tonic::server::NamedService for GrpcHealthServer {
    const NAME: &'static str = "grpc.health.v1.Health";
}

impl<B> tower::Service<http::Request<B>> for GrpcHealthServer
where
    B: tonic::codegen::Body + Send + 'static,
    B::Error: Into<tonic::codegen::StdError> + Send + 'static,
{
    type Response = http::Response<tonic::body::BoxBody>;
    type Error = std::convert::Infallible;
    type Future = tonic::codegen::BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        #[allow(non_camel_case_types)]
        struct CheckSvc(std::sync::Arc<GrpcHealthService>);
        impl tonic::server::UnaryService<grpc_health::HealthCheckRequest> for CheckSvc {
            type Response = grpc_health::HealthCheckResponse;
            type Future = tonic::codegen::BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
            fn call(&mut self, request: tonic::Request<grpc_health::HealthCheckRequest>) -> Self::Future {
                let inner = std::sync::Arc::clone(&self.0);
                let fut = async move { inner.check(request).await };
                Box::pin(fut)
            }
        }

        let inner = self.inner.clone();
        match req.uri().path() {
            "/grpc.health.v1.Health/Check" => {
                let fut = async move {
                    let mut grpc = tonic::server::Grpc::new(tonic::codec::ProstCodec::default());
                    let resp = grpc.unary(CheckSvc(inner), req).await;
                    Ok(resp)
                };
                Box::pin(fut)
            }
            _ => Box::pin(async move {
                Ok(http::Response::builder()
                    .status(200)
                    .header("grpc-status", "12") // UNIMPLEMENTED
                    .header("content-type", "application/grpc")
                    .body(tonic::body::empty_body())
                    .unwrap())
            }),
        }
    }
}

async fn start_grpc_server(port: u16) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let server_addr_str = format!("0.0.0.0:{}", port);

    let service = TestServiceImpl::new(server_addr_str);

    info!("✓ gRPC server listening on http://{}", addr);

    Server::builder()
        .add_service(TestServiceServer::new(service))
        .add_service(GrpcHealthServer {
            inner: std::sync::Arc::new(GrpcHealthService),
        })
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
// TCP Server with Proxy Protocol v2 Parsing
// ============================================================================

async fn start_tcp_pp2_server(port: u16) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;

    info!("✓ TCP-PP2 server listening on tcp://{} (Proxy Protocol v2 aware)", addr);

    loop {
        match listener.accept().await {
            Ok((mut socket, peer_addr)) => {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];

                    // Read initial data (PP2 header + possibly app data)
                    let mut total_read = match socket.read(&mut buf).await {
                        Ok(0) => return,
                        Ok(n) => n,
                        Err(_) => return,
                    };

                    // Try to parse PP2 header
                    match proxy_header::ProxyHeader::parse(&buf[..total_read], proxy_header::ParseConfig::default()) {
                        Ok((header, consumed)) => {
                            let (src_addr, dst_addr) = match header.proxied_address() {
                                Some(addr) => (addr.source.to_string(), addr.destination.to_string()),
                                None => ("local".to_string(), "local".to_string()),
                            };

                            let authority = header.authority().unwrap_or("").to_string();

                            let response = serde_json::json!({
                                "pp2": true,
                                "src_addr": src_addr,
                                "dst_addr": dst_addr,
                                "authority": authority,
                                "peer_addr": peer_addr.to_string(),
                                "pp2_header_len": consumed,
                            });

                            let response_bytes = serde_json::to_vec(&response).unwrap();
                            let _ = socket.write_all(&response_bytes).await;

                            // If there was app data after the PP2 header, echo it too
                            if consumed < total_read {
                                let _ = socket.write_all(&buf[consumed..total_read]).await;
                            }
                        }
                        Err(proxy_header::Error::BufferTooShort) => {
                            // Need more data — read more and retry
                            match socket.read(&mut buf[total_read..]).await {
                                Ok(0) => {}
                                Ok(n) => {
                                    total_read += n;
                                    if let Ok((header, consumed)) = proxy_header::ProxyHeader::parse(
                                        &buf[..total_read],
                                        proxy_header::ParseConfig::default(),
                                    ) {
                                        let (src_addr, dst_addr) = match header.proxied_address() {
                                            Some(addr) => (addr.source.to_string(), addr.destination.to_string()),
                                            None => ("local".to_string(), "local".to_string()),
                                        };
                                        let authority = header.authority().unwrap_or("").to_string();
                                        let response = serde_json::json!({
                                            "pp2": true,
                                            "src_addr": src_addr,
                                            "dst_addr": dst_addr,
                                            "authority": authority,
                                            "peer_addr": peer_addr.to_string(),
                                            "pp2_header_len": consumed,
                                        });
                                        let response_bytes = serde_json::to_vec(&response).unwrap();
                                        let _ = socket.write_all(&response_bytes).await;
                                        if consumed < total_read {
                                            let _ = socket.write_all(&buf[consumed..total_read]).await;
                                        }
                                    } else {
                                        let response = serde_json::json!({
                                            "pp2": false,
                                            "error": "failed to parse PP2 after retry",
                                        });
                                        let _ = socket.write_all(&serde_json::to_vec(&response).unwrap()).await;
                                    }
                                }
                                Err(_) => {}
                            }
                        }
                        Err(_) => {
                            // Not a PP2 connection — return error JSON + echo data
                            let response = serde_json::json!({
                                "pp2": false,
                                "error": "no valid PP2 header",
                                "peer_addr": peer_addr.to_string(),
                            });
                            let response_bytes = serde_json::to_vec(&response).unwrap();
                            let _ = socket.write_all(&response_bytes).await;
                            let _ = socket.write_all(&buf[..total_read]).await;
                        }
                    }
                });
            }
            Err(e) => {
                error!("TCP-PP2 accept error: {}", e);
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
        "token_endpoint": format!("{}/token", endpoint_base),
        "userinfo_endpoint": format!("{}/userinfo", endpoint_base),
        "authorization_endpoint": format!("{}/authorize", endpoint_base),
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

// ============================================================================
// H2 Trailer-Malformed Raw Backend (for H2TrailerMalformed protocol testing)
// ============================================================================
//
// This backend speaks raw HTTP/2 via the `h2` crate and deliberately produces
// a protocol-violating stream to exercise Pingora's upstream H2 reader.
//
// Violation chosen: send RST_STREAM(REFUSED_STREAM) before any HEADERS frame.
// The stream is reset by the server before response status is committed, so
// Pingora's upstream H2 client sees the failure with no headers in flight and
// must surface a 5xx to the downstream client (status not yet sent).
//
// Alternatives considered:
//   1. Drop SendStream after sending HEADERS(200) + partial DATA without
//      END_STREAM. The h2 crate emits RST_STREAM(CANCEL) on drop, but by then
//      the 200 headers were already proxied to the downstream client, so
//      Pingora can only reset the downstream stream — the test sees 200 OK
//      instead of 5xx, defeating the assertion.
//   2. Send a trailers HEADERS frame containing `transfer-encoding: chunked`
//      (forbidden in HTTP/2 per RFC 9113 §8.2.2). The h2 crate's
//      `send_trailers()` rejects this at the library level before any bytes
//      hit the wire.
// The pre-headers RST_STREAM approach produces a wire-level anomaly that
// Pingora must handle and that the downstream-facing assertion can catch.

async fn start_h2_trailer_malformed_server(port: u16) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    info!("H2 trailer-malformed server listening on 0.0.0.0:{}", port);

    loop {
        match listener.accept().await {
            Ok((tcp, peer)) => {
                info!("H2TrailerMalformed: accepted connection from {}", peer);
                tokio::spawn(async move {
                    if let Err(e) = handle_h2_trailer_malformed_conn(tcp).await {
                        // Log at info level so the test-server output shows what happened.
                        // This is expected behaviour (Pingora will reset the stream).
                        info!("H2TrailerMalformed: connection handler finished: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("H2TrailerMalformed: accept error: {}", e);
            }
        }
    }
}

async fn handle_h2_trailer_malformed_conn(tcp: tokio::net::TcpStream) -> Result<()> {
    let mut conn = h2s::handshake(tcp).await?;

    while let Some(request) = conn.accept().await {
        let (req, mut respond) = request?;
        info!("H2TrailerMalformed: received {} {}", req.method(), req.uri().path());

        // Reset the stream with REFUSED_STREAM before sending any HEADERS
        // frame. Pingora's upstream H2 reader observes the reset before it
        // has committed any response status downstream, so it can return a
        // 5xx to the client.
        respond.send_reset(h2::Reason::REFUSED_STREAM);
    }

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

// ============================================================================
// Multi-SNI HTTPS Backend (for BackendTLSSNIHostOverride testing)
// ============================================================================
//
// The server uses a per-SNI certificate resolver that routes incoming TLS
// ClientHello messages to one of two certificates based on server_name:
//
//   SNI = "hostname-a.example.com"  ->  cert A  (SAN: hostname-a.example.com)
//   SNI = "hostname-b.example.com"  ->  cert B  (SAN: hostname-b.example.com)
//   any other SNI / no SNI          ->  None    (rustls sends unrecognized_name alert)
//
// This makes it possible to prove that the Gateway sends the correct SNI to
// the upstream backend as required by BackendTLSPolicy.spec.validation.hostname:
// both routes only succeed when the Gateway uses exactly the right per-policy
// hostname as the TLS SNI extension value.

use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

#[derive(Debug)]
struct MultiSniResolver {
    cert_a: Arc<CertifiedKey>,
    cert_b: Arc<CertifiedKey>,
}

impl ResolvesServerCert for MultiSniResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        match hello.server_name() {
            Some("hostname-a.example.com") => Some(self.cert_a.clone()),
            Some("hostname-b.example.com") => Some(self.cert_b.clone()),
            // Reject all other SNI values (including absent SNI).
            // rustls translates None return into an unrecognized_name TLS alert,
            // which causes the TLS handshake to fail — proving that the backend
            // won't serve traffic for wrong-SNI connections.
            _ => None,
        }
    }
}

fn load_certified_key(cert_path: &str, key_path: &str) -> Result<Arc<CertifiedKey>> {
    let cert_chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert_path)
        .with_context(|| format!("failed to read cert file {}", cert_path))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("failed to parse cert chain {}", cert_path))?;

    let private_key =
        PrivateKeyDer::from_pem_file(key_path).with_context(|| format!("failed to parse private key {}", key_path))?;

    let signing_key = rustls::crypto::ring::sign::any_supported_type(&private_key)
        .with_context(|| format!("unsupported private key type in {}", key_path))?;

    Ok(Arc::new(CertifiedKey::new(cert_chain, signing_key)))
}

async fn start_https_backend_multi_sni_server(
    port: u16,
    cert_a_path: String,
    key_a_path: String,
    cert_b_path: String,
    key_b_path: String,
) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let server_addr_str = format!("0.0.0.0:{}", port);

    let cert_a = load_certified_key(&cert_a_path, &key_a_path)?;
    let cert_b = load_certified_key(&cert_b_path, &key_b_path)?;

    let resolver = Arc::new(MultiSniResolver { cert_a, cert_b });

    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);

    // Advertise HTTP/1.1 only; this test verifies SNI dispatch, not protocol version.
    tls_config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let tls_config = RustlsConfig::from_config(Arc::new(tls_config));
    let app = create_echo_router(server_addr_str.clone());

    info!("Multi-SNI HTTPS backend listening on https://{}", addr);
    info!("  Cert A (hostname-a.example.com): {}", cert_a_path);
    info!("  Cert B (hostname-b.example.com): {}", cert_b_path);

    let app_with_connect_info = app.into_make_service_with_connect_info::<SocketAddr>();
    axum_server::bind_rustls(addr, tls_config)
        .serve(app_with_connect_info)
        .await?;

    Ok(())
}
