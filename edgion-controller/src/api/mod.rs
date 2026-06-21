// pub(crate): also used by the fed_client watch check.
pub(crate) mod authz_classifier;
// pub(crate): authz_layer and inject_center_identity are used at the federation spawn site.
pub(crate) mod authz_middleware;
pub(crate) mod cli_token_middleware;
mod cluster_handlers;
mod common;
mod configserver_handlers;
mod debug_handlers;
mod diagnostics_handlers;
mod effective_handlers;
pub(crate) mod ip_allowlist;
mod namespaced_handlers;
mod public_paths;
mod region_route_handlers;
mod resource_locks;
mod types;
mod wipe_handlers;

pub use types::{ApiResponse, ListResponse};

use crate::conf_mgr::{ConfMgr, SchemaValidator};
use axum::{
    extract::{Path, Request, State},
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
/// controller is still in init / reload. Mounted on the write subset of
/// `create_admin_router` (POST/PUT/DELETE on `/api/v1/cluster/...` and
/// `/api/v1/namespaced/...`). Read endpoints, health, and auth routes are
/// intentionally not gated.
///
/// Includes `Retry-After: 30` — controller init / reload typically completes within 30 s.
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

// ============= Handlers =============

/// Health check endpoint - always returns OK if server is up
async fn health_check() -> Json<ApiResponse<String>> {
    Json(ApiResponse::ok_body("OK".to_string()))
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
/// This endpoint returns the current server_id which changes on reload.
/// Useful for verifying that reload has taken effect.
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

/// Re-read `[center]` and `cli_tokens` from the config file on disk, atomically
/// swap the authz policy + cli_tokens map, and notify the federation supervisor.
/// Called from `reload_all_resources` BEFORE `request_reload()` so the
/// newer/stricter policy is live before any reloaded handler runs.
///
/// On load error: logs a warning and keeps the current policy -- on load error
/// we keep the current policy and still proceed to `request_reload()`. Note that
/// the current policy might already be deny-all, so this is NOT fail-open.
///
/// `cli_tokens` rebuild is NOT gated on `[center]` presence: cli tokens are
/// independent of the federation center, so a center-less deployment can still
/// hot-reload them. When `[center]` is absent the center policy defaults to
/// deny-all via `authorizer.apply(deny_all, cli_map)`.
///
/// Factored out for unit-testability; the handler calls this and then
/// `request_reload()`.
fn apply_center_reload(state: &AdminState) {
    let path = match &state.config_file {
        Some(p) => p.clone(),
        None => return,
    };

    let cli_cfg = crate::config::EdgionControllerConfig {
        config_file: Some(path.clone()),
        ..Default::default()
    };

    let full = match crate::config::EdgionControllerConfig::load(cli_cfg) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                component = "admin_api",
                path = %path,
                error = %e,
                "reload: failed to re-read config; keeping current policy"
            );
            return;
        }
    };

    // --- 1) Resolve new center policy (if [center] is present) ---
    // When [center] is absent the center policy falls back to deny-all.
    // The center_reload_tx is only notified when [center] is explicitly present,
    // so the supervisor is not disturbed by center-less configs.
    let (new_center_policy, center_for_supervisor) = if let Some(center) = full.center {
        if let Some(rbac) = &center.rbac {
            if let Err(e) = rbac.validate() {
                tracing::warn!(
                    component = "fed_rbac",
                    error = %e,
                    "invalid center.rbac rule; it will not match any request"
                );
            }
            // Lint the operator-supplied center RBAC policy (the built-in default is
            // known-good and does not need linting).
            crate::authz::lint::lint_rbac_policy(rbac, "reload", "center");
        }
        let policy = crate::fed_sync::default_policy::center_policy_from(center.rbac.as_ref());
        (policy, Some(center))
    } else {
        (crate::authz::CenterPolicy::deny_all(), None)
    };

    // --- 2) Rebuild cli_tokens map ---
    // Uses the shared lint pass: per-entry shape validation, GLOBAL dedup
    // (no silent last-wins on duplicate name/hash), advisory WARNs for
    // unknown/non-canonical kinds, unknown verbs, and dangerous grants.
    // Map key = token_sha256 VERBATIM (already a hash; do NOT re-hash).
    let cli_map = crate::authz::lint::build_cli_token_map(&full.cli_tokens, "reload");

    // --- 3) Atomically apply center policy + cli_tokens in a single write ---
    // This is the SINGLE writer on the reload path (supervisor reads channel for
    // lifecycle only; it does NOT call swap_policy separately).
    state.authorizer.apply(new_center_policy, cli_map);

    // --- 4) Notify supervisor when [center] is present ---
    if let Some(center) = center_for_supervisor {
        let _ = state.center_reload_tx.send(center);
    }
}

/// Reload all resources - triggers a full restart of the configuration center
///
/// This endpoint triggers a full reload:
/// 1. Stop current controllers
/// 2. Clear PROCESSOR_REGISTRY
/// 3. Create new ConfigSyncServer (new server_id)
/// 4. Restart controllers (full Init -> InitApply -> InitDone flow)
///
/// Gateway clients will detect the server_id change and re-list all resources.
///
/// Returns `Retry-After: 5` on 503 — a reload already in progress is a transient condition.
async fn reload_all_resources(State(state): State<Arc<AdminState>>) -> impl IntoResponse {
    tracing::info!(
        component = "admin_api",
        event = "reload_requested",
        "Reload requested via Admin API"
    );

    // Re-read [center] from disk and apply it before triggering the async
    // fire-and-forget resource reload, so the newer/stricter policy is live first.
    apply_center_reload(&state);

    match state.conf_mgr.request_reload() {
        Ok(()) => {
            tracing::info!(
                component = "admin_api",
                event = "reload_initiated",
                "Reload initiated successfully"
            );
            Json(types::ApiResponse::ok_body(
                "Reload initiated - controllers will restart with new server_id".to_string(),
            ))
            .into_response()
        }
        Err(e) => {
            tracing::warn!(
                component = "admin_api",
                event = "reload_failed",
                error = %e,
                "Reload request failed"
            );
            let body = Json(ApiResponse::<()>::err_body(format!("reload unavailable: {}", e)));
            let mut resp = (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
            resp
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
    if crate::services::acme::notify_resource_changed(key.clone()).await {
        Json(ApiResponse::ok_body(format!("ACME check triggered for {}", key))).into_response()
    } else {
        let body = Json(ApiResponse::<String>::err_body(format!(
            "ACME service not running; trigger for {} was not queued",
            key
        )));
        (StatusCode::SERVICE_UNAVAILABLE, body).into_response()
    }
}

// ============= Router Setup =============

/// Build the shared admin state. Used by the admin router and the dedicated
/// probe listener (readiness reads the same `conf_mgr`-backed state).
pub fn build_admin_state(
    conf_mgr: Arc<ConfMgr>,
    schema_validator: Arc<SchemaValidator>,
    config_file: Option<String>,
    authorizer: Arc<crate::authz::Authorizer>,
    center_reload_tx: tokio::sync::watch::Sender<crate::config::CenterClientConfig>,
) -> Arc<AdminState> {
    Arc::new(AdminState {
        conf_mgr,
        schema_validator,
        config_file,
        authorizer,
        center_reload_tx,
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
pub fn create_admin_router(
    conf_mgr: Arc<ConfMgr>,
    schema_validator: Arc<SchemaValidator>,
    config_file: Option<String>,
    authorizer: Arc<crate::authz::Authorizer>,
    center_reload_tx: tokio::sync::watch::Sender<crate::config::CenterClientConfig>,
) -> Router {
    let admin_state = build_admin_state(conf_mgr, schema_validator, config_file, authorizer, center_reload_tx);

    // Write routes — gated by readiness middleware so requests arriving during
    // controller init / reload are rejected with 503 instead of racing the
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
        .route("/api/v1/server-info", get(get_server_info))
        .route("/api/v1/reload", post(reload_all_resources))
        // Effective resolved views for Center dashboard polling.
        .route(
            "/api/v1/region-routes/effective",
            get(region_route_handlers::list_region_routes_effective),
        )
        .route(
            "/api/v1/cluster-region-routes",
            get(region_route_handlers::list_cluster_region_edgion_config_data),
        )
        .route(
            "/api/v1/service-region-routes",
            get(region_route_handlers::list_service_region_edgion_config_data),
        )
        .route(
            "/api/v1/global-ip-restrictions/effective",
            get(effective_handlers::global_ip_restrictions_effective),
        )
        // New-URL aliases for RegionRoute failover
        .route(
            "/api/v1/cluster-region-routes/failover",
            post(region_route_handlers::cluster_edgion_config_data_failover),
        )
        .route(
            "/api/v1/service-region-routes/failover",
            post(region_route_handlers::service_edgion_config_data_failover),
        )
        // Services: ACME manual trigger
        .route("/api/v1/services/acme/{namespace}/{name}/trigger", post(trigger_acme))
        // Diagnostics: roadmap data conditions (e.g. ExtensionRef → Envoy alignment)
        .route(
            "/api/v1/diagnostics/nested_ext_ref_usage",
            get(diagnostics_handlers::nested_ext_ref_usage),
        )
        .route(
            "/api/v1/diagnostics/conf-conflicts",
            get(diagnostics_handlers::conf_conflicts),
        )
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
                post(wipe_handlers::wipe_all).with_state(admin_state),
            );
    }
    router
}

/// Serve the admin API on the specified address.
///
/// Thin wrapper over [`serve_with_shutdown`] with a never-resolving shutdown
/// future. Kept as a separate public entry point for callers that don't need
/// graceful shutdown plumbing.
pub async fn serve(
    conf_mgr: Arc<ConfMgr>,
    schema_validator: Arc<SchemaValidator>,
    addr: std::net::SocketAddr,
    config_file: Option<String>,
    authorizer: Arc<crate::authz::Authorizer>,
    center_reload_tx: tokio::sync::watch::Sender<crate::config::CenterClientConfig>,
    allow_admin_ips: Vec<String>,
) -> anyhow::Result<()> {
    serve_with_shutdown(
        conf_mgr,
        schema_validator,
        addr,
        config_file,
        authorizer,
        center_reload_tx,
        allow_admin_ips,
        std::future::pending::<()>(),
    )
    .await
}

/// Serve the admin API with graceful shutdown support.
///
/// Credentials come only from configuration (resolved into `local_auth_config` /
/// `auth_config` upstream); they are never auto-generated. When neither is present,
/// no provider loads and the Admin API returns 503 fail-close.
pub async fn serve_with_shutdown(
    conf_mgr: Arc<ConfMgr>,
    schema_validator: Arc<SchemaValidator>,
    addr: std::net::SocketAddr,
    config_file: Option<String>,
    authorizer: Arc<crate::authz::Authorizer>,
    center_reload_tx: tokio::sync::watch::Sender<crate::config::CenterClientConfig>,
    allow_admin_ips: Vec<String>,
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let base_router = create_admin_router(
        conf_mgr.clone(),
        schema_validator,
        config_file,
        authorizer.clone(),
        center_reload_tx,
    );

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
mod auth_status_tests {
    use super::build_token_auth_status;
    use crate::authz::{sha256_hex, Authorizer, CenterPolicy, CliTokenEntry};
    use edgion_resources::auth_dto::AuthStatus;
    use std::sync::Arc;

    /// No token registered → Initializing, auth always required, mode "token".
    #[test]
    fn status_initializing_without_token() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        let resp = build_token_auth_status(&az);
        assert!(resp.auth_required);
        assert_eq!(resp.mode, "token");
        assert_eq!(resp.status, AuthStatus::Initializing);
    }

    /// At least one token registered → Ready.
    #[test]
    fn status_ready_with_token() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        let mut m = std::collections::HashMap::new();
        m.insert(
            sha256_hex("tok"),
            CliTokenEntry {
                name: "bot".into(),
                policy: Arc::new(CenterPolicy::deny_all()),
            },
        );
        az.apply(CenterPolicy::deny_all(), m);
        let resp = build_token_auth_status(&az);
        assert!(resp.auth_required);
        assert_eq!(resp.status, AuthStatus::Ready);
    }

    /// The local admin token alone makes status Ready (it is merged by `apply`).
    #[test]
    fn status_ready_with_admin_token_only() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        az.set_admin_token(crate::authz::build_admin_token_entry(Some("admin-secret")));
        az.apply(CenterPolicy::deny_all(), std::collections::HashMap::new());
        assert_eq!(build_token_auth_status(&az).status, AuthStatus::Ready);
    }
}

#[cfg(test)]
mod reload_center_tests {
    use super::*;
    use crate::authz::{Authorizer, CenterPolicy, Verb};
    use std::io::Write;
    use std::sync::Arc;

    /// Build a minimal AdminState for reload tests:
    /// - config_file points to a caller-supplied temp path
    /// - Authorizer starts deny-all
    /// - watch channel created fresh
    ///
    /// `apply_center_reload` only touches `config_file`, `authorizer`, and
    /// `center_reload_tx`; the ConfMgr is real but not exercised.
    async fn make_reload_state(
        config_path: &str,
    ) -> (
        Arc<AdminState>,
        tokio::sync::watch::Receiver<crate::config::CenterClientConfig>,
        tempfile::TempDir, // kept alive so its path stays valid
    ) {
        use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

        let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
        let fs_config = FileSystemConfig::new(tmp_data.path().to_path_buf());
        let conf_mgr = Arc::new(
            ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
                .await
                .expect("ConfMgr::create"),
        );
        let schema_validator = Arc::new(SchemaValidator::empty());
        let (tx, rx) = tokio::sync::watch::channel(crate::config::CenterClientConfig::default());
        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let state = Arc::new(AdminState {
            conf_mgr,
            schema_validator,
            config_file: Some(config_path.to_string()),
            authorizer,
            center_reload_tx: tx,
        });
        (state, rx, tmp_data)
    }

    /// Write a minimal controller config YAML that includes a [center] block
    /// with an RBAC rule granting `get` on `EdgionConfigData`.
    fn write_center_config(path: &str) {
        let yaml = r#"
center:
  address: "http://center.example.com:50052"
  name: "ctrl-test"
  cluster: "test-cluster"
  enabled: true
  rbac:
    allow:
      - verbs: [get, list]
        kinds: [EdgionConfigData]
"#;
        let mut f = std::fs::File::create(path).expect("create config file");
        f.write_all(yaml.as_bytes()).expect("write config");
    }

    #[tokio::test]
    async fn reload_rereads_center_and_swaps_policy_before_request_reload() {
        let tmp_cfg = tempfile::NamedTempFile::new().expect("named tempfile");
        let cfg_path = tmp_cfg.path().to_str().expect("path str").to_string();

        write_center_config(&cfg_path);

        let (state, mut rx, _tmp_data) = make_reload_state(&cfg_path).await;

        // Before: authorizer is deny-all.
        assert!(
            !state
                .authorizer
                .authorize(&state.authorizer.center_role(), Verb::Get, "EdgionConfigData")
                .allowed(),
            "pre-condition: deny-all should deny"
        );

        // Mark the current value as seen so `has_changed()` is meaningful.
        rx.borrow_and_update();

        // Run the reload helper (the part that doesn't need ConfMgr to be live).
        apply_center_reload(&state);

        // Assertion 1: policy is now swapped -- Center can GET EdgionConfigData.
        assert!(
            state
                .authorizer
                .authorize(&state.authorizer.center_role(), Verb::Get, "EdgionConfigData")
                .allowed(),
            "post: authorizer must allow get/EdgionConfigData after policy swap"
        );
        // Also ensure deny-all still applies for ungranted pairs.
        assert!(
            !state
                .authorizer
                .authorize(&state.authorizer.center_role(), Verb::Delete, "EdgionConfigData")
                .allowed(),
            "post: delete/EdgionConfigData must remain denied"
        );

        // Assertion 2: center_reload_tx has a new value (supervisor notified).
        assert!(
            rx.has_changed().expect("channel not closed"),
            "supervisor channel must have a new value"
        );
        let new_cfg = rx.borrow_and_update();
        assert_eq!(new_cfg.name, "ctrl-test", "center config name must match YAML");
        assert!(new_cfg.rbac.is_some(), "center rbac must be present in new value");
    }

    #[tokio::test]
    async fn reload_missing_config_file_keeps_current_policy() {
        // If the config file does not exist, warn and keep current policy.
        let (state, mut rx, _tmp_data) = make_reload_state("/nonexistent/path/edgion-controller.yaml").await;
        rx.borrow_and_update();

        apply_center_reload(&state);

        // Policy must remain deny-all.
        assert!(
            !state
                .authorizer
                .authorize(&state.authorizer.center_role(), Verb::Get, "EdgionConfigData")
                .allowed(),
            "policy must remain deny-all when config file is missing"
        );
        // Channel must NOT have received a new value.
        assert!(
            !rx.has_changed().expect("channel not closed"),
            "supervisor channel must NOT be notified when config load fails"
        );
    }

    #[tokio::test]
    async fn reload_no_center_section_does_not_notify() {
        // Config file with no [center] block: policy unchanged, no notify.
        let tmp_cfg = tempfile::NamedTempFile::new().expect("named tempfile");
        let cfg_path = tmp_cfg.path().to_str().expect("path str").to_string();

        // Write a valid config with no center section.
        std::fs::write(&cfg_path, "# no center section\n").expect("write");

        let (state, mut rx, _tmp_data) = make_reload_state(&cfg_path).await;
        rx.borrow_and_update();

        apply_center_reload(&state);

        assert!(
            !state
                .authorizer
                .authorize(&state.authorizer.center_role(), Verb::Get, "EdgionConfigData")
                .allowed(),
            "policy must remain deny-all when no [center] section present"
        );
        assert!(
            !rx.has_changed().expect("channel not closed"),
            "supervisor channel must NOT be notified when [center] is absent"
        );
    }

    #[tokio::test]
    async fn reload_no_config_file_is_noop() {
        // When config_file is None, apply_center_reload returns immediately --
        // the authorizer policy stays deny-all and the channel is not notified.
        use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

        let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
        let fs_config = FileSystemConfig::new(tmp_data.path().to_path_buf());
        let conf_mgr = Arc::new(
            ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
                .await
                .expect("ConfMgr::create"),
        );
        let schema_validator = Arc::new(SchemaValidator::empty());
        let (tx, mut rx) = tokio::sync::watch::channel(crate::config::CenterClientConfig::default());
        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let state = AdminState {
            conf_mgr,
            schema_validator,
            config_file: None, // no config file
            authorizer,
            center_reload_tx: tx,
        };

        rx.borrow_and_update();

        apply_center_reload(&state);

        // Policy must remain deny-all.
        assert!(
            !state
                .authorizer
                .authorize(&state.authorizer.center_role(), Verb::Get, "EdgionConfigData")
                .allowed(),
            "policy must be unchanged (deny-all) when config_file is None"
        );
        // Channel must NOT have received a new value.
        assert!(
            !rx.has_changed().expect("channel not closed"),
            "supervisor channel must NOT be notified when config_file is None"
        );
    }

    /// Reload that adds a cli_token makes `authorizer.cli_lookup(tok)` resolve afterward.
    ///
    /// Also verifies that cli_tokens are hot-reloaded independently of `[center]`:
    /// the config written here has a cli_tokens entry but NO [center] section.
    #[tokio::test]
    async fn reload_cli_token_added_resolves_after_reload() {
        use crate::authz::sha256_hex;

        let tmp_cfg = tempfile::NamedTempFile::new().expect("named tempfile");
        let cfg_path = tmp_cfg.path().to_str().expect("path str").to_string();

        // Raw token value (not pre-hashed); the config stores sha256_hex(tok).
        let raw_token = "test-cli-token-abc123";
        let token_hash = sha256_hex(raw_token);

        let yaml = format!(
            r#"
cli_tokens:
  - name: reload-bot
    token_sha256: "{}"
    rbac:
      allow:
        - verbs: [get, list]
          kinds: [EdgionConfigData]
"#,
            token_hash
        );
        std::fs::write(&cfg_path, yaml.as_bytes()).expect("write config");

        let (state, mut rx, _tmp_data) = make_reload_state(&cfg_path).await;
        rx.borrow_and_update();

        // Before reload: no cli tokens registered.
        assert!(
            state.authorizer.cli_lookup(raw_token).is_none(),
            "before reload: no cli token"
        );
        assert!(
            !state.authorizer.cli_tokens_present(),
            "before reload: cli_tokens_present must be false"
        );

        apply_center_reload(&state);

        // After reload: cli_lookup resolves and role is correct.
        let (role, name) = state
            .authorizer
            .cli_lookup(raw_token)
            .expect("after reload: cli_lookup must hit");
        assert_eq!(name, "reload-bot");
        assert!(
            state
                .authorizer
                .authorize(&role, Verb::Get, "EdgionConfigData")
                .allowed(),
            "after reload: get/EdgionConfigData must be allowed"
        );
        assert!(
            !state
                .authorizer
                .authorize(&role, Verb::Delete, "EdgionConfigData")
                .allowed(),
            "after reload: delete must remain denied"
        );
        assert!(
            state.authorizer.cli_tokens_present(),
            "after reload: cli_tokens_present must be true"
        );

        // No [center] section => supervisor channel must NOT be notified.
        assert!(
            !rx.has_changed().expect("channel not closed"),
            "supervisor must NOT be notified when no [center] section is present"
        );
    }

    /// Startup drop-invalid: a cli_token with a bad token_sha256 is dropped,
    /// valid entries are still loaded.
    #[tokio::test]
    async fn reload_cli_token_drop_invalid_keeps_valid() {
        use crate::authz::sha256_hex;

        let tmp_cfg = tempfile::NamedTempFile::new().expect("named tempfile");
        let cfg_path = tmp_cfg.path().to_str().expect("path str").to_string();

        let valid_raw = "valid-token-xyz";
        let valid_hash = sha256_hex(valid_raw);

        // One valid entry + one invalid entry (bad hash).
        let yaml = format!(
            r#"
cli_tokens:
  - name: valid-bot
    token_sha256: "{}"
    rbac:
      allow:
        - verbs: [get]
          kinds: [EdgionConfigData]
  - name: bad-bot
    token_sha256: "notahexhash"
    rbac:
      allow:
        - verbs: [get]
          kinds: [EdgionConfigData]
"#,
            valid_hash
        );
        std::fs::write(&cfg_path, yaml.as_bytes()).expect("write config");

        let (state, mut rx, _tmp_data) = make_reload_state(&cfg_path).await;
        rx.borrow_and_update();

        apply_center_reload(&state);

        // valid-bot must resolve; bad-bot must be dropped.
        assert!(
            state.authorizer.cli_lookup(valid_raw).is_some(),
            "valid-bot must resolve after reload"
        );
        assert!(
            state.authorizer.cli_lookup("notahexhash").is_none(),
            "bad-bot must be dropped"
        );
        assert!(
            state.authorizer.cli_tokens_present(),
            "at least one valid entry => present"
        );

        assert!(
            !rx.has_changed().expect("channel not closed"),
            "no [center] section => supervisor not notified"
        );
    }
}

#[cfg(test)]
mod admin_exposure_tests {
    use super::admin_exposure_is_insecure;
    use std::net::SocketAddr;
    use std::str::FromStr;

    fn addr(s: &str) -> SocketAddr {
        SocketAddr::from_str(s).unwrap()
    }

    #[test]
    fn loopback_empty_allowlist_is_safe() {
        assert!(!admin_exposure_is_insecure(&addr("127.0.0.1:12101"), &[]));
    }

    #[test]
    fn ipv6_loopback_empty_allowlist_is_safe() {
        assert!(!admin_exposure_is_insecure(&addr("[::1]:12101"), &[]));
    }

    #[test]
    fn unspecified_empty_allowlist_is_insecure() {
        assert!(admin_exposure_is_insecure(&addr("0.0.0.0:12101"), &[]));
    }

    #[test]
    fn unspecified_with_allowlist_is_safe() {
        assert!(!admin_exposure_is_insecure(
            &addr("0.0.0.0:12101"),
            &["10.0.0.0/8".to_string()]
        ));
    }

    #[test]
    fn specific_nonloopback_empty_allowlist_is_insecure() {
        assert!(admin_exposure_is_insecure(&addr("10.1.2.3:12101"), &[]));
    }

    #[test]
    fn ipv4_mapped_ipv6_loopback_empty_allowlist_is_safe() {
        assert!(!admin_exposure_is_insecure(&addr("[::ffff:127.0.0.1]:12101"), &[]));
    }

    #[test]
    fn ipv6_unspecified_empty_allowlist_is_insecure() {
        assert!(admin_exposure_is_insecure(&addr("[::]:12101"), &[]));
    }

    #[test]
    fn ipv4_mapped_nonloopback_empty_allowlist_is_insecure() {
        assert!(admin_exposure_is_insecure(&addr("[::ffff:10.1.2.3]:12101"), &[]));
    }
}
