// Edgion Unified Test Server
// Supports all protocols: HTTP/HTTPS, gRPC, WebSocket, TCP, UDP

use anyhow::{Context, Result};
use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, Extension, Path, Query, Request as AxumRequest},
    http::Version,
    response::IntoResponse,
    routing::get,
    Router,
};
use base64::Engine as _;
use clap::Parser;
use h2::server as h2s;
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tracing::{error, info};

// Transport-shape backends + the transport event recorder. Lives in its own
// module tree because it is a self-contained protocol harness (raw H1, raw H2,
// a global event log) rather than another axum endpoint.
mod l4_duplex;
#[path = "transport/mod.rs"]
mod transport;

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

/// HTTP DNS resolve counter: tracks how many times the fake HTTP DNS endpoint
/// resolved each host. Lets tests assert caching behaviour (a `gateway`-mode
/// resolution should NOT re-hit the endpoint within its TTL).
static DNS_RESOLVE_COUNTER: std::sync::LazyLock<Mutex<std::collections::HashMap<String, u32>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Total requests that reached the native Anthropic fixture. The integration
/// client samples this around a locally rejected request to prove the provider
/// was not contacted; no request data is retained.
static ANTHROPIC_REQUEST_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const OAUTH2_ACTIVE_CLIENT_ID: &str = "oauth2-active-client";
const OAUTH2_ACTIVE_CLIENT_SECRET: &str = "active-secret";
const OAUTH2_PREVIOUS_CLIENT_ID: &str = "oauth2-previous-client";
const OAUTH2_PREVIOUS_CLIENT_SECRET: &str = "previous-secret";

/// Per-scenario, fixed-label state for AI resilience integration tests. Only
/// an opaque fixture ID, bounded server-owned modes, and counters are kept.
static AI_JSON_USAGE_RELEASES: std::sync::LazyLock<Mutex<std::collections::HashMap<String, Arc<tokio::sync::Notify>>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

static AI_RESILIENCE_STATE: std::sync::LazyLock<Mutex<std::collections::HashMap<String, AiResilienceFixtureState>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

#[derive(Clone, Copy, Debug, Default)]
enum AiResiliencePrimaryMode {
    #[default]
    Healthy,
    Fail,
    ProbeSuccess,
    ProbeFail,
}

#[derive(Debug, Default)]
struct AiResilienceFixtureState {
    counts: [u64; 6],
    half_open_primary_mode: AiResiliencePrimaryMode,
    half_open_probe_release: std::sync::Arc<tokio::sync::Notify>,
}

/// Per-trace provider observations for the credential-exhaustion scenario.
/// Only fixture-owned service labels and non-secret credential generations are
/// retained; request bodies and authorization values never enter this store.
static AI_CREDENTIAL_EXHAUSTION_OBSERVATIONS: std::sync::LazyLock<
    Mutex<std::collections::HashMap<String, Vec<AiCredentialExhaustionObservation>>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Per-trace lifecycle of the paced AI stream used to verify that a downstream
/// client disconnect cancels the gateway's upstream body consumption.
static AI_LONG_STREAM_OBSERVATIONS: std::sync::LazyLock<
    Mutex<std::collections::HashMap<String, AiLongStreamObservation>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Bounded, content-free observations for Guardrail integration assertions.
/// The fixture retains only counters and structural validation booleans, never
/// semantic window text or configured authentication headers.
static GUARDRAIL_OBSERVATIONS: std::sync::LazyLock<
    Mutex<std::collections::HashMap<String, GuardrailFixtureObservation>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GuardrailFixtureObservation {
    calls: u64,
    total_windows: u64,
    last_window_count: usize,
    version_v1: bool,
    minimal_envelope: bool,
    sorted_unique_keys: bool,
    canonical_content_window: bool,
}

#[derive(Debug, serde::Serialize)]
struct AiCredentialExhaustionObservation {
    backend: &'static str,
    credential_generation: &'static str,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
struct AiLongStreamObservation {
    chunks_queued: u64,
    upstream_cancelled: bool,
    provider_completed: bool,
}

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

    /// Controlled L4 duplex fixture plain TCP port
    #[arg(long)]
    l4_duplex_port: Option<u16>,

    /// Controlled L4 duplex fixture TLS port (uses cert-file and key-file)
    #[arg(long)]
    l4_duplex_tls_port: Option<u16>,

    /// UDP server port
    #[arg(long, default_value = "30011")]
    udp_port: u16,

    /// TCP server port with Proxy Protocol v2 parsing
    #[arg(long)]
    tcp_pp2_port: Option<u16>,

    /// Fake auth server port (for ForwardAuth plugin testing)
    #[arg(long)]
    auth_port: Option<u16>,

    /// Fake ext_proc gRPC server port (for ExtProc plugin testing)
    #[arg(long)]
    ext_proc_port: Option<u16>,

    /// Fake ext_proc gRPC server TLS port (for ExtProc-over-TLS testing).
    /// Serves the same processor over HTTPS/h2; needs --ext-proc-tls-cert/key.
    #[arg(long)]
    ext_proc_tls_port: Option<u16>,

    /// Fake ext_proc gRPC server mTLS port (requires a client certificate).
    #[arg(long)]
    ext_proc_mtls_port: Option<u16>,

    /// TLS certificate file for the ext_proc TLS server (SAN must cover 127.0.0.1).
    #[arg(long)]
    ext_proc_tls_cert: Option<String>,

    /// TLS private key file for the ext_proc TLS server.
    #[arg(long)]
    ext_proc_tls_key: Option<String>,

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

    /// Raw HTTP/1.1 transport-shape backend port (default 30030 in the launch
    /// scripts). Steered per request by the `x-up-*` control headers; every
    /// wire-level fact it observes lands in the transport event recorder,
    /// queryable at `/transport/query/{trace_id}` on the HTTP router.
    #[arg(long)]
    transport_h1_port: Option<u16>,

    /// Raw HTTP/2 (h2c prior-knowledge) transport-shape backend port
    /// (default 30031 in the launch scripts). Same `x-up-*` vocabulary as the
    /// H1 backend, plus RST_STREAM/GOAWAY with a chosen error code.
    #[arg(long)]
    transport_h2_port: Option<u16>,

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

    /// HTTPS backend wildcard port (for the EdgionBackendTrafficPolicy/UpstreamAuthority
    /// integration scenario). Serves a genuine `*.sandbox.internal` wildcard certificate;
    /// the TLS resolver accepts any SNI inside that pattern (rejecting the bare parent
    /// domain and anything outside it), so a successful handshake already proves the
    /// gateway sent an in-pattern SNI. The exact composed authority is then verified at
    /// the HTTP layer via the `Host` header the shared `/echo` handler echoes back.
    #[arg(long)]
    https_backend_wildcard_port: Option<u16>,

    /// TLS certificate file for the wildcard backend (SAN: *.sandbox.internal)
    #[arg(long)]
    wildcard_cert: Option<String>,

    /// TLS private key file for the wildcard backend
    #[arg(long)]
    wildcard_key: Option<String>,

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

    if let Some(port) = cli.l4_duplex_port {
        handles.push(tokio::spawn(l4_duplex::start(port, None)));
    }
    if let Some(port) = cli.l4_duplex_tls_port {
        let identity = (
            cli.cert_file.clone().context("L4 TLS fixture requires cert-file")?,
            cli.key_file.clone().context("L4 TLS fixture requires key-file")?,
        );
        handles.push(tokio::spawn(l4_duplex::start(port, Some(identity))));
    }

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

    // Start Fake ext_proc server (if configured)
    if let Some(ext_proc_port) = cli.ext_proc_port {
        let handle = tokio::spawn(ext_proc_server::start(ext_proc_port));
        handles.push(handle);
    }

    // Start Fake ext_proc TLS server (if configured)
    if let Some(ext_proc_tls_port) = cli.ext_proc_tls_port {
        match (cli.ext_proc_tls_cert.clone(), cli.ext_proc_tls_key.clone()) {
            (Some(cert), Some(key)) => {
                let handle = tokio::spawn(ext_proc_server::start_tls(ext_proc_tls_port, cert, key, None));
                handles.push(handle);
            }
            _ => error!(
                "--ext-proc-tls-port set but --ext-proc-tls-cert/--ext-proc-tls-key missing; skipping TLS ext_proc"
            ),
        }
    }

    // Start Fake ext_proc mTLS server (if configured)
    if let Some(ext_proc_mtls_port) = cli.ext_proc_mtls_port {
        match (
            cli.ext_proc_tls_cert.clone(),
            cli.ext_proc_tls_key.clone(),
            cli.client_ca_file.clone(),
        ) {
            (Some(cert), Some(key), Some(client_ca)) => {
                let handle = tokio::spawn(ext_proc_server::start_tls(
                    ext_proc_mtls_port,
                    cert,
                    key,
                    Some(client_ca),
                ));
                handles.push(handle);
            }
            _ => error!(
                "--ext-proc-mtls-port set but ext_proc TLS identity or --client-ca-file missing; skipping mTLS ext_proc"
            ),
        }
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

    // Start the raw HTTP/1.1 transport-shape backend (if configured)
    if let Some(port) = cli.transport_h1_port {
        let handle = tokio::spawn(transport::h1::start_transport_h1_server(port));
        handles.push(handle);
    }

    // Start the raw HTTP/2 transport-shape backend (if configured)
    if let Some(port) = cli.transport_h2_port {
        let handle = tokio::spawn(transport::h2::start_transport_h2_server(port));
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

    // Start HTTPS wildcard backend server (if configured)
    if let Some(wildcard_port) = cli.https_backend_wildcard_port {
        match (cli.wildcard_cert.as_ref(), cli.wildcard_key.as_ref()) {
            (Some(cert), Some(key)) => {
                let handle = tokio::spawn(start_https_backend_wildcard_server(
                    wildcard_port,
                    cert.clone(),
                    key.clone(),
                ));
                handles.push(handle);
            }
            _ => {
                error!("https-backend-wildcard-port specified but wildcard-cert / wildcard-key is missing");
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
    // Best-effort IPv6 loopback listener on the same port. A backend that
    // resolves `localhost` to `::1` (dual-stack hosts, e.g. the DnsResolution
    // system-resolver scenarios) must still reach the test server. Bound
    // separately from 0.0.0.0 (distinct address family) so there is no conflict;
    // non-fatal if IPv6 is unavailable.
    tokio::spawn(async move {
        let v6_addr = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port));
        match TcpListener::bind(v6_addr).await {
            Ok(listener) => {
                info!("✓ HTTP server also listening on http://{}", v6_addr);
                let app =
                    create_echo_router(format!("[::1]:{}", port)).into_make_service_with_connect_info::<SocketAddr>();
                if let Err(e) = axum::serve(listener, app).await {
                    error!("IPv6 HTTP server error on port {}: {}", port, e);
                }
            }
            // Warn (not info): the system-resolver DNS scenarios resolve
            // `localhost` to ::1 on dual-stack hosts and depend on this listener,
            // so an unexpected bind failure must be visible.
            Err(e) => tracing::warn!("IPv6 loopback bind skipped on port {}: {}", port, e),
        }
    });

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
        .route("/body-digest", get(body_digest_handler).post(body_digest_handler))
        .route("/body-redirect", get(body_redirect_handler).post(body_redirect_handler))
        .route("/body-slow", get(body_slow_handler).post(body_slow_handler))
        .route("/request-header-test", get(header_echo_handler))
        .route("/both-headers-test", get(header_echo_handler))
        .route("/multi-request-headers", get(header_echo_handler))
        .route("/multi-value-append-test", get(header_echo_handler))
        .route("/response-header-test", get(header_echo_handler))
        .route("/headers", get(headers_handler))
        .route("/vhost-health", get(vhost_health_handler))
        .route("/multi-set-cookie", get(multi_set_cookie_handler))
        .route("/auth-header-probe", get(auth_header_probe_handler))
        .route("/oauth2/token", axum::routing::post(oauth2_token_handler))
        .route(
            "/webhook/resolve",
            get(webhook_resolve_handler).post(webhook_resolve_handler),
        )
        // Path-override scenarios reuse the same echo handler; the extracted
        // `data.path` proves which path the gateway actually called.
        .route(
            "/webhook/default",
            get(webhook_resolve_handler).post(webhook_resolve_handler),
        )
        .route(
            "/webhook/override",
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
        .route("/status-empty/{code}", get(status_empty_handler))
        .route("/delay/{seconds}", get(delay_handler))
        // GET /sse?events=N&interval_ms=T: paced SSE stream for streaming tests.
        .route("/sse", get(sse_handler))
        // OpenAI-compatible deterministic provider used by the AI Gateway
        // integration suite. It reports only booleans and fixed fixture IDs;
        // credentials and arbitrary request values are never echoed.
        .route("/ai/{*path}", get(ai_provider_handler).post(ai_provider_handler))
        // Bounded deterministic unary policy fixture for the Guardrail plugin.
        // It shares this server with the AI provider so integration coverage
        // does not add another process or network topology.
        .route(
            "/guardrail/decision/{action}",
            axum::routing::post(guardrail_decision_handler),
        )
        .route("/guardrail/observation/{action}", get(guardrail_observation_handler))
        .route("/guardrail/reset", axum::routing::post(guardrail_reset_handler))
        .route(
            "/ai-json-usage/release/{fixture_id}",
            axum::routing::post(ai_json_usage_release_handler),
        )
        .route("/ai-resilience/count/{trace_id}", get(ai_resilience_count_handler))
        .route("/ai-resilience/counts/{fixture_id}", get(ai_resilience_counts_handler))
        .route(
            "/ai-resilience/mode/{fixture_id}/{backend}/{mode}",
            axum::routing::post(ai_resilience_mode_handler),
        )
        .route(
            "/ai-resilience/release/{fixture_id}/half-open-primary",
            axum::routing::post(ai_resilience_release_handler),
        )
        .route(
            "/ai-credential-exhaustion/observations/{trace_id}",
            get(ai_credential_exhaustion_observations_handler),
        )
        .route(
            "/ai-long-stream/observation/{trace_id}",
            get(ai_long_stream_observation_handler),
        )
        // Native Anthropic Messages fixture. Its observation endpoint exposes
        // only a monotonically increasing count, never headers or bodies.
        .route("/anthropic/{*path}", axum::routing::post(anthropic_provider_handler))
        .route("/messages/{*path}", axum::routing::post(messages_provider_handler))
        .route("/anthropic-fixture/count", get(anthropic_request_count_handler))
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
        // --- Transport event recorder (for the raw transport-shape backends) ---
        // GET /transport/query/{trace_id}: the trace's own events, plus the
        // connection-scoped events of every connection that carried it, plus a
        // connections[] summary and the global conn_accepted_total.
        .route(
            "/transport/query/{trace_id}",
            get(transport::recorder::transport_query_handler),
        )
        // GET /transport/reset: clear the per-case transport log (call first in every case)
        .route("/transport/reset", get(transport::recorder::transport_reset_handler))
        // GET /transport/dump: everything the recorder holds, for failure dumps
        .route("/transport/dump", get(transport::recorder::transport_dump_handler))
        .route("/l4-duplex/{id}", get(l4_duplex::observe))
        .route("/l4-duplex/{id}/{action}", axum::routing::post(l4_duplex::command))
        // --- Retry-sequence endpoints (for retry-on-status integration tests) ---
        .route(
            "/retry-sequence/{test_id}/{status_list}",
            get(retry_sequence_handler).post(retry_sequence_handler),
        )
        .route("/retry-sequence-reset", get(retry_sequence_reset_handler))
        // --- Streaming-body endpoints (for per-try-timeout mid-body tests) ---
        // GET /stream-stall/{test_id}: 200 + one body chunk, then hang the body forever
        .route("/stream-stall/{test_id}", get(stream_stall_handler))
        // GET /stream-trickle/{test_id}/{chunks}/{interval_ms}: 200 + N chunks spaced apart, then finish
        .route(
            "/stream-trickle/{test_id}/{chunks}/{interval_ms}",
            get(stream_trickle_handler),
        )
        // --- Circuit-breaker endpoints (for circuit-breaker integration tests) ---
        .route("/cb-fail/{test_id}/{fail_count}", get(cb_fail_handler))
        .route("/cb-fail/{test_id}/{fail_count}/{status}", get(cb_fail_status_handler))
        .route("/cb-reset", get(cb_reset_handler))
        // --- Fake HTTP DNS endpoint (for httpdns resolver integration tests) ---
        // GET /dns/resolve?host=<domain>[&ip=<ip>[,<ip>...]]: returns the generic
        // httpdns JSON body {"ips":[...]} and counts one resolution for <host>.
        .route("/dns/resolve", get(dns_resolve_handler))
        // GET /dns/hits/{host}: how many times <host> has been resolved (caching assertions).
        .route("/dns/hits/{host}", get(dns_hits_handler))
        // GET /dns/reset: clear all HTTP DNS resolve counters (between test cases).
        .route("/dns/reset", get(dns_reset_handler))
        .route("/test/waf-dynamic", get(catch_all_handler).post(catch_all_handler))
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

/// Health endpoint for the EdgionBackendTrafficPolicy/UpstreamAuthority
/// integration scenario (Task 8: probe authority for health checks). Returns
/// 200 only when the received `Host` header falls inside the
/// `*.sandbox.internal` wildcard the scenario's `upstreamAuthority` policy
/// declares; any other Host — notably the raw socket-address Host a probe
/// sends when it has no `healthCheckHost` override — gets 421. This makes a
/// passing probe genuine evidence that the override reached this endpoint,
/// rather than a check that always passes regardless of Host.
async fn vhost_health_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::http::StatusCode;

    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if host.ends_with(".sandbox.internal") {
        (StatusCode::OK, "OK").into_response()
    } else {
        (
            StatusCode::MISDIRECTED_REQUEST,
            format!("unexpected Host for vhost-health: {host}"),
        )
            .into_response()
    }
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

async fn body_digest_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::body::to_bytes;
    use sha2::{Digest, Sha256};

    let content_length = req
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("absent")
        .to_string();
    let transfer_encoding = req
        .headers()
        .get("transfer-encoding")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("absent")
        .to_string();
    let body = to_bytes(req.into_body(), 32 * 1024 * 1024).await.unwrap_or_default();
    let digest = hex::encode(Sha256::digest(&body));

    (
        [
            ("x-body-sha256", digest),
            ("x-body-len", body.len().to_string()),
            ("x-content-length", content_length),
            ("x-transfer-encoding", transfer_encoding),
        ],
        "body-digest",
    )
}

async fn body_redirect_handler() -> impl IntoResponse {
    (
        axum::http::StatusCode::TEMPORARY_REDIRECT,
        [("location", "/body-digest")],
        "redirect-not-followed",
    )
}

async fn body_slow_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    body_digest_handler(req).await
}

async fn headers_handler(req: AxumRequest<Body>) -> axum::response::Response {
    use axum::http::{HeaderValue, StatusCode};
    use axum::response::Json;
    use serde_json::json;

    let headers = req.headers();
    if headers
        .get("x-ext-proc-backend-opaque")
        .and_then(|value| value.to_str().ok())
        == Some("1")
    {
        let values = |name: &str| {
            headers
                .get_all(name)
                .iter()
                .map(|value| value.as_bytes())
                .collect::<Vec<_>>()
        };
        let request_is_exact = values("x-opaque-presence") == [b"\xffrequest-first".as_slice(), b"\x80request-second"]
            && values("x-opaque-overwrite") == [b"\x81request-overwrite".as_slice()]
            && values("x-opaque-added") == [b"\x82request-add".as_slice(), b"\x83request-append"];
        if !request_is_exact {
            return (StatusCode::BAD_REQUEST, "opaque request mutation mismatch").into_response();
        }

        let mut response = (StatusCode::OK, "opaque header fixture").into_response();
        for (name, value) in [
            ("x-opaque-presence", b"\xfdresponse-first".as_slice()),
            ("x-opaque-presence", b"\x80response-second"),
            ("x-opaque-overwrite", b"\xfcresponse-old"),
        ] {
            response.headers_mut().append(
                name,
                HeaderValue::from_bytes(value).expect("fixed opaque header must be valid"),
            );
        }
        return response;
    }

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

    (StatusCode::OK, Json(response)).into_response()
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

fn oauth2_fixture_client(authorization: Option<&str>, form: &[u8]) -> Option<&'static str> {
    if form != b"grant_type=client_credentials&scope=inference.invoke" {
        return None;
    }
    let encoded = authorization?.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    let decoded = std::str::from_utf8(&decoded).ok()?;
    match decoded.split_once(':') {
        Some((OAUTH2_ACTIVE_CLIENT_ID, OAUTH2_ACTIVE_CLIENT_SECRET)) => Some("active"),
        Some((OAUTH2_PREVIOUS_CLIENT_ID, OAUTH2_PREVIOUS_CLIENT_SECRET)) => Some("previous"),
        _ => None,
    }
}

/// Deterministic OAuth2 Client Credentials issuer used by the Controller-side
/// credential lifecycle integration test. The five-second generation changes
/// independently of Kubernetes resources, allowing the test to prove that the
/// Controller performs expiry-aware refresh instead of merely reacting to
/// bootstrap Secret updates.
async fn oauth2_token_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;

    let authorization = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let Ok(form) = to_bytes(req.into_body(), 4 * 1024).await else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_request"})));
    };
    let Some(client) = oauth2_fixture_client(authorization.as_deref(), &form) else {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid_client"})));
    };
    let generation = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 5;
    (
        StatusCode::OK,
        Json(json!({
            "access_token": format!("{client}-oauth2-token-{generation}"),
            "token_type": "Bearer",
            "expires_in": 60
        })),
    )
}

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
    use sha2::{Digest, Sha256};

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

    // Read body (truncate at 4KB)
    let body_bytes = to_bytes(req.into_body(), 4096).await.unwrap_or_default();
    let body_sha256 = hex::encode(Sha256::digest(&body_bytes));
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
        info!("mirror/capture: stored request for trace_id={}", trace_id);
    }

    (
        StatusCode::OK,
        [("x-captured-body-sha256", body_sha256)],
        "mirror-captured",
    )
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
/// status_list format: "502,503,200" — returns 502 on 1st call, 503 on 2nd, 200 on 3rd+.
/// An entry may carry a delay: "200d3000" — sleep 3000ms then return 200
/// (delay capped at 10s); out-of-range calls reuse the last entry including its delay.
async fn retry_sequence_handler(
    Path((test_id, status_list)): Path<(String, String)>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    use axum::http::StatusCode;
    use sha2::{Digest, Sha256};

    if let Some(expected) = headers
        .get("x-expected-body-sha256")
        .and_then(|value| value.to_str().ok())
    {
        let actual = hex::encode(Sha256::digest(&body));
        if actual != expected {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                [
                    ("x-error".to_string(), "request body digest mismatch".to_string()),
                    ("x-attempt-number".to_string(), "0".to_string()),
                    ("x-attempt-history".to_string(), String::new()),
                    ("x-test-id".to_string(), test_id),
                ],
                format!("request body digest mismatch: expected={expected}, actual={actual}"),
            )
                .into_response();
        }
    }

    // Entry syntax: "STATUS" or "STATUSdDELAY_MS" (e.g. "200d3000" = sleep
    // 3000ms then return 200). Delay is capped at 10s for safety.
    let entries: Vec<(u16, u64)> = status_list
        .split(',')
        .filter_map(|s| {
            let s = s.trim();
            let (status_part, delay_part) = match s.split_once('d') {
                Some((st, d)) => (st, Some(d)),
                None => (s, None),
            };
            let status = status_part.parse::<u16>().ok()?;
            let delay_ms = match delay_part {
                Some(d) => d.parse::<u64>().ok()?.min(10_000),
                None => 0,
            };
            Some((status, delay_ms))
        })
        .collect();

    if entries.is_empty() {
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

    let (attempt, current_status, delay_ms) = {
        let mut store = RETRY_SEQUENCE_COUNTER.lock();
        let history = store.entry(test_id.clone()).or_default();
        let idx = history.len();
        // Use last entry for out-of-range attempts
        let (status, delay_ms) = entries.get(idx).copied().unwrap_or(*entries.last().unwrap());
        history.push(status);
        (idx + 1, status, delay_ms)
    };

    if delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }

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

/// /stream-stall/{test_id}
/// Sends 200 + response headers + ONE body chunk, then stalls the body stream
/// forever (the connection is held open with no further bytes). Models a backend
/// that commits a response then hangs mid-body.
///
/// Routed through a per-try-timeout rule, the gateway's `backendRequest` bound
/// fires while the body is in flight. Because response headers are already
/// committed downstream, the per-try `response_written` guard forbids a retry
/// (retrying would corrupt the half-sent response). The client therefore sees a
/// 200 with a truncated/errored body at ~backendRequest, and exactly one
/// upstream attempt is made.
async fn stream_stall_handler(Path(test_id): Path<String>) -> impl IntoResponse {
    use axum::body::{Body, Bytes};
    use axum::http::StatusCode;
    use axum::response::Response;

    // One channel item is emitted immediately (the first/only body chunk); the
    // sender is then parked in a detached task so the stream never terminates.
    // The 30s park is a safety cap — the gateway cuts the attempt long before.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    tokio::spawn(async move {
        let _ = tx.send(Ok(Bytes::from_static(b"stall-chunk-1;"))).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(tx);
    });

    let body = Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
    Response::builder()
        .status(StatusCode::OK)
        .header("x-test-id", test_id)
        .header("x-stream-mode", "stall")
        .body(body)
        .unwrap()
}

/// /stream-trickle/{test_id}/{chunks}/{interval_ms}
/// Sends 200 + `chunks` body chunks spaced `interval_ms` apart, then completes
/// the body normally. Models a backend that keeps making steady progress.
///
/// When `interval_ms` is below the route's `backendRequest` bound, no single
/// read stalls long enough to trip the per-try timeout, so the full body is
/// delivered even though the total transfer exceeds `backendRequest` — pinning
/// the per-I/O ("an attempt that keeps making progress is not killed")
/// semantics. `chunks` is clamped to [1, 50] and `interval_ms` to <= 5000.
async fn stream_trickle_handler(Path((test_id, chunks, interval_ms)): Path<(String, u32, u64)>) -> impl IntoResponse {
    use axum::body::{Body, Bytes};
    use axum::http::StatusCode;
    use axum::response::Response;

    let chunks = chunks.clamp(1, 50);
    let interval_ms = interval_ms.min(5_000);

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    tokio::spawn(async move {
        for i in 0..chunks {
            if interval_ms > 0 {
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            }
            if tx.send(Ok(Bytes::from(format!("trickle-chunk-{};", i)))).await.is_err() {
                break; // downstream went away
            }
        }
        // Dropping tx ends the stream → body completes normally.
    });

    let body = Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
    Response::builder()
        .status(StatusCode::OK)
        .header("x-test-id", test_id)
        .header("x-stream-mode", "trickle")
        .body(body)
        .unwrap()
}

#[derive(serde::Deserialize)]
struct SseQuery {
    events: Option<u32>,
    interval_ms: Option<u64>,
}

/// `/sse?events=N&interval_ms=T`
///
/// Sends one SSE event immediately, then sends the remaining events at the
/// requested interval. Each channel item is yielded independently to Hyper,
/// which makes the event observable downstream without waiting for the stream
/// to finish. Bounds keep malformed test input from creating a long-lived or
/// unbounded test-server task.
async fn sse_handler(Query(query): Query<SseQuery>) -> impl IntoResponse {
    use axum::body::{Body, Bytes};
    use axum::http::StatusCode;
    use axum::response::Response;

    let events = query.events.unwrap_or(5).clamp(1, 50);
    let interval_ms = query.interval_ms.unwrap_or(200).min(5_000);

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    tokio::spawn(async move {
        for seq in 0..events {
            let ts_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let event = format!("data: {{\"seq\":{seq},\"ts_ms\":{ts_ms}}}\n\n");
            if tx.send(Ok(Bytes::from(event))).await.is_err() {
                break;
            }
            if seq + 1 < events && interval_ms > 0 {
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            }
        }
    });

    let body = Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(body)
        .unwrap()
}

const AI_PROVIDER_KEY_A: &str = "ai-provider-test-key";
const AI_PROVIDER_KEY_B: &str = "ai-provider-rotated-test-key";
const AI_PROVIDER_EXPECTED_HOSTS: &[&str] = &[
    "127.0.0.1:30001",
    "127.0.0.1:30026",
    "127.0.0.1:30051",
    "127.0.0.1:30052",
    "edgion-test-server.edgion-test.svc.cluster.local:30001",
    "edgion-test-server.edgion-test.svc.cluster.local:30026",
    "edgion-test-server.edgion-test.svc.cluster.local:30051",
    "edgion-test-server.edgion-test.svc.cluster.local:30052",
];

#[derive(Debug, Default, serde::Deserialize)]
struct AiProviderQuery {
    mock_mode: Option<String>,
    fixture_query: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct AiProviderObservation {
    fixture_id: &'static str,
    auth_is_provider_key: bool,
    credential_generation: &'static str,
    auth_is_client_key: bool,
    host_is_expected: bool,
    path_has_prefix: bool,
    query_is_preserved: bool,
    model_is_canonical: bool,
    model_is_economy: bool,
    model_is_unknown: bool,
    stream_requested: bool,
    include_usage: bool,
}

fn ai_provider_observation(
    path: &str,
    query: &AiProviderQuery,
    headers: &axum::http::HeaderMap,
    body: &[u8],
) -> AiProviderObservation {
    let json: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let model = json.get("model").and_then(serde_json::Value::as_str);
    let credential_generation = if auth == format!("Bearer {AI_PROVIDER_KEY_A}") {
        "a"
    } else if auth == format!("Bearer {AI_PROVIDER_KEY_B}") {
        "b"
    } else {
        "unknown"
    };
    AiProviderObservation {
        fixture_id: "openai-compatible-v1",
        auth_is_provider_key: credential_generation != "unknown",
        credential_generation,
        auth_is_client_key: auth == "Bearer client-key-must-be-stripped",
        host_is_expected: headers
            .get(axum::http::header::HOST)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|host| AI_PROVIDER_EXPECTED_HOSTS.contains(&host)),
        path_has_prefix: path == "prefix/v1/chat/completions" || path == "responses/v1/responses",
        query_is_preserved: query.fixture_query.as_deref() == Some("preserved"),
        model_is_canonical: model == Some("fixture-model"),
        model_is_economy: model == Some("economy-model"),
        model_is_unknown: model == Some("unknown-fixture-model"),
        stream_requested: json.get("stream").and_then(serde_json::Value::as_bool) == Some(true),
        include_usage: json
            .pointer("/stream_options/include_usage")
            .and_then(serde_json::Value::as_bool)
            == Some(true),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AiStreamPlan {
    chunks: Vec<Bytes>,
    interval: Duration,
    abort_after_chunks: bool,
}

/// Payload size for the `oversized` fixture modes, chosen so the surrounding
/// event exceeds the gateway decoder's incomplete-event cap by a clear margin.
/// Bound to the gateway constant on purpose: these fixtures exist to cross that
/// limit, and a hardcoded literal would quietly stop doing so the next time the
/// cap is raised.
fn oversized_event_payload_bytes() -> usize {
    edgion_gateway::sse::DEFAULT_MAX_PENDING_EVENT_BYTES + 4096
}

fn ai_stream_plan(mode: &str, observation: &str) -> AiStreamPlan {
    let first = format!(
        "data: {{\"id\":\"stream-fixture\",\"choices\":[{{\"delta\":{{\"content\":\"fixed \"}}}}],\"mock\":{observation}}}\n\n"
    );
    let middle = "data: {\"id\":\"stream-fixture\",\"choices\":[{\"delta\":{\"content\":\"completion\"}}]}\n\n";
    let usage = "data: {\"id\":\"stream-fixture\",\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7,\"total_tokens\":18}}\n\n";
    let done = "data: [DONE]\n\n";

    match mode {
        "guardrail_split" => {
            let first = format!(
                "data: {{\"id\":\"stream-fixture\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"fixed \"}}}}],\"mock\":{observation}}}\n\n"
            );
            AiStreamPlan {
                chunks: format!("{first}{usage}{done}")
                    .as_bytes()
                    .chunks(7)
                    .map(Bytes::copy_from_slice)
                    .collect(),
                interval: Duration::ZERO,
                abort_after_chunks: false,
            }
        }
        "guardrail_two_windows" => {
            let first = format!(
                "data: {{\"id\":\"stream-fixture\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"first\"}}}}],\"mock\":{observation}}}\n\n"
            );
            let second = "data: {\"id\":\"stream-fixture\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"second\"}}]}\n\n";
            AiStreamPlan {
                chunks: vec![
                    Bytes::from(first),
                    Bytes::from_static(second.as_bytes()),
                    Bytes::from_static(usage.as_bytes()),
                    Bytes::from_static(done.as_bytes()),
                ],
                interval: Duration::ZERO,
                abort_after_chunks: false,
            }
        }
        "split" => AiStreamPlan {
            chunks: format!("{first}{usage}{done}")
                .as_bytes()
                .chunks(7)
                .map(Bytes::copy_from_slice)
                .collect(),
            interval: Duration::ZERO,
            abort_after_chunks: false,
        },
        "missing_usage" => AiStreamPlan {
            chunks: vec![Bytes::from(first), Bytes::from_static(done.as_bytes())],
            interval: Duration::ZERO,
            abort_after_chunks: false,
        },
        // One event remains incomplete until the delimiter well past the
        // decoder's bounded inspection limit, deliberately crossing it. Sized
        // from the constant rather than a literal so raising the cap cannot
        // silently turn this fixture into an ordinary large event.
        "oversized" => AiStreamPlan {
            chunks: vec![
                Bytes::from(format!(
                    "data: {{\"id\":\"oversized-fixture\",\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
                    "x".repeat(oversized_event_payload_bytes())
                )),
                Bytes::from_static(usage.as_bytes()),
                Bytes::from_static(done.as_bytes()),
            ],
            interval: Duration::ZERO,
            abort_after_chunks: false,
        },
        "resilience_abort" => AiStreamPlan {
            chunks: vec![
                Bytes::from(first),
                Bytes::from_static(middle.as_bytes()),
                Bytes::from_static(
                    b"data: {\"id\":\"stream-fixture\",\"choices\":[{\"delta\":{\"content\":\" before-abort\"}}]}\n\n",
                ),
            ],
            interval: Duration::from_millis(50),
            abort_after_chunks: true,
        },
        // Two completed poisoned events with one valid event between them.
        // Invalid bytes are built directly so no UTF-8 text API can replace
        // or reject them. The one-second cadence makes the client observe the
        // valid middle event before the cumulative threshold terminates, while
        // leaving a deterministic tail that would complete naturally later.
        "quota_malformed" => AiStreamPlan {
            chunks: vec![
                Bytes::from_static(b"data: \xff\n\n"),
                Bytes::from_static(
                    b"data: {\"id\":\"quota-malformed-fixture\",\"choices\":[{\"delta\":{\"content\":\"valid-middle\"}}]}\n\n",
                ),
                // Leave the boundary probe incomplete in the same transport
                // chunk as the second completed poisoned event. The gateway's
                // injected error must supply its own leading blank-line
                // boundary so this fragment cannot swallow the error event.
                Bytes::from_static(b"data: \xfe\n\ndata: quota-malformed-boundary-probe"),
                Bytes::from_static(
                    b"data: {\"unreachable_marker\":\"quota-malformed-must-not-reach-client\"}\n\ndata: [DONE]\n\n",
                ),
            ],
            interval: Duration::from_secs(1),
            abort_after_chunks: false,
        },
        // Five real content deltas (one quota output unit each), spaced far
        // apart (150ms) relative to the 10ms quota flush interval the
        // `ai-proxy-quota-terminate` fixture plugin uses, so a real
        // Redis-backed over-limit flip is always observed with a wide safety
        // margin before the fixture's tail. The tail (usage + [DONE]) carries
        // an `unreachable_marker` field so a test can assert its exact
        // absence without needing to pin exactly which content chunk the
        // gateway terminates on.
        "quota_terminate" => {
            let delta = |label: &str| {
                Bytes::from(format!(
                    "data: {{\"id\":\"quota-terminate-fixture\",\"choices\":[{{\"delta\":{{\"content\":\"{label}\"}}}}]}}\n\n"
                ))
            };
            AiStreamPlan {
                chunks: vec![
                    delta("chunk-a"),
                    delta("chunk-b"),
                    delta("chunk-c"),
                    delta("chunk-d"),
                    delta("chunk-e"),
                    Bytes::from_static(
                        b"data: {\"id\":\"quota-terminate-fixture\",\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7,\"total_tokens\":18},\"unreachable_marker\":\"quota-terminate-must-not-reach-client\"}\n\n",
                    ),
                    Bytes::from_static(b"data: [DONE]\n\n"),
                ],
                interval: Duration::from_millis(150),
                abort_after_chunks: false,
            }
        }
        // The same `quota_terminate` wire bytes, delivered one byte per write.
        // A provider is free to segment wherever it likes, so the gateway's
        // injected error event lands at an arbitrary offset -- one byte per
        // write is simply the worst case stated explicitly, and it guarantees
        // the mid-stream cut falls inside an SSE event rather than on a
        // boundary. It also maximizes filter invocations and per-batch
        // boundaries, which stresses the response-body sink's per-batch
        // lifecycle far harder than any whole-event fixture.
        //
        // 2ms per byte keeps one ~90-byte event around 180ms, still an order
        // of magnitude above the fixture plugin's 10ms quota flush interval,
        // so the over-limit flip is observed with the same wide margin the
        // paced fixture relies on. Unlike `quota_terminate` this mode declares
        // no `content-length` (see `full_length` below): stale-length
        // stripping is already covered there, and leaving it out keeps this
        // fixture focused on the byte-boundary property.
        "quota_terminate_byte_at_a_time" => {
            let paced = ai_stream_plan("quota_terminate", observation);
            AiStreamPlan {
                chunks: paced
                    .chunks
                    .into_iter()
                    .flat_map(|chunk| (0..chunk.len()).map(move |index| chunk.slice(index..index + 1)))
                    .collect(),
                interval: Duration::from_millis(2),
                abort_after_chunks: false,
            }
        }
        // Long enough for Task 9 to drop the downstream client before the
        // provider reaches its final usage event.
        "long" => {
            let mut chunks = vec![Bytes::from(first)];
            chunks.extend((0..100).map(|_| {
                Bytes::from_static(
                    b"data: {\"id\":\"long-fixture\",\"choices\":[{\"delta\":{\"content\":\"tick\"}}]}\n\n",
                )
            }));
            chunks.push(Bytes::from_static(usage.as_bytes()));
            chunks.push(Bytes::from_static(done.as_bytes()));
            AiStreamPlan {
                chunks,
                interval: Duration::from_millis(100),
                abort_after_chunks: false,
            }
        }
        _ => AiStreamPlan {
            chunks: vec![
                Bytes::from(first),
                Bytes::from_static(middle.as_bytes()),
                Bytes::from_static(usage.as_bytes()),
                Bytes::from_static(done.as_bytes()),
            ],
            interval: Duration::from_millis(250),
            abort_after_chunks: false,
        },
    }
}

async fn guardrail_decision_handler(
    Path(action): Path<String>,
    request: AxumRequest<Body>,
) -> axum::response::Response {
    use axum::body::to_bytes;
    use axum::http::{header::CONTENT_TYPE, StatusCode};
    use axum::response::Response;

    let Ok(body) = to_bytes(request.into_body(), 1024 * 1024).await else {
        return Response::builder()
            .status(StatusCode::PAYLOAD_TOO_LARGE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"error":"request_too_large"}"#))
            .unwrap();
    };
    let input: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    record_guardrail_observation(&action, &input);

    let response = match action.as_str() {
        "pass" => serde_json::json!({"version": "v1", "action": "pass"}).to_string(),
        "reject" => serde_json::json!({
            "version": "v1",
            "action": "reject",
            "code": "fixture_policy",
            "reason": "Rejected by the integration fixture"
        })
        .to_string(),
        "replace" => {
            let replacements = input
                .get("windows")
                .and_then(serde_json::Value::as_array)
                .map(|windows| {
                    windows
                        .iter()
                        .map(|window| {
                            serde_json::json!({
                                "choiceIndex": window.get("choiceIndex").cloned().unwrap_or_default(),
                                "field": window.get("field").cloned().unwrap_or_default(),
                                "current": "reviewed"
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            serde_json::json!({
                "version": "v1",
                "action": "replace",
                "replacements": replacements
            })
            .to_string()
        }
        // Deliberately valid UTF-8 but invalid JSON. The Guardrail fail-open
        // route must release the held original window byte-identically.
        "malformed" => "{not-json".to_string(),
        "timeout" => {
            tokio::time::sleep(Duration::from_millis(500)).await;
            serde_json::json!({"version": "v1", "action": "pass"}).to_string()
        }
        _ => serde_json::json!({"error": "unknown_fixture_action"}).to_string(),
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(response))
        .unwrap()
}

fn record_guardrail_observation(action: &str, input: &serde_json::Value) {
    let object = input.as_object();
    let windows = input
        .get("windows")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut keys = windows
        .iter()
        .map(|window| {
            let choice = window.get("choiceIndex").and_then(serde_json::Value::as_u64);
            let field = window.pointer("/field/type").and_then(serde_json::Value::as_str);
            let tool = window
                .pointer("/field/toolCallIndex")
                .and_then(serde_json::Value::as_u64);
            (choice, field.map(str::to_owned), tool)
        })
        .collect::<Vec<_>>();
    let original_keys = keys.clone();
    keys.sort();
    let sorted_unique_keys = original_keys == keys && keys.windows(2).all(|pair| pair[0] != pair[1]);
    let canonical_content_window = windows.len() == 1
        && windows[0].get("choiceIndex").and_then(serde_json::Value::as_u64) == Some(0)
        && windows[0].pointer("/field/type").and_then(serde_json::Value::as_str) == Some("content")
        && windows[0].get("context").is_some_and(serde_json::Value::is_string)
        && windows[0].get("current").is_some_and(serde_json::Value::is_string)
        && windows[0].as_object().is_some_and(|window| window.len() == 4);

    let mut observations = GUARDRAIL_OBSERVATIONS.lock();
    let observation = observations.entry(action.to_string()).or_default();
    observation.calls = observation.calls.saturating_add(1);
    observation.total_windows = observation.total_windows.saturating_add(windows.len() as u64);
    observation.last_window_count = windows.len();
    observation.version_v1 = input.get("version").and_then(serde_json::Value::as_str) == Some("v1");
    observation.minimal_envelope = object
        .is_some_and(|object| object.len() == 2 && object.contains_key("version") && object.contains_key("windows"));
    observation.sorted_unique_keys = sorted_unique_keys;
    observation.canonical_content_window = canonical_content_window;
}

async fn guardrail_observation_handler(Path(action): Path<String>) -> impl IntoResponse {
    let observation = GUARDRAIL_OBSERVATIONS.lock().get(&action).cloned().unwrap_or_default();
    axum::Json(observation)
}

async fn guardrail_reset_handler() -> axum::http::StatusCode {
    GUARDRAIL_OBSERVATIONS.lock().clear();
    axum::http::StatusCode::NO_CONTENT
}

async fn ai_json_usage_release_handler(Path(fixture_id): Path<String>) -> axum::http::StatusCode {
    let release = AI_JSON_USAGE_RELEASES.lock().get(&fixture_id).cloned();
    match release {
        Some(release) => {
            release.notify_one();
            axum::http::StatusCode::NO_CONTENT
        }
        None => axum::http::StatusCode::NOT_FOUND,
    }
}

/// A bounded producer for ordinary JSON accounting. The eight MiB field is
/// emitted as shared 32 KiB slices, never assembled as a complete response.
fn incremental_json_usage_fixture(mode: Option<&str>, fixture_id: Option<&str>) -> Option<axum::response::Response> {
    let mode = mode?;
    if mode == "json_encoded" {
        // Deterministic gzip of the small usage document; these are real encoded
        // bytes so passive observation must preserve them without parsing.
        const GZIP: &[u8] = &[
            31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 171, 86, 42, 45, 78, 76, 79, 85, 178, 170, 86, 42, 40, 202, 207, 45, 40,
            137, 47, 201, 207, 78, 205, 43, 86, 178, 50, 210, 81, 74, 6, 10, 228, 164, 150, 100, 230, 231, 193, 69,
            141, 107, 107, 1, 168, 37, 36, 72, 51, 0, 0, 0,
        ];
        return Some(
            axum::response::Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .header("content-encoding", "identity")
                .header("content-encoding", "gzip")
                .body(Body::from(GZIP))
                .unwrap(),
        );
    }
    let (prefix, suffix): (&'static [u8], &'static [u8]) = match mode {
        "json_large_usage_before" => (
            br#"{"usage":{"prompt_tokens":2,"completion_tokens":3},"ignored":""#,
            br#""}"#,
        ),
        "json_large_usage_after" => (
            br#"{"ignored":""#,
            br#"","usage":{"prompt_tokens":2,"completion_tokens":3}}"#,
        ),
        "json_large_missing_usage" => (br#"{"ignored":""#, br#""}"#),
        "json_malformed_tail" => (
            br#"{"usage":{"prompt_tokens":2,"completion_tokens":3},"ignored":""#,
            br#""}garbage"#,
        ),
        "json_duplicate_parent" => (
            br#"{"usage":{"prompt_tokens":2,"completion_tokens":3},"ignored":""#,
            br#"","usage":{"prompt_tokens":4,"completion_tokens":5}}"#,
        ),
        _ => return None,
    };
    let fixture_id = fixture_id.filter(|id| !id.is_empty() && id.len() <= 80)?.to_string();
    let release = Arc::new(tokio::sync::Notify::new());
    {
        let mut pending = AI_JSON_USAGE_RELEASES.lock();
        if pending.len() >= 16 || pending.contains_key(&fixture_id) {
            return None;
        }
        pending.insert(fixture_id.clone(), release.clone());
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    tokio::spawn(async move {
        let sent_prefix = tx.send(Ok(Bytes::from_static(prefix))).await.is_ok();
        let released = sent_prefix
            && tokio::time::timeout(Duration::from_secs(5), release.notified())
                .await
                .is_ok();
        AI_JSON_USAGE_RELEASES.lock().remove(&fixture_id);
        if !released {
            let _ = tx
                .send(Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "JSON fixture prefix was not released",
                )))
                .await;
            return;
        }
        let payload = Bytes::from(vec![b'x'; 32 * 1024]);
        for _ in 0..256 {
            if tx.send(Ok(payload.clone())).await.is_err() {
                return;
            }
        }
        let _ = tx.send(Ok(Bytes::from_static(suffix))).await;
    });
    Some(
        axum::response::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)))
            .unwrap(),
    )
}

/// Native Responses fixture uses the same bounded channel and observation owner as Chat.
fn responses_provider_response(
    body: &[u8],
    mode: &str,
    observation: &AiProviderObservation,
    headers: &axum::http::HeaderMap,
) -> axum::response::Response {
    use serde_json::json;
    let request: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let stream = request["stream"] == true;
    let status = match mode {
        "incomplete" => "incomplete",
        "failed" => "failed",
        "cancelled" => "cancelled",
        _ => "completed",
    };
    let text = if mode == "large" {
        "x".repeat(256 * 1024)
    } else {
        let mut native_observation = serde_json::to_value(observation).unwrap();
        native_observation["principal_selectors_absent"] =
            json!(!headers.contains_key("openai-organization") && !headers.contains_key("openai-project"));
        native_observation.to_string()
    };
    let mut response = json!({"id":"resp_fixture", "object":"response", "created_at":1, "status":status,
        "error":null,"incomplete_details":null,"model":"fixture-model","output":[{"id":"msg_fixture","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]}],
        "parallel_tool_calls":true,"tool_choice":"auto","tools":[],"temperature":1.0,"top_p":1.0,
        "usage":{"input_tokens":11,"output_tokens":7,"total_tokens":18,"input_tokens_details":{"cached_tokens":3},"output_tokens_details":{"reasoning_tokens":4}},
        "native_extension":{"preserved":true,"request":request}});
    if mode == "missing_usage" {
        response.as_object_mut().unwrap().remove("usage");
    }
    if !stream {
        let mut wire = response.to_string();
        if mode == "malformed" {
            wire.push_str("invalid-tail");
        }
        return axum::response::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from(wire))
            .unwrap();
    }
    let event = |value: serde_json::Value| {
        Bytes::from(format!(
            "event: {}\ndata: {}\n\n",
            value["type"].as_str().unwrap(),
            value
        ))
    };
    let mut frames = vec![
        event(
            json!({"type":"response.created","sequence_number":0,"response":{"id":"resp_fixture","status":"in_progress"}}),
        ),
        event(
            json!({"type":"response.output_text.delta","sequence_number":1,"item_id":"msg_fixture","output_index":0,"content_index":0,"delta":"hello"}),
        ),
        event(
            json!({"type":"response.output_text.done","sequence_number":2,"item_id":"msg_fixture","output_index":0,"content_index":0,"text":"hello"}),
        ),
    ];
    if mode == "long" {
        frames.extend((3..1000).map(|seq| event(json!({"type":"response.output_text.delta","sequence_number":seq,"item_id":"msg_fixture","output_index":0,"content_index":0,"delta":"fixed streamed text"}))));
    }
    let terminal = match status {
        "failed" => "response.failed",
        "incomplete" => "response.incomplete",
        _ => "response.completed",
    };
    frames.push(event(
        json!({"type":terminal,"sequence_number":1001,"response":response}),
    ));
    if mode == "malformed" {
        frames.push(Bytes::from_static(b"data: {"));
    }
    if mode == "oversized" {
        frames.insert(
            1,
            event(json!({"type":"response.output_text.delta","sequence_number":1,"delta":"x".repeat(1024 * 1024)})),
        );
    }
    if mode == "abort" {
        frames.truncate(2);
    }
    let chunks = if mode == "split" {
        frames
            .into_iter()
            .flat_map(|frame| frame.chunks(7).map(Bytes::copy_from_slice).collect::<Vec<_>>())
            .collect()
    } else {
        frames
    };
    let trace = if mode == "long" {
        headers
            .get("x-trace-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    } else {
        None
    };
    if let Some(trace) = &trace {
        AI_LONG_STREAM_OBSERVATIONS
            .lock()
            .insert(trace.clone(), AiLongStreamObservation::default());
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    let abort = mode == "abort";
    tokio::spawn(async move {
        for chunk in chunks {
            if tx.send(Ok(chunk)).await.is_err() {
                if let Some(trace) = &trace {
                    if let Some(state) = AI_LONG_STREAM_OBSERVATIONS.lock().get_mut(trace) {
                        state.upstream_cancelled = true;
                    }
                }
                return;
            }
            if let Some(trace) = &trace {
                if let Some(state) = AI_LONG_STREAM_OBSERVATIONS.lock().get_mut(trace) {
                    state.chunks_queued += 1;
                }
            }
            if trace.is_some() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        if abort {
            let _ = tx
                .send(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "intentional Responses fixture abort",
                )))
                .await;
        }
        if let Some(trace) = &trace {
            if let Some(state) = AI_LONG_STREAM_OBSERVATIONS.lock().get_mut(trace) {
                state.provider_completed = true;
            }
        }
    });
    axum::response::Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)))
        .unwrap()
}

async fn ai_provider_handler(
    Path(path): Path<String>,
    Query(query): Query<AiProviderQuery>,
    request: AxumRequest<Body>,
) -> axum::response::Response {
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::Response;

    let (parts, body) = request.into_parts();
    let Ok(body) = to_bytes(body, 1024 * 1024).await else {
        return Response::builder()
            .status(StatusCode::PAYLOAD_TOO_LARGE)
            .body(Body::empty())
            .unwrap();
    };
    let observation = ai_provider_observation(&path, &query, &parts.headers, &body);
    if path == "responses/v1/responses" {
        return responses_provider_response(
            &body,
            query.mock_mode.as_deref().unwrap_or("completed"),
            &observation,
            &parts.headers,
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let trace_id = parts
        .headers
        .get("x-trace-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    // `quota_terminate_force_sse` deliberately ignores the client's declared
    // `stream` field: it simulates a provider that answers with an SSE body
    // even though the caller asked for a plain JSON completion, which is the
    // only way to observe the `requested_streaming` gate from outside the
    // process (Edgion's own `requested_streaming` comes from the client
    // request, not from the wire shape of the provider's answer).
    let force_sse = query.mock_mode.as_deref() == Some("quota_terminate_force_sse");
    let streaming = force_sse || json.get("stream").and_then(serde_json::Value::as_bool) == Some(true);

    if let Some(trace_id) = trace_id.as_deref() {
        let exhaustion_backend = if path.starts_with("credential-exhaustion-primary/") {
            Some("primary")
        } else if path.starts_with("credential-exhaustion-secondary/") {
            Some("secondary")
        } else {
            None
        };
        if let Some(backend) = exhaustion_backend {
            AI_CREDENTIAL_EXHAUSTION_OBSERVATIONS
                .lock()
                .entry(trace_id.to_string())
                .or_default()
                .push(AiCredentialExhaustionObservation {
                    backend,
                    credential_generation: observation.credential_generation,
                });
        }
        let slot = if path.starts_with("resilience-429/") {
            Some(0)
        } else if path.starts_with("resilience-stream/") {
            Some(2)
        } else if path.starts_with("resilience-stream-fallback/") {
            Some(3)
        } else if path.starts_with("resilience-half-open-primary/") {
            Some(4)
        } else if path.starts_with("resilience-half-open-fallback/") {
            Some(5)
        } else {
            None
        };
        if let Some(slot) = slot {
            let fixture_id = parts
                .headers
                .get("x-resilience-fixture-id")
                .and_then(|value| value.to_str().ok())
                .unwrap_or(trace_id);
            AI_RESILIENCE_STATE
                .lock()
                .entry(fixture_id.to_string())
                .or_default()
                .counts[slot] += 1;
        }
    }

    if path.starts_with("resilience-429/") {
        let mut response = Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header("content-type", "application/json")
            .header("x-ratelimit-reset-requests", "1500ms");
        if path.ends_with("/v1/messages") {
            response = response.header("retry-after", "2");
        }
        return response
            .body(Body::from(
                r#"{"error":{"message":"private resilience fixture detail"}}"#,
            ))
            .unwrap();
    }

    if path.starts_with("resilience-half-open-primary/") {
        let fixture_id = parts
            .headers
            .get("x-resilience-fixture-id")
            .and_then(|value| value.to_str().ok())
            .or_else(|| parts.headers.get("x-trace-id").and_then(|value| value.to_str().ok()));
        let state = fixture_id
            .and_then(|fixture_id| {
                AI_RESILIENCE_STATE.lock().get(fixture_id).map(|state| {
                    (
                        state.half_open_primary_mode,
                        std::sync::Arc::clone(&state.half_open_probe_release),
                    )
                })
            })
            .unwrap_or_else(|| {
                (
                    AiResiliencePrimaryMode::default(),
                    std::sync::Arc::new(tokio::sync::Notify::new()),
                )
            });
        let (mode, probe_release) = state;
        match mode {
            AiResiliencePrimaryMode::Fail => {
                return Response::builder()
                    .status(StatusCode::SERVICE_UNAVAILABLE)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"error":{"message":"private half-open fixture detail"}}"#,
                    ))
                    .unwrap();
            }
            AiResiliencePrimaryMode::ProbeSuccess | AiResiliencePrimaryMode::ProbeFail => {
                let gate_timeout = if matches!(mode, AiResiliencePrimaryMode::ProbeFail) {
                    Duration::from_secs(120)
                } else {
                    Duration::from_secs(10)
                };
                let _ = tokio::time::timeout(gate_timeout, probe_release.notified()).await;
                if matches!(mode, AiResiliencePrimaryMode::ProbeFail) {
                    return Response::builder()
                        .status(StatusCode::SERVICE_UNAVAILABLE)
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"error":{"message":"fixed gated retry failure"}}"#))
                        .unwrap();
                }
            }
            AiResiliencePrimaryMode::Healthy => {}
        }
    }

    if path.starts_with("fail-503/") {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"error":{"message":"fixed retryable provider failure"}}"#,
            ))
            .unwrap();
    }

    if path.starts_with("credential-exhaustion-") {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"error":{"message":"fixed retryable provider failure"}}"#,
            ))
            .unwrap();
    }

    if let Some(response) = incremental_json_usage_fixture(query.mock_mode.as_deref(), query.fixture_query.as_deref()) {
        return response;
    }

    if !streaming {
        let mut response = serde_json::json!({
            "id": "completion-fixture",
            "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "fixed completion"}}],
            "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18},
            "mock": observation,
        });
        if query.mock_mode.as_deref() == Some("header_modifier") {
            // Observe only the synthetic fields owned by the modifier regression.
            let values = |name| {
                parts
                    .headers
                    .get_all(name)
                    .iter()
                    .map(|value| value.to_str().unwrap_or_default())
                    .collect::<Vec<_>>()
            };
            response["mock"]["header_modifier"] = serde_json::json!({
                "set": values("x-header-modifier-set"),
                "add": values("x-header-modifier-add"),
                "remove": values("x-header-modifier-remove"),
            });
        }
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(response.to_string()))
            .unwrap();
    }

    let observation = serde_json::to_string(&observation).unwrap();
    let mock_mode = query.mock_mode.as_deref();
    // Repeated identity then gzip is the D0105 wire shape: the first field is
    // valid, but the field set is not identity-coded. The body stays the
    // uncompressed Guardrail SSE fixture so fail-open can prove passthrough.
    let mixed_identity_gzip = mock_mode == Some("guardrail_encoded");
    let mode = if path.starts_with("resilience-stream/") {
        "resilience_abort"
    } else {
        match mock_mode.unwrap_or("paced") {
            // The force-sse marker only changes admission (`streaming` above);
            // the wire fixture itself is the same `quota_terminate` plan a
            // normal streaming request would get.
            "quota_terminate_force_sse" => "quota_terminate",
            "guardrail_encoded" => "guardrail_split",
            other => other,
        }
    };
    let long_trace_id = (mode == "long").then_some(trace_id).flatten();
    if let Some(trace_id) = long_trace_id.as_ref() {
        AI_LONG_STREAM_OBSERVATIONS
            .lock()
            .insert(trace_id.clone(), AiLongStreamObservation::default());
    }
    let plan = ai_stream_plan(mode, &observation);
    // Simulates a provider that pre-declares `Content-Length` for its full,
    // untruncated SSE body (unusual, but legal) so the fixture can prove
    // Edgion actually strips a stale length rather than merely never having
    // one to strip in the first place — every other streaming fixture here
    // is naturally unbounded and would pass a framing check trivially.
    let full_length = (mode == "quota_terminate").then(|| plan.chunks.iter().map(Bytes::len).sum::<usize>());
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    tokio::spawn(async move {
        for chunk in plan.chunks {
            if tx.send(Ok(chunk)).await.is_err() {
                if let Some(trace_id) = long_trace_id.as_ref() {
                    if let Some(state) = AI_LONG_STREAM_OBSERVATIONS.lock().get_mut(trace_id) {
                        state.upstream_cancelled = true;
                    }
                }
                return;
            }
            if let Some(trace_id) = long_trace_id.as_ref() {
                if let Some(state) = AI_LONG_STREAM_OBSERVATIONS.lock().get_mut(trace_id) {
                    state.chunks_queued = state.chunks_queued.saturating_add(1);
                }
            }
            if !plan.interval.is_zero() {
                tokio::time::sleep(plan.interval).await;
            }
        }
        if plan.abort_after_chunks {
            let _ = tx
                .send(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "intentional AI provider fixture abort",
                )))
                .await;
        }
        if let Some(trace_id) = long_trace_id.as_ref() {
            if let Some(state) = AI_LONG_STREAM_OBSERVATIONS.lock().get_mut(trace_id) {
                state.provider_completed = true;
            }
        }
    });

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache");
    if let Some(full_length) = full_length {
        builder = builder.header("content-length", full_length.to_string());
    }
    if mixed_identity_gzip {
        builder = builder
            .header("content-encoding", "identity")
            .header("content-encoding", "gzip");
    }
    builder
        .body(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)))
        .unwrap()
}

async fn ai_long_stream_observation_handler(Path(trace_id): Path<String>) -> impl IntoResponse {
    let observation = {
        let mut observations = AI_LONG_STREAM_OBSERVATIONS.lock();
        let observation = observations.get(&trace_id).cloned();
        if observation
            .as_ref()
            .is_some_and(|state| state.upstream_cancelled || state.provider_completed)
        {
            observations.remove(&trace_id);
        }
        observation
    };
    axum::Json(serde_json::json!({
        "found": observation.is_some(),
        "observation": observation,
    }))
}

async fn ai_resilience_count_handler(Path(trace_id): Path<String>) -> impl IntoResponse {
    let counts = AI_RESILIENCE_STATE
        .lock()
        .remove(&trace_id)
        .map(|state| state.counts)
        .unwrap_or_default();
    axum::Json(ai_resilience_counts_json(counts))
}

async fn ai_resilience_counts_handler(Path(fixture_id): Path<String>) -> impl IntoResponse {
    let counts = AI_RESILIENCE_STATE
        .lock()
        .get(&fixture_id)
        .map(|state| state.counts)
        .unwrap_or_default();
    axum::Json(ai_resilience_counts_json(counts))
}

fn ai_resilience_counts_json(counts: [u64; 6]) -> serde_json::Value {
    serde_json::json!({
        "rate_limited": counts[0],
        "healthy": counts[1],
        "stream_primary": counts[2],
        "stream_fallback": counts[3],
        "half_open_primary": counts[4],
        "half_open_fallback": counts[5],
    })
}

async fn ai_resilience_mode_handler(
    Path((fixture_id, backend, mode)): Path<(String, String, String)>,
) -> axum::http::StatusCode {
    if fixture_id.len() > 128 || backend != "half-open-primary" {
        return axum::http::StatusCode::BAD_REQUEST;
    }
    let mode = match mode.as_str() {
        "healthy" => AiResiliencePrimaryMode::Healthy,
        "fail" => AiResiliencePrimaryMode::Fail,
        "probe-success" => AiResiliencePrimaryMode::ProbeSuccess,
        "probe-fail" => AiResiliencePrimaryMode::ProbeFail,
        _ => return axum::http::StatusCode::BAD_REQUEST,
    };
    let mut fixtures = AI_RESILIENCE_STATE.lock();
    let state = fixtures.entry(fixture_id).or_default();
    state.half_open_primary_mode = mode;
    if matches!(
        mode,
        AiResiliencePrimaryMode::ProbeSuccess | AiResiliencePrimaryMode::ProbeFail
    ) {
        state.half_open_probe_release = std::sync::Arc::new(tokio::sync::Notify::new());
    }
    axum::http::StatusCode::NO_CONTENT
}

async fn ai_resilience_release_handler(Path(fixture_id): Path<String>) -> axum::http::StatusCode {
    let Some(release) = AI_RESILIENCE_STATE
        .lock()
        .get(&fixture_id)
        .map(|state| std::sync::Arc::clone(&state.half_open_probe_release))
    else {
        return axum::http::StatusCode::NOT_FOUND;
    };
    release.notify_one();
    axum::http::StatusCode::NO_CONTENT
}

async fn ai_credential_exhaustion_observations_handler(Path(trace_id): Path<String>) -> impl IntoResponse {
    let observations = AI_CREDENTIAL_EXHAUSTION_OBSERVATIONS
        .lock()
        .remove(&trace_id)
        .unwrap_or_default();
    axum::Json(serde_json::json!({ "observations": observations }))
}

/// Separate native Messages fixture; the existing Chat converter fixture is unchanged.
async fn messages_provider_handler(Path(path): Path<String>, request: AxumRequest<Body>) -> axum::response::Response {
    use serde_json::json;
    let (parts, body) = request.into_parts();
    let Ok(body) = axum::body::to_bytes(body, 1024 * 1024).await else {
        return (axum::http::StatusCode::PAYLOAD_TOO_LARGE, "fixture body limit").into_response();
    };
    let request: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return (axum::http::StatusCode::BAD_REQUEST, "fixture invalid JSON").into_response(),
    };
    let query: std::collections::BTreeMap<String, String> =
        reqwest::Url::parse(&format!("http://fixture{}", parts.uri))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
    let mode = query.get("mock_mode").map(String::as_str).unwrap_or("normal");
    let key = parts
        .headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let credential_label = match key {
        "messages-key-A" => "A",
        "messages-key-B" => "B",
        _ => "invalid",
    };
    let observed = json!({
        "fixture":"messages-native","path":path,"query":query,
        "request":request,
        "credential_label":credential_label,
        "auth_absent":!parts.headers.contains_key("authorization")&&!parts.headers.contains_key("api-key"),
        "workspace_absent":!parts.headers.contains_key("anthropic-workspace-id"),
        "profile_absent":!parts.headers.contains_key("anthropic-user-profile-id"),
        "version":parts.headers.get("anthropic-version").and_then(|v|v.to_str().ok()),
        "beta":parts.headers.get_all("anthropic-beta").iter().filter_map(|v|v.to_str().ok()).collect::<Vec<_>>(),
    });
    let tool = mode == "tool";
    let thinking = mode == "thinking";
    let reason = if tool {
        "tool_use"
    } else if mode == "pause" {
        "pause_turn"
    } else if request["max_tokens"] == 0 {
        "max_tokens"
    } else {
        "end_turn"
    };
    let zero = request["max_tokens"] == 0;
    let content = if zero {
        json!([])
    } else if tool {
        json!([{"type":"tool_use","id":"tool_fixture","name":"weather","input":{"city":"Paris"}}])
    } else if thinking {
        json!([{"type":"thinking","thinking":"fixture thought","signature":"opaque-fixture-signature"},{"type":"text","text":"hello"}])
    } else {
        json!([{"type":"text","text":observed.to_string()}])
    };
    let output_tokens = if zero {
        0
    } else if mode == "quota" {
        32
    } else {
        7
    };
    let mut usage = json!({"input_tokens":8,"output_tokens":output_tokens,"cache_creation_input_tokens":3,"cache_read_input_tokens":2});
    if mode == "composite" {
        usage["iterations"] =
            json!([{"type":"advisor_message","model":"advisor-model","usage":{"input_tokens":3,"output_tokens":4}}]);
    }
    if mode == "iterations" {
        usage["iterations"] =
            json!([{"type":"message","model":"fixture-model","usage":{"input_tokens":13,"output_tokens":7}}]);
    }
    let message = json!({"id":"msg_fixture","type":"message","role":"assistant","model":"fixture-model","content":content,"stop_reason":reason,"stop_sequence":null,"usage":usage});
    if request["stream"] != true {
        let mut wire = message.to_string();
        if mode == "malformed" {
            wire.push_str("invalid-tail");
        }
        return axum::response::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from(wire))
            .unwrap();
    }
    let event = |name: &str, value: serde_json::Value| Bytes::from(format!("event: {name}\ndata: {value}\n\n"));
    let mut frames = vec![
        Bytes::from_static(b": heartbeat\n\n"),
        event(
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_fixture","type":"message","role":"assistant","model":"fixture-model","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":5,"output_tokens":0,"cache_creation_input_tokens":3,"cache_read_input_tokens":0}}}),
        ),
    ];
    for (index, block) in content.as_array().unwrap().iter().enumerate() {
        let mut start = block.clone();
        let ty = block["type"].as_str().unwrap();
        let delta = match ty {
            "tool_use" => {
                start["input"] = json!({});
                json!({"type":"input_json_delta","partial_json":block["input"].to_string()})
            }
            "thinking" => {
                start["thinking"] = json!("");
                start["signature"] = json!("");
                json!({"type":"thinking_delta","thinking":block["thinking"]})
            }
            _ => {
                start["text"] = json!("");
                json!({"type":"text_delta","text":block["text"]})
            }
        };
        frames.push(event(
            "content_block_start",
            json!({"type":"content_block_start","index":index,"content_block":start}),
        ));
        frames.push(event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,"delta":delta}),
        ));
        if ty == "thinking" {
            frames.push(event("content_block_delta",json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":block["signature"]}})));
        }
        if index == 0 && mode == "long" {
            frames.extend((0..1000).map(|_| {
                event(
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}),
                )
            }));
        }
        frames.push(event(
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        ));
    }
    if mode == "oversized" {
        frames.push(event(
            "future_event",
            json!({"type":"future_event","content":"x".repeat(1024*1024)}),
        ));
    }
    frames.push(event(
        "message_delta",
        json!({"type":"message_delta","delta":{},"usage":usage}),
    ));
    frames.push(event(
        "message_delta",
        json!({"type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},"usage":{"output_tokens":output_tokens}}),
    ));
    frames.push(event("message_stop", json!({"type":"message_stop"})));
    if mode == "error" {
        frames.truncate(1);
        frames.push(event(
            "error",
            json!({"type":"error","error":{"type":"overloaded_error","message":"fixture unavailable"}}),
        ));
    }
    if mode == "malformed" {
        frames.push(Bytes::from_static(b"data: {"));
    }
    if mode == "abort" {
        frames.truncate(3);
    }
    let chunks = if mode == "split" {
        frames
            .into_iter()
            .flat_map(|v| v.chunks(7).map(Bytes::copy_from_slice).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    } else {
        frames
    };
    let trace = if mode == "long" {
        parts
            .headers
            .get("x-trace-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    } else {
        None
    };
    if let Some(trace) = &trace {
        AI_LONG_STREAM_OBSERVATIONS
            .lock()
            .insert(trace.clone(), AiLongStreamObservation::default());
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    let abort = mode == "abort";
    let interval = Duration::from_millis(if mode == "split" { 1 } else { 20 });
    tokio::spawn(async move {
        for chunk in chunks {
            if tx.send(Ok(chunk)).await.is_err() {
                if let Some(trace) = &trace {
                    if let Some(state) = AI_LONG_STREAM_OBSERVATIONS.lock().get_mut(trace) {
                        state.upstream_cancelled = true;
                    }
                }
                return;
            }
            if let Some(trace) = &trace {
                if let Some(state) = AI_LONG_STREAM_OBSERVATIONS.lock().get_mut(trace) {
                    state.chunks_queued += 1;
                }
            }
            tokio::time::sleep(interval).await;
        }
        if abort {
            let _ = tx
                .send(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "intentional Messages fixture abort",
                )))
                .await;
        }
        if let Some(trace) = &trace {
            if let Some(state) = AI_LONG_STREAM_OBSERVATIONS.lock().get_mut(trace) {
                state.provider_completed = true;
            }
        }
    });
    let mut response = axum::response::Response::builder()
        .status(200)
        .header("content-type", "text/event-stream");
    if mode == "encoded" {
        response = response.header("content-encoding", "gzip");
    }
    response
        .body(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)))
        .unwrap()
}

const ANTHROPIC_PROVIDER_KEY: &str = "anthropic-provider-test-key";
const ANTHROPIC_PROVIDER_EXPECTED_HOSTS: &[&str] = &[
    "127.0.0.1:30001",
    "edgion-test-server.edgion-test.svc.cluster.local:30001",
];

#[derive(Debug, serde::Serialize)]
struct AnthropicProviderObservation {
    fixture_id: &'static str,
    credential_valid: bool,
    authorization_absent: bool,
    version_valid: bool,
    beta_absent: bool,
    accept_encoding_identity: bool,
    host_valid: bool,
    path_valid: bool,
    query_absent: bool,
    model_canonical: bool,
    max_tokens_valid: bool,
    messages_valid: bool,
    system_valid: bool,
    openai_fields_absent: bool,
}

fn anthropic_provider_observation(
    path: &str,
    uri: &axum::http::Uri,
    headers: &axum::http::HeaderMap,
    body: &serde_json::Value,
) -> AnthropicProviderObservation {
    let messages_valid = body
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|messages| {
            !messages.is_empty()
                && messages.iter().all(|message| {
                    message
                        .get("role")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|role| matches!(role, "user" | "assistant"))
                })
        });
    AnthropicProviderObservation {
        fixture_id: "anthropic-native-v1",
        credential_valid: headers.get("x-api-key").and_then(|value| value.to_str().ok())
            == Some(ANTHROPIC_PROVIDER_KEY),
        authorization_absent: !headers.contains_key(axum::http::header::AUTHORIZATION),
        version_valid: headers.get("anthropic-version").and_then(|value| value.to_str().ok()) == Some("2023-06-01"),
        beta_absent: !headers.contains_key("anthropic-beta"),
        accept_encoding_identity: {
            let mut values = headers.get_all(axum::http::header::ACCEPT_ENCODING).iter();
            values.next().and_then(|value| value.to_str().ok()) == Some("identity") && values.next().is_none()
        },
        host_valid: headers
            .get(axum::http::header::HOST)
            .and_then(|value| value.to_str().ok())
            .or_else(|| uri.authority().map(|authority| authority.as_str()))
            .is_some_and(|host| ANTHROPIC_PROVIDER_EXPECTED_HOSTS.contains(&host)),
        path_valid: path == "prefix/v1/messages",
        query_absent: uri.query().is_none(),
        model_canonical: body.get("model").and_then(serde_json::Value::as_str) == Some("claude-fixture"),
        max_tokens_valid: body.get("max_tokens").and_then(serde_json::Value::as_u64) == Some(64),
        messages_valid,
        system_valid: body
            .get("system")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|blocks| {
                !blocks.is_empty()
                    && blocks.iter().all(|block| {
                        block.get("type").and_then(serde_json::Value::as_str) == Some("text")
                            && block.get("text").and_then(serde_json::Value::as_str).is_some()
                    })
            }),
        openai_fields_absent: !body.as_object().is_some_and(|object| {
            object.contains_key("max_completion_tokens") || object.contains_key("stream_options")
        }),
    }
}

fn anthropic_message_response(content: serde_json::Value, stop_reason: &str) -> axum::response::Response {
    let response = serde_json::json!({
        "id": "msg_fixture",
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": "claude-provider-alias",
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": 5,
            "cache_creation_input_tokens": 3,
            "cache_read_input_tokens": 2,
            "output_tokens": 7
        }
    });
    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("content-type", "application/json")
        .header("request-id", "provider-request-id-must-be-stripped")
        .body(Body::from(response.to_string()))
        .unwrap()
}

fn anthropic_event(name: &str, value: serde_json::Value) -> Bytes {
    Bytes::from(format!("event: {name}\ndata: {}\n\n", value))
}

fn anthropic_stream_plan(mode: &str, observation: &AnthropicProviderObservation) -> AiStreamPlan {
    if mode == "malformed" {
        return AiStreamPlan {
            chunks: vec![Bytes::from_static(b"event: message_start\ndata: {not-json}\n\n")],
            interval: Duration::ZERO,
            abort_after_chunks: false,
        };
    }
    if mode == "oversized" {
        return AiStreamPlan {
            chunks: vec![Bytes::from(format!(
                "event: future_event\ndata: {{\"type\":\"future_event\",\"payload\":\"{}\"}}\n\n",
                "x".repeat(oversized_event_payload_bytes())
            ))],
            interval: Duration::ZERO,
            abort_after_chunks: false,
        };
    }

    let start = anthropic_event(
        "message_start",
        serde_json::json!({
            "type":"message_start",
            "message":{
                "id":"msg_stream_fixture",
                "type":"message",
                "role":"assistant",
                "content":[],
                "model":"claude-provider-alias",
                "stop_reason":null,
                "stop_sequence":null,
                "usage":{
                    "input_tokens":5,
                    "cache_creation_input_tokens":3,
                    "cache_read_input_tokens":2,
                    "output_tokens":0
                }
            }
        }),
    );
    let (block_start, deltas, stop_reason) = if mode == "stream_tool" {
        (
            anthropic_event(
                "content_block_start",
                serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_fixture","name":"weather","input":{}}}),
            ),
            vec![
                anthropic_event(
                    "content_block_delta",
                    serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"city\":"}}),
                ),
                anthropic_event(
                    "content_block_delta",
                    serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"Paris\"}"}}),
                ),
            ],
            "tool_use",
        )
    } else {
        let observation = serde_json::to_string(observation).unwrap_or_else(|_| "{}".to_string());
        (
            anthropic_event(
                "content_block_start",
                serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            ),
            vec![anthropic_event(
                "content_block_delta",
                serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":observation}}),
            )],
            "end_turn",
        )
    };
    let mut chunks = vec![start, block_start];
    chunks.extend(deltas);
    chunks.extend([
        anthropic_event(
            "content_block_stop",
            serde_json::json!({"type":"content_block_stop","index":0}),
        ),
        anthropic_event(
            "message_delta",
            serde_json::json!({
                "type":"message_delta",
                "delta":{"stop_reason":stop_reason,"stop_sequence":null},
                "usage":{"output_tokens":7}
            }),
        ),
        anthropic_event("message_stop", serde_json::json!({"type":"message_stop"})),
    ]);
    AiStreamPlan {
        chunks,
        interval: if mode == "paced" {
            Duration::from_millis(175)
        } else {
            Duration::ZERO
        },
        abort_after_chunks: false,
    }
}

async fn anthropic_provider_handler(Path(path): Path<String>, request: AxumRequest<Body>) -> axum::response::Response {
    ANTHROPIC_REQUEST_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (parts, body) = request.into_parts();
    if parts
        .headers
        .get("x-resilience-case")
        .and_then(|value| value.to_str().ok())
        == Some("cross-provider")
    {
        if let Some(trace_id) = parts.headers.get("x-trace-id").and_then(|value| value.to_str().ok()) {
            let fixture_id = parts
                .headers
                .get("x-resilience-fixture-id")
                .and_then(|value| value.to_str().ok())
                .unwrap_or(trace_id);
            AI_RESILIENCE_STATE
                .lock()
                .entry(fixture_id.to_string())
                .or_default()
                .counts[1] += 1;
        }
    }
    let mode = parts
        .headers
        .get("x-anthropic-fixture-mode")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("default")
        .to_string();
    let Ok(body) = axum::body::to_bytes(body, 1024 * 1024).await else {
        return axum::response::Response::builder()
            .status(axum::http::StatusCode::PAYLOAD_TOO_LARGE)
            .body(Body::empty())
            .unwrap();
    };
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let observation = anthropic_provider_observation(&path, &parts.uri, &parts.headers, &json);

    let bodyless_status = match mode.as_str() {
        "bodyless_200" => Some(axum::http::StatusCode::OK),
        "bodyless_sse_200" => Some(axum::http::StatusCode::OK),
        "bodyless_503" => Some(axum::http::StatusCode::SERVICE_UNAVAILABLE),
        _ => None,
    };
    if let Some(status) = bodyless_status {
        let content_type = if mode == "bodyless_sse_200" {
            "text/event-stream"
        } else {
            "application/json"
        };
        return axum::response::Response::builder()
            .status(status)
            .header("content-type", content_type)
            .header("request-id", "bodyless-provider-id-must-be-stripped")
            .header("x-provider-private", "bodyless-private-detail")
            .body(Body::empty())
            .unwrap();
    }

    let fixed_status = match mode.as_str() {
        "status_400" => Some(axum::http::StatusCode::BAD_REQUEST),
        "status_401" => Some(axum::http::StatusCode::UNAUTHORIZED),
        "status_429" => Some(axum::http::StatusCode::TOO_MANY_REQUESTS),
        _ => None,
    };
    if let Some(status) = fixed_status {
        return axum::response::Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .header("request-id", "provider-request-id-must-be-stripped")
            .header("location", "http://provider.internal/private")
            .header("set-cookie", "provider_session=must-not-pass")
            .header("retry-after", "3")
            .body(Body::from(
                r#"{"type":"error","error":{"type":"provider_private","message":"provider private detail"}}"#,
            ))
            .unwrap();
    }

    if mode.starts_with("unary_") {
        let mut response =
            anthropic_message_response(serde_json::json!([{"type":"text","text":"unary fixture"}]), "end_turn");
        match mode.as_str() {
            "unary_malformed" => {
                *response.body_mut() = Body::from("{\"provider_private\":\"provider private detail\"} broken")
            }
            "unary_oversized" => *response.body_mut() = Body::from("x".repeat(1024 * 1024 + 1)),
            "unary_wrong_media" => {
                response
                    .headers_mut()
                    .insert("content-type", axum::http::HeaderValue::from_static("text/plain"));
            }
            "unary_gzip" => {
                response
                    .headers_mut()
                    .insert("content-encoding", axum::http::HeaderValue::from_static("gzip"));
            }
            _ => {}
        }
        return response;
    }

    if mode == "nonstream_tool" {
        let tools_valid = json
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|tools| {
                tools.len() == 2
                    && tools.iter().all(|tool| {
                        tool.get("name").and_then(serde_json::Value::as_str).is_some()
                            && tool.get("input_schema").is_some_and(serde_json::Value::is_object)
                    })
            });
        if !tools_valid {
            return axum::response::Response::builder()
                .status(axum::http::StatusCode::BAD_REQUEST)
                .body(Body::empty())
                .unwrap();
        }
        return anthropic_message_response(
            serde_json::json!([
                {"type":"tool_use","id":"toolu_one","name":"weather","input":{"city":"Paris"}},
                {"type":"tool_use","id":"toolu_two","name":"clock","input":{"zone":"UTC"}}
            ]),
            "tool_use",
        );
    }

    let streaming = json.get("stream").and_then(serde_json::Value::as_bool) == Some(true);
    if !streaming {
        let observation = serde_json::to_string(&observation).unwrap_or_else(|_| "{}".to_string());
        return anthropic_message_response(serde_json::json!([{"type":"text","text":observation}]), "end_turn");
    }

    let plan = anthropic_stream_plan(&mode, &observation);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    tokio::spawn(async move {
        for chunk in plan.chunks {
            if tx.send(Ok(chunk)).await.is_err() {
                return;
            }
            if !plan.interval.is_zero() {
                tokio::time::sleep(plan.interval).await;
            }
        }
    });
    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("request-id", "provider-request-id-must-be-stripped")
        .body(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)))
        .unwrap()
}

async fn anthropic_request_count_handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "count": ANTHROPIC_REQUEST_COUNT.load(std::sync::atomic::Ordering::Relaxed)
    }))
}

/// /cb-fail/{test_id}/{fail_count}
/// Returns 502 for the first `fail_count` calls per test_id, then 200.
async fn cb_fail_handler(Path((test_id, fail_count)): Path<(String, u32)>) -> impl IntoResponse {
    cb_fail_core(test_id, fail_count, 502)
}

/// /cb-fail/{test_id}/{fail_count}/{status}
/// Returns `status` for the first `fail_count` calls per test_id, then 200.
/// Lets tests drive application (500) vs gateway (502/503/504) failure classes.
async fn cb_fail_status_handler(Path((test_id, fail_count, status)): Path<(String, u32, u16)>) -> impl IntoResponse {
    cb_fail_core(test_id, fail_count, status)
}

fn cb_fail_core(test_id: String, fail_count: u32, fail_status: u16) -> impl IntoResponse {
    use axum::http::StatusCode;

    let attempt = {
        let mut store = CB_COUNTER.lock();
        let count = store.entry(test_id.clone()).or_insert(0);
        *count += 1;
        *count
    };

    let status = if attempt <= fail_count {
        StatusCode::from_u16(fail_status).unwrap_or(StatusCode::BAD_GATEWAY)
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

// ============================================================================
// Fake HTTP DNS endpoint (for httpdns resolver integration tests)
// ============================================================================

/// Parse the `/dns/resolve` query string into `(host, ips)`.
///
/// - `host` is the domain being resolved (empty when absent).
/// - `ips` come from an optional `ip=<a>[,<b>...]` param; when absent the
///   endpoint defaults to `127.0.0.1` (the local test backend), so a scenario
///   can point an ExternalName at the loopback backend without a real DNS.
///
/// Values are used verbatim (no percent-decoding) — tests pass plain IP literals.
fn parse_dns_query(query: &str) -> (String, Vec<String>) {
    let mut host = String::new();
    let mut ips: Vec<String> = Vec::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let mut it = pair.splitn(2, '=');
        let key = it.next().unwrap_or("");
        let val = it.next().unwrap_or("");
        match key {
            "host" => host = val.to_string(),
            "ip" => {
                for ip in val.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    ips.push(ip.to_string());
                }
            }
            _ => {}
        }
    }
    if ips.is_empty() {
        ips.push("127.0.0.1".to_string());
    }
    (host, ips)
}

fn resolve_dns_answer_tokens(ips: Vec<String>, pod_ip: Option<&str>) -> Vec<String> {
    ips.into_iter()
        .map(|ip| match (ip.as_str(), pod_ip) {
            ("pod", Some(value)) => value.to_string(),
            _ => ip,
        })
        .collect()
}

/// `/dns/resolve?host=<domain>[&ip=<ip>...]` — the fake HTTP DNS endpoint.
///
/// Returns the generic httpdns response body `{"ips":[...]}` (matching the
/// default `HttpDnsResponseFormat` `ipPath: ips`) and records one resolution for
/// `host` so caching tests can assert the endpoint was (or was not) re-hit.
async fn dns_resolve_handler(axum::extract::RawQuery(query): axum::extract::RawQuery) -> impl IntoResponse {
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;

    let raw = query.unwrap_or_default();
    let (host, ips) = parse_dns_query(&raw);
    let pod_ip = std::env::var("POD_IP").ok();
    let ips = resolve_dns_answer_tokens(ips, pod_ip.as_deref());

    {
        let mut store = DNS_RESOLVE_COUNTER.lock();
        *store.entry(host.clone()).or_insert(0) += 1;
        info!("dns/resolve: host={} ips={:?}", host, ips);
    }

    (StatusCode::OK, Json(json!({ "ips": ips })))
}

/// `/dns/hits/{host}` — resolution count for `host` (0 when never resolved).
async fn dns_hits_handler(Path(host): Path<String>) -> impl IntoResponse {
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;

    let hits = DNS_RESOLVE_COUNTER.lock().get(&host).copied().unwrap_or(0);
    (StatusCode::OK, Json(json!({ "host": host, "hits": hits })))
}

/// `/dns/reset` — clear all HTTP DNS resolve counters.
async fn dns_reset_handler() -> impl IntoResponse {
    use axum::http::StatusCode;
    let mut store = DNS_RESOLVE_COUNTER.lock();
    let count = store.len();
    store.clear();
    info!("dns/reset: cleared {} host entries", count);
    (StatusCode::OK, format!("dns-reset-ok: cleared {}", count))
}

/// Webhook resolve handler — returns resolved key value AND echoes the inbound
/// request so integration tests can verify outbound-request construction.
///
/// Simulates an external key-resolution service. Reads the `X-Tenant-Id` header
/// from the forwarded request and returns:
/// - Response header `X-Resolved-Key` with the resolved value
/// - Response header `X-Webhook-Status` = "resolved"
/// - Set-Cookie `webhook_session=wh-sess-<tenant>`
/// - JSON body:
///   ```json
///   {"data":{
///     "user_id":"uid-<tenant>","tenant":"<tenant>",
///     "method":"GET","path":"/webhook/resolve","query":"a=b",
///     "echo_headers":{"<lowercased-name>":"<value>", ...},
///     "echo_args":{"<name>":"<value>", ...},
///     "echo_cookies":{"<name>":"<value>", ...},
///     "echo_body": <parsed JSON of the received request body, or null>
///   }}
///   ```
///
/// The `echo_*` maps reflect exactly what the gateway sent on the wire, so a
/// webhook `extract` of e.g. `data.echo_headers.x-source` surfaces the outbound
/// header value into a ctx var (and thus the access log) for assertion. The
/// `data.path` field lets path-override tests prove which path was called.
/// `echo_body` mirrors the same idea for the request body: it is the request
/// body parsed as JSON (so `extract` can walk into fields like
/// `data.echo_body.userId`), or `Null` when the body is empty or not valid JSON.
///
/// If `X-Tenant-Id` is missing, returns a default resolution. The legacy
/// `user_id`/`tenant` body keys and response headers are preserved so existing
/// extraction paths keep working — the echo fields are purely additive.
///
/// Note: query/cookie values are echoed verbatim (no percent-decoding); tests
/// that assert on them use plain alphanumeric values.
async fn webhook_resolve_handler(req: AxumRequest<Body>) -> impl IntoResponse {
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;
    use std::collections::BTreeMap;

    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    let (parts, body) = req.into_parts();
    let body_bytes = to_bytes(body, 8 * 1024).await.unwrap_or_default();
    // Echo the received request body: parsed JSON when possible (lets fixtures
    // extract data.echo_body.<field>), else Null.
    let echo_body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null);

    let headers = &parts.headers;
    let tenant = headers
        .get("x-tenant-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    // Echo every inbound header (keys lowercased for stable lookup).
    let mut echo_headers: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in headers.iter() {
        if let Ok(val) = v.to_str() {
            echo_headers.insert(k.as_str().to_lowercase(), val.to_string());
        }
    }

    // Echo query args (raw, split on `&` then `=`).
    let mut echo_args: BTreeMap<String, String> = BTreeMap::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let mut it = pair.splitn(2, '=');
        let key = it.next().unwrap_or("").to_string();
        let val = it.next().unwrap_or("").to_string();
        echo_args.insert(key, val);
    }

    // Echo cookies parsed from the single `Cookie` header the gateway sends.
    let mut echo_cookies: BTreeMap<String, String> = BTreeMap::new();
    if let Some(cookie_hdr) = headers.get("cookie").and_then(|v| v.to_str().ok()) {
        for pair in cookie_hdr.split(';') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let mut it = pair.splitn(2, '=');
            let key = it.next().unwrap_or("").trim().to_string();
            let val = it.next().unwrap_or("").trim().to_string();
            echo_cookies.insert(key, val);
        }
    }

    let resolved_key = format!("resolved-{}", tenant);
    let user_id = format!("uid-{}", tenant);
    let cookie_val = format!("webhook_session=wh-sess-{}", tenant);

    let body = json!({
        "data": {
            "user_id": user_id,
            "tenant": tenant,
            "method": method,
            "path": path,
            "query": query,
            "echo_headers": echo_headers,
            "echo_args": echo_args,
            "echo_cookies": echo_cookies,
            "echo_body": echo_body,
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

/// Upstream that emits TWO Set-Cookie headers, simulating an app that sets its
/// own cookies. Used to verify the gateway preserves every upstream Set-Cookie
/// when a plugin (e.g. CSRF) also queues its own Set-Cookie via append semantics.
async fn multi_set_cookie_handler() -> impl IntoResponse {
    use axum::http::{header::SET_COOKIE, HeaderMap, HeaderValue, StatusCode};

    let mut headers = HeaderMap::new();
    headers.append(SET_COOKIE, HeaderValue::from_static("upstream_a=aaa; Path=/"));
    headers.append(SET_COOKIE, HeaderValue::from_static("upstream_b=bbb; Path=/"));
    (StatusCode::OK, headers, "multi-set-cookie-ok")
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

async fn status_empty_handler(Path(code): Path<u16>) -> impl IntoResponse {
    // Body-less response: status code with an empty body (Content-Length: 0).
    // Exercises the filter-04b path where the upstream sends no body bytes.
    (
        axum::http::StatusCode::from_u16(code).unwrap_or(axum::http::StatusCode::OK),
        String::new(),
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
    let mut resp = format!(
        "Server: {}\nClient: {}\nMethod: {}\nPath: {}\nQuery: {}\n",
        server_addr,
        addr,
        req.method(),
        req.uri().path(),
        req.uri().query().unwrap_or_default()
    );
    // Echo headers in the same format as `echo_handler` so plugin tests that
    // rewrite the URI to an unmapped path can still verify header rewrites.
    resp.push_str("\nHeaders:\n");
    for (key, value) in req.headers() {
        let display = display_header_name(key.as_str());
        resp.push_str(&format!("  {}: {}\n", display, value.to_str().unwrap_or("<invalid>")));
    }
    resp
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
        // Echo any `x-*` request metadata into the response message. gRPC has
        // no generic header-echo endpoint the way the HTTP catch-all handler
        // does (`echo_handler`/`echo_post_handler` above); this is the
        // minimal extension needed to prove GRPCRoute filter inheritance
        // (e.g. a delegation parent's RequestHeaderModifier) reaches the
        // backend, mirroring what the HTTP echo handlers already do for
        // HTTPRoute filter tests. Only `x-*` names are echoed to keep the
        // dump small and avoid leaking internal gRPC metadata.
        let echoed: Vec<String> = request
            .metadata()
            .iter()
            .filter_map(|kv| match kv {
                tonic::metadata::KeyAndValueRef::Ascii(key, value) => {
                    let key = key.as_str();
                    if key.starts_with("x-") {
                        value.to_str().ok().map(|v| format!("{key}={v}"))
                    } else {
                        None
                    }
                }
                tonic::metadata::KeyAndValueRef::Binary(_, _) => None,
            })
            .collect();

        let name = request.into_inner().name;

        let message = if echoed.is_empty() {
            format!("Hello, {}!", name)
        } else {
            format!("Hello, {}! metadata=({})", name, echoed.join(","))
        };

        let response = HelloResponse {
            message,
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
    expected_scheme: &'static str,
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
        if req.uri().scheme_str() != Some(self.expected_scheme) {
            return Box::pin(async {
                Ok(http::Response::builder()
                    .status(200)
                    .header("grpc-status", "13")
                    .header("content-type", "application/grpc")
                    .body(tonic::body::empty_body())
                    .expect("static health response headers are valid"))
            });
        }
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
            expected_scheme: "http",
        })
        .serve(addr)
        .await?;

    Ok(())
}

// ============================================================================
// Fake ext_proc (Envoy External Processor) gRPC Server
// ============================================================================
//
// Minimal `envoy.service.ext_proc.v3.ExternalProcessor` implementation for the
// ExtProc plugin integration tests. Behavior:
//   - RequestHeaders: if the request carries `x-ext-proc-block`, return an
//     ImmediateResponse 403 (short-circuit); otherwise add `x-ext-proc-req`.
//   - ResponseHeaders: add `x-ext-proc-resp`.
mod ext_proc_server {
    use std::net::SocketAddr;
    use std::pin::Pin;

    use anyhow::Result;
    use edgion_common::ext_proc::proto::external_processor_server::{ExternalProcessor, ExternalProcessorServer};
    use edgion_common::ext_proc::proto::{
        body_mutation, common_response::ResponseStatus, processing_request, processing_response, BodyMutation,
        BodyResponse, CommonResponse, HeaderAppendAction, HeaderMutation, HeaderValue as EpHeaderValue,
        HeaderValueOption, HeadersResponse, HttpStatus, ImmediateResponse, ProcessingRequest, ProcessingResponse,
    };
    use futures::Stream;
    use tonic::{transport::Server, Request, Response, Status};
    use tracing::info;

    const BLOCK_HEADER: &str = "x-ext-proc-block";
    /// Trigger: make the processor emit a malformed (empty) verdict, to exercise
    /// the gateway's fail-mode handling of an unusable answer.
    const BAD_VERDICT_HEADER: &str = "x-ext-proc-badverdict";
    const TEST_MODE_HEADER: &str = "x-ext-proc-test-mode";

    #[derive(Default)]
    struct FakeProcessor;

    /// Build a single `set` (overwrite-or-add) header mutation option.
    fn set_opt(key: &str, value: &str) -> HeaderValueOption {
        HeaderValueOption {
            header: Some(EpHeaderValue {
                key: key.to_string(),
                value: value.to_string(),
                raw_value: Vec::new(),
            }),
            append_action: HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
            keep_empty_value: false,
        }
    }

    fn headers_verdict(is_request: bool, set_key: &str) -> ProcessingResponse {
        let common = CommonResponse {
            status: ResponseStatus::Continue as i32,
            header_mutation: Some(HeaderMutation {
                set_headers: vec![set_opt(set_key, "seen")],
                remove_headers: Vec::new(),
            }),
            body_mutation: None,
            trailers: None,
            clear_route_cache: false,
        };
        let hr = HeadersResponse { response: Some(common) };
        let response = if is_request {
            processing_response::Response::RequestHeaders(hr)
        } else {
            processing_response::Response::ResponseHeaders(hr)
        };
        ProcessingResponse {
            response: Some(response),
            mode_override: None,
        }
    }

    fn immediate_403() -> ProcessingResponse {
        ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(ImmediateResponse {
                status: Some(HttpStatus { code: 403 }),
                headers: Some(HeaderMutation {
                    set_headers: vec![set_opt("x-ext-proc-denied", "1")],
                    remove_headers: Vec::new(),
                }),
                body: b"blocked by ext_proc".to_vec(),
                grpc_status: None,
                details: String::new(),
            })),
            mode_override: None,
        }
    }

    fn request_body_verdict_with(body: &'static [u8]) -> ProcessingResponse {
        ProcessingResponse {
            response: Some(processing_response::Response::RequestBody(BodyResponse {
                response: Some(CommonResponse {
                    status: ResponseStatus::Continue as i32,
                    header_mutation: None,
                    body_mutation: Some(BodyMutation {
                        mutation: Some(body_mutation::Mutation::Body(body.to_vec())),
                    }),
                    trailers: None,
                    clear_route_cache: false,
                }),
            })),
            mode_override: None,
        }
    }

    fn response_body_verdict_with(body: &'static [u8]) -> ProcessingResponse {
        ProcessingResponse {
            response: Some(processing_response::Response::ResponseBody(BodyResponse {
                response: Some(CommonResponse {
                    status: ResponseStatus::Continue as i32,
                    header_mutation: None,
                    body_mutation: Some(BodyMutation {
                        mutation: Some(body_mutation::Mutation::Body(body.to_vec())),
                    }),
                    trailers: None,
                    clear_route_cache: false,
                }),
            })),
            mode_override: None,
        }
    }

    fn request_headers_verdict(status: ResponseStatus, body: Option<&'static [u8]>) -> ProcessingResponse {
        ProcessingResponse {
            response: Some(processing_response::Response::RequestHeaders(HeadersResponse {
                response: Some(CommonResponse {
                    status: status as i32,
                    header_mutation: None,
                    body_mutation: body.map(|body| BodyMutation {
                        mutation: Some(body_mutation::Mutation::Body(body.to_vec())),
                    }),
                    trailers: None,
                    clear_route_cache: false,
                }),
            })),
            mode_override: None,
        }
    }

    fn header_value(headers: &edgion_common::ext_proc::proto::HeaderMap, name: &str) -> Option<String> {
        headers
            .headers
            .iter()
            .find(|header| header.key.eq_ignore_ascii_case(name))
            .map(|header| {
                if header.raw_value.is_empty() {
                    header.value.clone()
                } else {
                    String::from_utf8_lossy(&header.raw_value).into_owned()
                }
            })
    }

    fn handle(
        req: ProcessingRequest,
        mode: &mut Option<String>,
        request_phase_seen: &mut bool,
    ) -> (Option<ProcessingResponse>, bool) {
        let Some(request) = req.request else {
            return (None, false);
        };
        match request {
            processing_request::Request::RequestHeaders(h) => {
                *request_phase_seen = true;
                *mode = h
                    .headers
                    .as_ref()
                    .and_then(|headers| header_value(headers, TEST_MODE_HEADER));
                let has = |name: &str| {
                    h.headers
                        .as_ref()
                        .map(|m| m.headers.iter().any(|hv| hv.key.eq_ignore_ascii_case(name)))
                        .unwrap_or(false)
                };
                if has(BAD_VERDICT_HEADER) {
                    // Malformed verdict: an empty ProcessingResponse (no oneof).
                    return (
                        Some(ProcessingResponse {
                            response: None,
                            mode_override: None,
                        }),
                        false,
                    );
                }
                match mode.as_deref() {
                    Some("wrong-order") => (Some(request_body_verdict_with(b"wrong-phase-body")), false),
                    Some("missing-body") => (Some(headers_verdict(true, "x-ext-proc-req")), true),
                    Some("close-before-verdict") => (None, true),
                    Some("continue-and-replace") => (
                        Some(request_headers_verdict(
                            ResponseStatus::ContinueAndReplace,
                            Some(b"replaced-at-headers"),
                        )),
                        false,
                    ),
                    _ => (
                        Some(if has(BLOCK_HEADER) {
                            immediate_403()
                        } else {
                            headers_verdict(true, "x-ext-proc-req")
                        }),
                        false,
                    ),
                }
            }
            processing_request::Request::ResponseHeaders(_) if *request_phase_seen => match mode.as_deref() {
                Some("close-on-response-headers") => (None, true),
                _ => (Some(headers_verdict(false, "x-ext-proc-resp")), false),
            },
            processing_request::Request::RequestBody(_) => {
                let response = match mode.as_deref() {
                    Some("duplicate-header") | Some("wrong-order") => headers_verdict(true, "x-duplicate-phase"),
                    Some("bodyless-create") => request_body_verdict_with(b"created-by-ext-proc"),
                    // Streamed mid-body rejection: the first body chunk draws an
                    // ImmediateResponse instead of a body verdict.
                    Some("body-block") => immediate_403(),
                    _ => request_body_verdict_with(b"processed-by-ext-proc"),
                };
                (Some(response), false)
            }
            processing_request::Request::ResponseBody(_) if *request_phase_seen => (
                Some(response_body_verdict_with(b"response-by-attached-ext-proc")),
                false,
            ),
            _ => (None, false),
        }
    }

    #[tonic::async_trait]
    impl ExternalProcessor for FakeProcessor {
        type ProcessStream = Pin<Box<dyn Stream<Item = Result<ProcessingResponse, Status>> + Send>>;

        async fn process(
            &self,
            request: Request<tonic::Streaming<ProcessingRequest>>,
        ) -> Result<Response<Self::ProcessStream>, Status> {
            let mut inbound = request.into_inner();
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tokio::spawn(async move {
                let mut mode = None;
                let mut request_phase_seen = false;
                while let Ok(Some(req)) = inbound.message().await {
                    let (response, terminal) = handle(req, &mut mode, &mut request_phase_seen);
                    if mode.as_deref() == Some("long-idle") {
                        mode = Some("long-idle-complete".to_string());
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    }
                    if let Some(resp) = response {
                        if tx.send(Ok(resp)).await.is_err() {
                            break;
                        }
                    }
                    if terminal {
                        break;
                    }
                }
            });
            Ok(Response::new(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))))
        }
    }

    pub async fn start(port: u16) -> Result<()> {
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        info!("✓ ext_proc server listening on http://{}", addr);
        Server::builder()
            .add_service(ExternalProcessorServer::new(FakeProcessor))
            .serve(addr)
            .await?;
        Ok(())
    }

    /// TLS variant: serves the same fake processor over HTTPS/h2 so the gateway's
    /// hand-built rustls ext_proc connector exercises a real outbound TLS
    /// handshake (ALPN h2 is negotiated by tonic). Used by the ExtProc-over-TLS
    /// integration scenario.
    pub async fn start_tls(
        port: u16,
        cert_path: String,
        key_path: String,
        client_ca_path: Option<String>,
    ) -> Result<()> {
        use tonic::transport::{Certificate, Identity, ServerTlsConfig};

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let cert = tokio::fs::read(&cert_path)
            .await
            .map_err(|e| anyhow::anyhow!("ext_proc TLS: failed to read cert {}: {}", cert_path, e))?;
        let key = tokio::fs::read(&key_path)
            .await
            .map_err(|e| anyhow::anyhow!("ext_proc TLS: failed to read key {}: {}", key_path, e))?;
        let identity = Identity::from_pem(cert, key);

        let mut tls = ServerTlsConfig::new().identity(identity);
        if let Some(client_ca_path) = client_ca_path.as_ref() {
            let client_ca = tokio::fs::read(client_ca_path)
                .await
                .map_err(|e| anyhow::anyhow!("ext_proc mTLS: failed to read client CA {}: {}", client_ca_path, e))?;
            tls = tls.client_ca_root(Certificate::from_pem(client_ca));
        }

        info!(
            "✓ ext_proc TLS server listening on https://{} (client auth: {})",
            addr,
            if client_ca_path.is_some() {
                "required"
            } else {
                "disabled"
            }
        );
        Server::builder()
            .tls_config(tls)?
            .add_service(ExternalProcessorServer::new(FakeProcessor))
            .add_service(super::GrpcHealthServer {
                inner: std::sync::Arc::new(super::GrpcHealthService),
                expected_scheme: "https",
            })
            .serve(addr)
            .await?;
        Ok(())
    }
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
        .route("/verify-deny-multivalue", get(auth_deny_multivalue_handler))
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

/// Fixed denial fixture: keep repeated fields separate, including a cookie clearance.
async fn auth_deny_multivalue_handler() -> impl IntoResponse {
    use axum::http::{HeaderValue, StatusCode};

    let mut response = (StatusCode::UNAUTHORIZED, "Authentication denied\n").into_response();
    let headers = response.headers_mut();
    for challenge in ["Bearer realm=\"primary\"", "Basic realm=\"fallback\""] {
        headers.append("www-authenticate", HeaderValue::from_static(challenge));
    }
    for cookie in [
        "session=renewed; Path=/; HttpOnly; SameSite=Lax",
        "csrf=; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
    ] {
        headers.append("set-cookie", HeaderValue::from_static(cookie));
    }
    headers.insert("x-auth-unselected", HeaderValue::from_static("not-for-client"));
    response
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

    let Ok(body_bytes) = to_bytes(req.into_body(), 8 * 1024).await else {
        return (StatusCode::BAD_REQUEST, Json(json!({"active": false})));
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
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::Json;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    let (parts, body) = req.into_parts();
    let headers = parts.headers;
    let Ok(body) = to_bytes(body, 1024 * 1024).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if let Some(expected) = headers
        .get("x-auth-expected-body-sha256")
        .and_then(|value| value.to_str().ok())
    {
        let actual = hex::encode(Sha256::digest(&body));
        if actual != expected {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

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
            // Reflect whether specific headers were present in the auth request.
            // Used by the dynamic hop-by-hop stripping integration test (RFC 9110 §7.6.1):
            // the gateway must strip headers listed in Connection before forwarding to the
            // auth server; these response headers let the test verify the auth server's view.
            let saw_custom_hop = if headers.contains_key("x-custom-hop") {
                "yes"
            } else {
                "no"
            };
            let saw_visible_header = if headers.contains_key("x-visible-header") {
                "yes"
            } else {
                "no"
            };
            // `Expect` is end-to-end, so hop-by-hop filtering keeps it. forwardAll
            // withholds it anyway: this auth request sends no body of its own, so
            // the expectation could never be satisfied here (FA-BODY-002).
            let saw_expect = if headers.contains_key("expect") { "yes" } else { "no" };
            (
                StatusCode::OK,
                [
                    ("X-User-ID", "user-123"),
                    ("X-User-Role", "member"),
                    ("X-User-Email", "test@example.com"),
                    ("X-Saw-Custom-Hop", saw_custom_hop),
                    ("X-Saw-Visible-Header", saw_visible_header),
                    ("X-Saw-Expect", saw_expect),
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
        // Valid token that returns 204 No Content (for success.statusCodes integration test).
        // Sends an empty body with X-User-ID so upstream-header propagation can be verified.
        "Bearer valid-204-token" => (
            StatusCode::NO_CONTENT,
            [("X-User-ID", "user-204"), ("X-Auth-Status", "ok-204")],
            "",
        )
            .into_response(),
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

// ============================================================================
// Wildcard-cert HTTPS Backend (for EdgionBackendTrafficPolicy/UpstreamAuthority
// integration testing — Task 8 of the dynamic-upstream-authority feature)
// ============================================================================
//
// A single wildcard certificate (`*.sandbox.internal`) covers every tenant
// subdomain the scenario dispatches to (`acme.sandbox.internal`,
// `globex.sandbox.internal`, the health-probe authority `probe.sandbox.internal`,
// ...). Unlike `MultiSniResolver` (which recognizes exactly two fixed
// hostnames), this resolver accepts any SNI *inside* the wildcard pattern and
// rejects everything else — including the bare parent domain
// `sandbox.internal`, which a wildcard certificate does not cover per RFC 6125.
// A successful handshake is therefore already proof the gateway sent an
// in-pattern SNI; the exact per-tenant value is verified at the HTTP layer via
// the `Host` header the shared `/echo` handler echoes back.

#[derive(Debug)]
struct VhostWildcardResolver {
    cert: Arc<CertifiedKey>,
}

impl ResolvesServerCert for VhostWildcardResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        match hello.server_name() {
            // `ends_with(".sandbox.internal")` cannot match the bare parent
            // domain "sandbox.internal" itself (the needle is strictly longer
            // than the parent), so the RFC 6125 exclusion falls out for free.
            Some(name) if name.to_ascii_lowercase().ends_with(".sandbox.internal") => Some(self.cert.clone()),
            // Reject everything else (including absent SNI). rustls turns a
            // `None` return into an `unrecognized_name` TLS alert.
            _ => None,
        }
    }
}

async fn start_https_backend_wildcard_server(port: u16, cert_path: String, key_path: String) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let server_addr_str = format!("0.0.0.0:{}", port);

    let cert = load_certified_key(&cert_path, &key_path)?;
    let resolver = Arc::new(VhostWildcardResolver { cert });

    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);

    // Advertise HTTP/1.1 only; this test verifies Host/SNI dispatch, not protocol version.
    tls_config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let tls_config = RustlsConfig::from_config(Arc::new(tls_config));
    let app = create_echo_router(server_addr_str.clone());

    info!(
        "Wildcard HTTPS backend (*.sandbox.internal) listening on https://{}",
        addr
    );
    info!("  Certificate: {}", cert_path);

    let app_with_connect_info = app.into_make_service_with_connect_info::<SocketAddr>();
    axum_server::bind_rustls(addr, tls_config)
        .serve(app_with_connect_info)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth2_fixture_accepts_only_expected_grant_scope_and_bootstrap_pairs() {
        let active = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD
                .encode(format!("{OAUTH2_ACTIVE_CLIENT_ID}:{OAUTH2_ACTIVE_CLIENT_SECRET}"))
        );
        let valid_form = b"grant_type=client_credentials&scope=inference.invoke";
        assert_eq!(oauth2_fixture_client(Some(&active), valid_form), Some("active"));
        assert_eq!(oauth2_fixture_client(Some("Basic aW52YWxpZA=="), valid_form), None);
        assert_eq!(
            oauth2_fixture_client(Some(&active), b"grant_type=client_credentials&scope=unexpected"),
            None
        );
    }

    #[test]
    fn dns_query_defaults_to_loopback_when_no_ip() {
        let (host, ips) = parse_dns_query("host=backend.test");
        assert_eq!(host, "backend.test");
        assert_eq!(ips, vec!["127.0.0.1".to_string()]);
    }

    #[test]
    fn dns_query_parses_explicit_single_and_multi_ip() {
        let (host, ips) = parse_dns_query("host=a.test&ip=10.0.0.1");
        assert_eq!(host, "a.test");
        assert_eq!(ips, vec!["10.0.0.1".to_string()]);

        let (_, ips) = parse_dns_query("host=b.test&ip=10.0.0.1,10.0.0.2");
        assert_eq!(ips, vec!["10.0.0.1".to_string(), "10.0.0.2".to_string()]);
    }

    #[test]
    fn dns_query_tolerates_missing_host_and_empty_query() {
        let (host, ips) = parse_dns_query("");
        assert_eq!(host, "");
        assert_eq!(ips, vec!["127.0.0.1".to_string()]);
    }

    #[test]
    fn dns_query_resolves_pod_token_from_explicit_input() {
        let (_, ips) = parse_dns_query("host=k8s.test&ip=pod");
        assert_eq!(
            resolve_dns_answer_tokens(ips, Some("10.0.0.42")),
            vec!["10.0.0.42".to_string()]
        );
    }

    #[test]
    fn ai_provider_observation_never_contains_credential_material() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer ai-provider-test-key".parse().unwrap(),
        );
        headers.insert(axum::http::header::HOST, "127.0.0.1:30001".parse().unwrap());
        let query = AiProviderQuery {
            mock_mode: None,
            fixture_query: Some("preserved".to_string()),
        };
        let body = br#"{"model":"fixture-model","stream":true,"stream_options":{"include_usage":true}}"#;
        let observation = ai_provider_observation("prefix/v1/chat/completions", &query, &headers, body);
        let encoded = serde_json::to_string(&observation).unwrap();

        assert!(observation.auth_is_provider_key);
        assert!(observation.host_is_expected);
        assert!(observation.model_is_canonical);
        assert!(observation.include_usage);
        assert_eq!(observation.credential_generation, "a");
        assert!(!encoded.contains(AI_PROVIDER_KEY_A));
        assert!(!encoded.contains(AI_PROVIDER_KEY_B));
        assert!(!encoded.contains("authorization"));

        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {AI_PROVIDER_KEY_B}").parse().unwrap(),
        );
        let rotated = ai_provider_observation("prefix/v1/chat/completions", &query, &headers, body);
        let rotated_encoded = serde_json::to_string(&rotated).unwrap();
        assert!(rotated.auth_is_provider_key);
        assert_eq!(rotated.credential_generation, "b");
        assert!(!rotated_encoded.contains(AI_PROVIDER_KEY_A));
        assert!(!rotated_encoded.contains(AI_PROVIDER_KEY_B));

        headers.insert(
            axum::http::header::HOST,
            "edgion-test-server.edgion-test.svc.cluster.local:30026"
                .parse()
                .unwrap(),
        );
        assert!(ai_provider_observation("prefix/v1/chat/completions", &query, &headers, body).host_is_expected);
        headers.insert(axum::http::header::HOST, "unexpected.example.com".parse().unwrap());
        assert!(!ai_provider_observation("prefix/v1/chat/completions", &query, &headers, body).host_is_expected);
    }

    #[test]
    fn anthropic_provider_observation_accepts_kubernetes_service_authority() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::HOST,
            "edgion-test-server.edgion-test.svc.cluster.local:30001"
                .parse()
                .unwrap(),
        );
        let uri = "/prefix/v1/messages".parse().unwrap();
        let body = serde_json::json!({
            "model": "claude-fixture",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "fixture"}]
        });
        assert!(anthropic_provider_observation("prefix/v1/messages", &uri, &headers, &body).host_valid);

        headers.insert(axum::http::header::HOST, "unexpected.example.com".parse().unwrap());
        assert!(!anthropic_provider_observation("prefix/v1/messages", &uri, &headers, &body).host_valid);

        headers.remove(axum::http::header::HOST);
        let h2_uri = "https://127.0.0.1:30001/prefix/v1/messages".parse().unwrap();
        assert!(anthropic_provider_observation("prefix/v1/messages", &h2_uri, &headers, &body).host_valid);

        let invalid_h2_uri = "https://unexpected.example.com/prefix/v1/messages".parse().unwrap();
        assert!(!anthropic_provider_observation("prefix/v1/messages", &invalid_h2_uri, &headers, &body).host_valid);
    }

    #[test]
    fn guardrail_observation_retains_only_bounded_structure() {
        let action = "unit-structure-only";
        let input = serde_json::json!({
            "version": "v1",
            "windows": [{
                "choiceIndex": 0,
                "field": {"type": "content"},
                "context": "sensitive-context-marker",
                "current": "sensitive-current-marker"
            }]
        });
        record_guardrail_observation(action, &input);
        let observation = GUARDRAIL_OBSERVATIONS.lock().remove(action).unwrap();
        assert_eq!(observation.calls, 1);
        assert_eq!(observation.total_windows, 1);
        assert!(observation.version_v1);
        assert!(observation.minimal_envelope);
        assert!(observation.sorted_unique_keys);
        assert!(observation.canonical_content_window);
        let encoded = serde_json::to_string(&observation).unwrap();
        assert!(!encoded.contains("sensitive-context-marker"));
        assert!(!encoded.contains("sensitive-current-marker"));
    }

    #[test]
    fn ai_stream_modes_have_deterministic_bounded_contracts() {
        let observation = "{\"fixture_id\":\"test\"}";

        let paced = ai_stream_plan("paced", observation);
        assert_eq!(paced.chunks.len(), 4);
        assert_eq!(paced.interval, Duration::from_millis(250));
        assert!(paced.chunks[2].windows(7).any(|window| window == b"\"usage\""));
        assert_eq!(paced.chunks[3], Bytes::from_static(b"data: [DONE]\n\n"));

        let split = ai_stream_plan("split", observation);
        assert!(split.chunks.len() > 4);
        assert!(split.chunks.iter().all(|chunk| chunk.len() <= 7));
        let split_joined = split.chunks.concat();
        assert!(split_joined.windows(7).any(|window| window == b"\"usage\""));
        assert!(split_joined.ends_with(b"data: [DONE]\n\n"));

        let guardrail_split = ai_stream_plan("guardrail_split", observation);
        assert!(guardrail_split.chunks.len() > 3);
        assert!(guardrail_split.chunks.iter().all(|chunk| chunk.len() <= 7));
        assert!(guardrail_split
            .chunks
            .concat()
            .windows(b"\"index\":0".len())
            .any(|window| window == b"\"index\":0"));

        let two_windows = ai_stream_plan("guardrail_two_windows", observation);
        assert_eq!(two_windows.chunks.len(), 4);
        assert_eq!(
            two_windows
                .chunks
                .concat()
                .windows(b"\"index\":0".len())
                .filter(|window| *window == b"\"index\":0")
                .count(),
            2
        );

        let missing = ai_stream_plan("missing_usage", observation).chunks.concat();
        assert!(!missing.windows(7).any(|window| window == b"\"usage\""));
        assert!(missing.ends_with(b"data: [DONE]\n\n"));

        let oversized = ai_stream_plan("oversized", observation);
        assert!(oversized.chunks[0].len() > edgion_gateway::sse::DEFAULT_MAX_PENDING_EVENT_BYTES);
        assert!(oversized.chunks[0].ends_with(b"\n\n"));
        assert_eq!(oversized.chunks.len(), 3);

        let resilience_abort = ai_stream_plan("resilience_abort", observation);
        assert_eq!(resilience_abort.chunks.len(), 3);
        assert!(resilience_abort.abort_after_chunks);

        let quota_malformed = ai_stream_plan("quota_malformed", observation);
        assert_eq!(quota_malformed.chunks.len(), 4);
        assert_eq!(quota_malformed.chunks[0], Bytes::from_static(b"data: \xff\n\n"));
        assert!(quota_malformed.chunks[1]
            .windows(b"valid-middle".len())
            .any(|window| window == b"valid-middle"));
        assert_eq!(
            quota_malformed.chunks[2],
            Bytes::from_static(b"data: \xfe\n\ndata: quota-malformed-boundary-probe")
        );
        assert!(quota_malformed.chunks[3]
            .windows(b"quota-malformed-must-not-reach-client".len())
            .any(|window| window == b"quota-malformed-must-not-reach-client"));
        assert!(quota_malformed.chunks[3].ends_with(b"data: [DONE]\n\n"));
        assert_eq!(quota_malformed.interval, Duration::from_secs(1));
        assert!(!quota_malformed.abort_after_chunks);

        let long = ai_stream_plan("long", observation);
        assert!(long.chunks.len() > 100);
        assert!(long.interval >= Duration::from_millis(100));
        assert_eq!(long.chunks.last().unwrap(), &Bytes::from_static(b"data: [DONE]\n\n"));

        let quota_terminate = ai_stream_plan("quota_terminate", observation);
        assert_eq!(quota_terminate.chunks.len(), 7, "5 content deltas + usage + [DONE]");
        assert!(quota_terminate.interval >= Duration::from_millis(100));
        assert!(!quota_terminate.abort_after_chunks);
        for content_chunk in &quota_terminate.chunks[..5] {
            assert!(content_chunk.windows(7).any(|window| window == b"\"delta\""));
        }
        let tail = quota_terminate.chunks[5].clone();
        assert!(tail.windows(19).any(|window| window == b"unreachable_marker\""));
        assert_eq!(quota_terminate.chunks[6], Bytes::from_static(b"data: [DONE]\n\n"));

        // Same bytes, one per write: the byte-at-a-time mode must stay a pure
        // re-segmentation of the paced plan, or the client-side assertions
        // that mirror the `quota_terminate` fixture would silently stop
        // applying to it.
        let byte_at_a_time = ai_stream_plan("quota_terminate_byte_at_a_time", observation);
        assert!(byte_at_a_time.chunks.iter().all(|chunk| chunk.len() == 1));
        assert_eq!(
            byte_at_a_time.chunks.concat(),
            quota_terminate.chunks.concat(),
            "byte-at-a-time delivery must not change a single byte of the fixture body"
        );
        assert!(byte_at_a_time.interval <= Duration::from_millis(5));
        assert!(!byte_at_a_time.abort_after_chunks);
    }
}
