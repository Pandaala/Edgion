// pub(crate): also used by the fed_client watch check.
pub(crate) mod authz_classifier;
// pub(crate): authz_layer and inject_center_identity are used at the federation spawn site.
mod access_handlers;
pub(crate) mod authz_middleware;
mod bounded_json;
pub(crate) mod cli_token_middleware;
mod cluster_handlers;
mod cluster_region_setting_handlers;
mod common;
mod configserver_handlers;
mod count_handlers;
mod debug_handlers;
mod diagnostics_handlers;
pub(crate) mod ip_allowlist;
mod namespaced_handlers;
mod public_paths;
mod region_route_handlers;
mod resource_locks;
mod types;
mod validate_handlers;
mod wipe_handlers;

pub use types::{ApiResponse, ListResponse};

use crate::conf_mgr::{ConfMgr, SchemaValidator};
use axum::{
    extract::{Extension, Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post, put},
    Router,
};
use edgion_resources::auth_dto::{AuthStatus, AuthStatusResponse};
use serde::Serialize;
use std::sync::Arc;
use types::AdminState;

/// Inject `Cache-Control: no-store` on every admin API response.
///
/// Private caches (browsers, local proxies) are NOT bound by RFC 9111 §3.5's
/// prohibition on storing responses with an `Authorization` header, so the
/// server must make its intent explicit. Applied as the outermost layer of the
/// Admin API so it fires unconditionally on every response path.
async fn cache_control_no_store_middleware(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Build the `/api/v1/auth/status` response from the `Authorizer`.
///
/// The controller authenticates with a single pre-shared bearer token (Istio
/// style), so `auth_required` is always true and `mode` is `"token"`. `status`
/// is `Ready` once at least one token (the local admin token or a Center cli
/// token) is registered, else `Initializing`.
fn build_token_auth_status(authorizer: &crate::authz::Authorizer) -> AuthStatusResponse {
    let status = if authorizer.cli_tokens_present() {
        AuthStatus::Ready
    } else {
        AuthStatus::Initializing
    };
    AuthStatusResponse {
        auth_required: true,
        mode: "token".to_string(),
        status,
    }
}

/// Public `GET /api/v1/auth/status` handler (token-only model).
async fn auth_status_handler(
    State(authorizer): State<Arc<crate::authz::Authorizer>>,
) -> Json<ApiResponse<AuthStatusResponse>> {
    Json(ApiResponse::ok_body(build_token_auth_status(&authorizer)))
}

/// Build the controller-side public status route.
fn auth_status_route(authorizer: Arc<crate::authz::Authorizer>) -> Router {
    Router::new()
        .route("/api/v1/auth/status", get(auth_status_handler))
        .with_state(authorizer)
}

/// Fail-closed token gate for the Admin API.
///
/// When no token (admin or Center) is registered, business routes return 503 +
/// WARN so the operator sees "configure a token" rather than a bare 401. This
/// preserves the documented "no auto-generation → fail-close, server still
/// boots" posture.
///
/// Mounted on the Admin API app ONLY — never inside `create_admin_router`, which
/// is shared with the federation router (whose Center identity does not come
/// from the token map; gating it there would 503 Center sync). It runs before
/// the token middleware and exempts the static public paths so
/// `/api/v1/auth/status` and the probes stay reachable to report `Initializing`.
async fn require_token_configured_gate(
    State(authorizer): State<Arc<crate::authz::Authorizer>>,
    request: Request,
    next: Next,
) -> Response {
    if crate::api::public_paths::is_static_public(request.uri().path()) {
        return next.run(request).await;
    }
    if !authorizer.cli_tokens_present() {
        tracing::warn!(
            component = "admin_api",
            "no admin/cli token configured; Admin API business routes fail closed with 503 \
             until EDGION_ADMIN_TOKEN (or a cli_token) is set"
        );
        let body = Json(ApiResponse::<()>::err_body(
            "admin API auth not configured: set EDGION_ADMIN_TOKEN or a cli_token".to_string(),
        ));
        return (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
    }
    next.run(request).await
}

/// Reject admin write requests with `503 Service Unavailable` while the
/// controller is still initializing. Mounted on the write subset of
/// `create_admin_router` (POST/PUT/DELETE on `/api/v1/cluster/...` and
/// `/api/v1/namespaced/...`). Read endpoints, health, and auth routes are
/// intentionally not gated.
///
/// Includes `Retry-After: 30` — controller initialization typically completes within 30 s.
async fn require_ready_middleware(State(state): State<Arc<AdminState>>, request: Request, next: Next) -> Response {
    if state.is_ready() {
        next.run(request).await
    } else {
        let body = Json(ApiResponse::<()>::err_body(
            "controller initializing, please retry after init completes".to_string(),
        ));
        let mut resp = (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
        resp.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
        resp
    }
}

/// Spec §6.2: decide whether a request must be rejected with the not-leader 409.
///
/// Pure decision function (unit-testable without axum plumbing):
/// - `NotApplicable` (Kubernetes / FileSystem) and `Leader` pass everything;
/// - `Follower` rejects every non-public path with 409 and a body naming the
///   leader's admin address (`null` while unknown, e.g. mid-election);
/// - the static public set (`/health`, `/ready`, `/metrics`,
///   `/api/v1/auth/status`) always passes so probes keep working on a standby.
fn not_leader_rejection(
    status: &crate::conf_mgr::conf_center::traits::LeadershipStatus,
    path: &str,
) -> Option<(StatusCode, serde_json::Value)> {
    use crate::conf_mgr::conf_center::traits::LeadershipStatus;
    match status {
        LeadershipStatus::NotApplicable | LeadershipStatus::Leader => None,
        LeadershipStatus::Follower { leader_admin_addr } => {
            if public_paths::is_static_public(path) {
                return None;
            }
            Some((
                StatusCode::CONFLICT,
                serde_json::json!({
                    "error": "not leader",
                    "leader": leader_admin_addr,
                }),
            ))
        }
    }
}

/// Axum wrapper for [`not_leader_rejection`]. Lives INSIDE `create_admin_router`
/// so the federation reverse-tunnel router is gated too (spec §6.2) — the
/// fail-close onion in `serve_admin_api_with_shutdown` deliberately is not.
async fn leadership_gate_middleware(State(state): State<Arc<AdminState>>, request: Request, next: Next) -> Response {
    let status = state.conf_mgr.leadership_status();
    match not_leader_rejection(&status, request.uri().path()) {
        Some((code, body)) => (code, Json(body)).into_response(),
        None => next.run(request).await,
    }
}

/// Deadman flag: set (sticky) when the control-plane lifecycle task ends for
/// any reason other than an orderly shutdown — including a lifecycle panic.
/// `/health` returns 503 while set, so the liveness probe finally has a
/// failure mode ("a liveness probe that cannot fail is not a liveness probe").
/// The lifecycle is spawned exactly once per process (cli step 5), so there is
/// no legitimate reset path outside tests.
static LIFECYCLE_TERMINATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Mark the control-plane lifecycle as abnormally terminated (see
/// `LIFECYCLE_TERMINATED`). Called from the lifecycle spawn wrapper in cli.
pub fn mark_lifecycle_terminated() {
    LIFECYCLE_TERMINATED.store(true, std::sync::atomic::Ordering::Release);
}

fn lifecycle_terminated() -> bool {
    LIFECYCLE_TERMINATED.load(std::sync::atomic::Ordering::Acquire)
}

// ============= Handlers =============

/// API request body for manual conf_sync rotation (Task 11).
#[derive(serde::Deserialize)]
struct RotateBody {
    #[serde(default = "default_rotate_scope")]
    scope: ApiRotateScope,
}

fn default_rotate_scope() -> ApiRotateScope {
    ApiRotateScope::Leaf
}

/// Scope discriminator for manual rotation (API representation).
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum ApiRotateScope {
    Leaf,
    Ca,
}

/// Health check endpoint. 200 while the process is healthy; 503 once the
/// control-plane lifecycle has terminated abnormally (deadman — the liveness
/// probe's failure mode of last resort).
async fn health_check() -> Response {
    if lifecycle_terminated() {
        let body = Json(ApiResponse::<()>::err_body(
            "control-plane lifecycle terminated".to_string(),
        ));
        (StatusCode::SERVICE_UNAVAILABLE, body).into_response()
    } else {
        Json(ApiResponse::ok_body("OK".to_string())).into_response()
    }
}

/// Readiness check endpoint - returns OK only when ConfigServer is ready
async fn readiness_check(State(state): State<Arc<AdminState>>) -> Result<Json<ApiResponse<String>>, StatusCode> {
    if state.is_ready() {
        Ok(Json(ApiResponse::ok_body("Ready".to_string())))
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

/// Server info response
#[derive(Serialize)]
struct ServerInfoResponse {
    server_id: String,
    ready: bool,
    mode: String,
}

/// Get server info including server_id
///
/// This endpoint returns the current serving generation and readiness.
async fn get_server_info(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<ApiResponse<ServerInfoResponse>>, StatusCode> {
    match state.config_sync_server() {
        Ok(server) => {
            let info = ServerInfoResponse {
                server_id: server.server_id(),
                ready: true,
                mode: "controller".to_string(),
            };
            Ok(Json(ApiResponse::ok_body(info)))
        }
        Err(_) => {
            let info = ServerInfoResponse {
                server_id: String::new(),
                ready: false,
                mode: "controller".to_string(),
            };
            Ok(Json(ApiResponse::ok_body(info)))
        }
    }
}

/// Manually trigger ACME certificate issuance or renewal for a specific resource.
///
/// POST /api/v1/services/acme/{namespace}/{name}/trigger
///
/// This resets the retry counter and re-evaluates the resource, same as modifying the CRD.
async fn trigger_acme(Path((namespace, name)): Path<(String, String)>) -> impl IntoResponse {
    let key = format!("{}/{}", namespace, name);
    tracing::info!(
        component = "admin_api",
        event = "acme_trigger",
        key = %key,
        "ACME trigger requested via Admin API"
    );

    // `false` means the ACME service is not running (no sender). Reporting a
    // `200 OK` in that case would mislead the operator into thinking the
    // trigger took effect, so surface it as a 503 instead.
    if crate::services::acme::trigger_resource(key.clone()).await {
        Json(ApiResponse::ok_body(format!("ACME check triggered for {}", key))).into_response()
    } else {
        let body = Json(ApiResponse::<String>::err_body(format!(
            "ACME service not running; trigger for {} was not queued",
            key
        )));
        (StatusCode::SERVICE_UNAVAILABLE, body).into_response()
    }
}

/// Manually trigger conf_sync certificate rotation (leaf or root).
///
/// POST /api/v1/services/conf-sync-ca/rotate
///
/// Request body: `{"scope": "leaf"|"ca"}` (default `leaf`).
/// - 202 Accepted: enqueued on the leader
/// - 409 Conflict: not the leader; retry via edgion-controller-leader Service
/// - 503 Service Unavailable: leader, but rotation service not ready yet
/// - 400 Bad Request: auto_ca disabled; replace files and hot reload applies them
async fn trigger_conf_sync_rotate(
    identity: Option<Extension<crate::api::cli_token_middleware::CliTokenIdentity>>,
    body: Option<Json<RotateBody>>,
) -> impl IntoResponse {
    use crate::services::conf_sync_ca::rotation::{trigger_manual_rotation, RotateScope, TriggerResult};

    let scope = match body.map(|Json(b)| b.scope).unwrap_or(ApiRotateScope::Leaf) {
        ApiRotateScope::Leaf => RotateScope::Leaf,
        ApiRotateScope::Ca => RotateScope::Ca,
    };

    // The cli-token authn middleware (`cli_token_pre_authn`) injects this identity
    // for both the local admin token and scoped `cli_tokens` entries; trivially
    // available here via the same `Extension` mechanism `authz_middleware` uses
    // for its deny-audit log, so include it for this security-relevant trigger too.
    let requester = identity
        .as_ref()
        .map(|Extension(id)| id.name.as_str())
        .unwrap_or("unknown");

    tracing::info!(
        component = "admin_api",
        event = "conf_sync_rotate_trigger",
        scope = ?scope,
        requester = %requester,
        "manual rotation requested"
    );

    match trigger_manual_rotation(scope).await {
        TriggerResult::Accepted => (StatusCode::ACCEPTED, Json(serde_json::json!({"status": "accepted"}))),
        TriggerResult::NotLeader => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "not leader",
                "hint": "retry via the edgion-controller-leader Service (admin HTTP port)"
            })),
        ),
        TriggerResult::LeaderNotReady => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "rotation service not ready on the leader; retry shortly"})),
        ),
        TriggerResult::NotConfigured => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "auto_ca disabled: rotation is externally managed; replace the cert files (hot reload applies them)"
            })),
        ),
    }
}

// ============= Router Setup =============

/// Build the shared admin state. Used by the admin router and the dedicated
/// probe listener (readiness reads the same `conf_mgr`-backed state).
pub fn build_admin_state(conf_mgr: Arc<ConfMgr>, schema_validator: Arc<SchemaValidator>) -> Arc<AdminState> {
    Arc::new(AdminState {
        conf_mgr,
        schema_validator,
    })
}

/// Dedicated liveness/readiness router (own listener, no auth, no business routes).
pub fn create_probe_router(state: Arc<AdminState>) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/ready", get(readiness_check))
        .with_state(state)
}

/// Dedicated Prometheus metrics router. Mounts the Controller-specific handler
/// (`status_metrics::metrics_handler`) so the `resource_abnormal` block is preserved.
pub fn create_metrics_router() -> Router {
    Router::new().route("/metrics", get(crate::observe::status_metrics::metrics_handler))
}

/// Create the admin API router with unified K8s-style endpoints
pub fn create_admin_router(conf_mgr: Arc<ConfMgr>, schema_validator: Arc<SchemaValidator>) -> Router {
    let admin_state = build_admin_state(conf_mgr, schema_validator);

    // Write routes — gated by readiness middleware so requests arriving during
    // controller initialization are rejected with 503 instead of racing the
    // processor LIST + cache build.
    let write_routes = Router::new()
        .route("/api/v1/cluster/{kind}", post(cluster_handlers::create_cluster))
        .route("/api/v1/cluster/{kind}/{name}", put(cluster_handlers::update_cluster))
        .route(
            "/api/v1/cluster/{kind}/{name}",
            delete(cluster_handlers::delete_cluster),
        )
        .route(
            "/api/v1/namespaced/{kind}/{namespace}",
            post(namespaced_handlers::create_namespaced),
        )
        .route(
            "/api/v1/namespaced/{kind}/{namespace}/{name}",
            put(namespaced_handlers::update_namespaced),
        )
        .route(
            "/api/v1/namespaced/{kind}/{namespace}/{name}",
            delete(namespaced_handlers::delete_namespaced),
        )
        .route(
            "/api/v1/cluster-region-settings/{namespace}/{name}/failover",
            post(region_route_handlers::apply_setting_failover),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            admin_state.clone(),
            require_ready_middleware,
        ));

    let mut router = Router::new()
        // Cross-namespace query - List all resources of a kind
        .route(
            "/api/v1/namespaced/{kind}",
            get(namespaced_handlers::list_all_namespaces),
        )
        // Cluster-scoped read routes (writes live in `write_routes` below)
        .route("/api/v1/cluster/{kind}", get(cluster_handlers::list_cluster))
        .route("/api/v1/cluster/{kind}/{name}", get(cluster_handlers::get_cluster))
        // Namespace-scoped read routes (writes live in `write_routes` below)
        .route(
            "/api/v1/namespaced/{kind}/{namespace}",
            get(namespaced_handlers::list_namespaced),
        )
        .route(
            "/api/v1/namespaced/{kind}/{namespace}/{name}",
            get(namespaced_handlers::get_namespaced),
        )
        // Metadata-only keys list routes (Card 01 of list-keys-api series)
        .route("/api/v1/count/namespaced/{kind}", get(count_handlers::namespaced))
        .route(
            "/api/v1/count/namespaced/{kind}/{namespace}",
            get(count_handlers::namespace),
        )
        .route("/api/v1/count/cluster/{kind}", get(count_handlers::cluster))
        .route("/api/v1/keys/cluster/{kind}", get(cluster_handlers::list_cluster_keys))
        .route(
            "/api/v1/keys/namespaced/{kind}",
            get(namespaced_handlers::list_all_namespaces_keys),
        )
        .route(
            "/api/v1/keys/namespaced/{kind}/{namespace}",
            get(namespaced_handlers::list_namespaced_keys),
        )
        .merge(write_routes)
        // Special operations
        .route("/api/v1/access", get(access_handlers::get_access))
        .route("/api/v1/server-info", get(get_server_info))
        // Effective resolved views for Center dashboard polling.
        .route(
            "/api/v1/cluster-region-settings/keys",
            get(cluster_region_setting_handlers::keys),
        )
        .route(
            "/api/v1/cluster-region-settings/{namespace}/{name}",
            get(cluster_region_setting_handlers::detail),
        )
        .route(
            "/api/v1/region-routes/effective",
            get(region_route_handlers::list_region_routes_effective),
        )
        .route(
            "/api/v1/cluster-region-settings/{namespace}/{name}/consumers",
            get(region_route_handlers::list_setting_consumers),
        )
        .route(
            "/api/v1/cluster-region-settings/{namespace}/{name}/failover/preview",
            post(region_route_handlers::preview_setting_failover),
        )
        // Services: ACME manual trigger
        .route("/api/v1/services/acme/{namespace}/{name}/trigger", post(trigger_acme))
        // Services: conf_sync rotation manual trigger
        .route("/api/v1/services/conf-sync-ca/rotate", post(trigger_conf_sync_rotate))
        // Diagnostics: roadmap data conditions (e.g. ExtensionRef → Envoy alignment)
        .route(
            "/api/v1/diagnostics/nested_ext_ref_usage",
            get(diagnostics_handlers::nested_ext_ref_usage),
        )
        .route(
            "/api/v1/diagnostics/conf-conflicts",
            get(diagnostics_handlers::conf_conflicts),
        )
        .route(
            "/api/v1/diagnostics/unknown-fields",
            get(diagnostics_handlers::unknown_fields),
        )
        // Read-only validation verdict on a submitted (not-yet-applied) document:
        // the building block for an operator's own blocking gate, since the
        // controller itself only ever warns.
        .route("/api/v1/validate/{kind}", post(validate_handlers::validate_kind))
        // ConfigServer endpoints (for edgion-cli --target server)
        .route("/configserver/{kind}/list", get(configserver_handlers::list_resources))
        .route("/configserver/{kind}", get(configserver_handlers::get_resource))
        .with_state(admin_state.clone());

    if edgion_common::test_mode::is_test_mode() {
        router = router
            .route(
                "/api/v1/debug/store-stats",
                get(debug_handlers::store_stats).with_state(admin_state.clone()),
            )
            .route(
                "/api/v1/admin/wipe-all",
                post(wipe_handlers::wipe_all).with_state(admin_state.clone()),
            );
    }
    // Spec §6.2: the not-leader 409 gate wraps the WHOLE router (business +
    // test-mode routes) and lives here — not in `serve_admin_api_with_shutdown`
    // — so the federation reverse-tunnel router built from this same function is
    // gated too. `NotApplicable` (K8s / FileSystem) makes it a pass-through.
    router.layer(axum::middleware::from_fn_with_state(
        admin_state,
        leadership_gate_middleware,
    ))
}

/// Serve the admin API with graceful shutdown support.
///
/// Credentials come only from configuration (resolved into `local_auth_config` /
/// `auth_config` upstream); they are never auto-generated. When neither is present,
/// no provider loads and the Admin API returns 503 fail-close.
#[allow(clippy::too_many_arguments)]
pub async fn serve_with_shutdown(
    conf_mgr: Arc<ConfMgr>,
    schema_validator: Arc<SchemaValidator>,
    addr: std::net::SocketAddr,
    authorizer: Arc<crate::authz::Authorizer>,
    allow_admin_ips: Vec<String>,
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let base_router = create_admin_router(conf_mgr.clone(), schema_validator);

    // Enforce authz on the business routes (route_layer: only matched routes,
    // never the fallback). A request reaches a handler only with a `Role`
    // (injected by the token middleware below); `authz_layer` 403s a missing
    // role and skips the static public paths.
    let business = base_router.route_layer(axum::middleware::from_fn_with_state(
        authorizer.clone(),
        authz_middleware::authz_layer,
    ));

    // Mount the public status route (no authz_layer — it is merged AFTER the
    // route_layer, so it stays reachable) and compose the Admin API onion. Layer
    // order (outer → inner), achieved via axum's LIFO `.layer()`:
    //   IP allowlist → 503 token gate → token authn → [authz_layer] → handler
    //   (cache-control wraps the lot so every response carries `no-store`).
    let app = business
        .merge(auth_status_route(authorizer.clone()))
        // Token authn: a recognized Bearer token injects `Role::Restricted`.
        .layer(axum::middleware::from_fn_with_state(
            authorizer.clone(),
            cli_token_middleware::cli_token_pre_authn,
        ))
        // 503 fail-close gate: runs BEFORE the token middleware (outer) so a
        // tokenless business request on a controller with no tokens configured
        // yields 503, not 401. Admin-app only — never on the shared fed router.
        .layer(axum::middleware::from_fn_with_state(
            authorizer,
            require_token_configured_gate,
        ))
        .layer(axum::middleware::from_fn(cache_control_no_store_middleware));

    // admin-api-02: outermost IP allowlist — rejects unauthorized peers before auth.
    // Not mounted when the allowlist is empty (allow-all, backward compatible).
    let app = match ip_allowlist::build_admin_ip_matcher(&allow_admin_ips)? {
        Some(m) => {
            tracing::info!(
                component = "admin_api",
                entries = allow_admin_ips.len(),
                "admin IP allowlist active"
            );
            app.layer(axum::middleware::from_fn_with_state(
                m,
                ip_allowlist::ip_allowlist_middleware,
            ))
        }
        None => {
            tracing::info!(component = "admin_api", "admin IP allowlist inactive (allow all)");
            app
        }
    };

    // Transport is always plaintext HTTP — there is no admin TLS listener. Network-level
    // controls (private bind, `allow_admin_ips`, K8s ClusterIP/NetworkPolicy) plus Bearer-token
    // auth guard this endpoint; make the plaintext nature explicit at startup.
    warn_if_admin_exposure_insecure(&addr, &allow_admin_ips);
    tracing::info!(
        component = "admin_api",
        addr = %addr,
        scheme = "http",
        "Controller Admin API server listening (plaintext HTTP) with unified K8s-style routes"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal)
    .await?;

    tracing::info!(component = "admin_api", "Admin API server stopped");
    Ok(())
}

/// True when the Admin API is bound to a non-loopback address with no IP
/// allowlist. The admin transport is always plaintext, so the "no TLS"
/// sub-condition is constant-true and folded in implicitly.
///
/// `to_canonical()` is used so that an IPv4-mapped IPv6 loopback address
/// (`::ffff:127.0.0.1`) is correctly treated as safe — matching the same
/// pattern used in `ip_allowlist.rs`.
fn admin_exposure_is_insecure(addr: &std::net::SocketAddr, allow_admin_ips: &[String]) -> bool {
    !addr.ip().to_canonical().is_loopback() && allow_admin_ips.is_empty()
}

/// Observability-only: when the admin plane is insecurely exposed, emit a
/// high-severity WARN and set the `edgion_system_error_state` gauge. Never
/// blocks startup.
fn warn_if_admin_exposure_insecure(addr: &std::net::SocketAddr, allow_admin_ips: &[String]) {
    if !admin_exposure_is_insecure(addr, allow_admin_ips) {
        return;
    }
    tracing::warn!(
        component = "admin_api",
        addr = %addr,
        scheme = "http",
        allow_admin_ips_configured = false,
        "SECURITY: Admin API bound to a non-loopback address with no IP allowlist and no TLS — \
         the privileged admin plane is reachable on every routable interface in plaintext. \
         Bind to 127.0.0.1, set allow_admin_ips, or restrict the admin port with a NetworkPolicy."
    );
    edgion_common::observe::system_metrics::mark_system_error(
        edgion_common::observe::system_metrics::components::ADMIN_API,
        edgion_common::observe::system_metrics::reasons::INSECURE_ADMIN_EXPOSURE,
    );
}

#[cfg(test)]
#[path = "../../tests/unit/api/mod_test/auth_status_tests.rs"]
mod auth_status_tests;

#[cfg(test)]
#[path = "../../tests/unit/api/mod_test/admin_exposure_tests.rs"]
mod admin_exposure_tests;

#[cfg(test)]
#[path = "../../tests/unit/api/mod_test/deadman_tests.rs"]
mod deadman_tests;

#[cfg(test)]
#[path = "../../tests/unit/api/mod_test/rotate_scope_tests.rs"]
mod rotate_scope_tests;

#[cfg(test)]
#[path = "../../tests/unit/api/mod_test/leadership_gate_tests.rs"]
mod leadership_gate_tests;

#[cfg(test)]
#[path = "../../tests/support/api/mod.rs"]
mod test_support;
#[cfg(test)]
pub(crate) use test_support::reset_lifecycle_terminated_for_test;
