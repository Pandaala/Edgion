// pub(crate): also used by the fed_client watch check (Task 6).
pub(crate) mod authz_classifier;
// pub(crate): authz_layer and inject_center_identity are used at the fed spawn site (Task 6).
pub(crate) mod authz_middleware;
pub(crate) mod cli_token_middleware;
mod cluster_handlers;
mod common;
mod configserver_handlers;
mod debug_handlers;
mod diagnostics_handlers;
mod namespaced_handlers;
mod region_route_handlers;
mod resource_locks;
mod types;
mod wipe_handlers;

use crate::core::common::api::{self, ApiResponse};
use crate::core::controller::conf_mgr::{ConfMgr, SchemaValidator};
use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post, put},
    Router,
};
use serde::Serialize;
use std::sync::Arc;
use types::AdminState;

/// Controller-side middleware that augments `/api/v1/auth/status` responses with
/// the `"cli"` active provider when at least one cli token is registered.
///
/// The `/api/v1/auth/status` handler is mounted by the shared `compose_admin_routes`
/// and calls `UnifiedAuthState::build_status()`, which knows only about OIDC and
/// local providers. The `Authorizer` (which knows about cli tokens) lives in
/// `AdminState`, not in `UnifiedAuthState`. Modifying `unified_auth` to accept an
/// authz resolver would violate the invariant that `unified_auth` never imports the
/// authz engine. Instead, this middleware intercepts the status response on the
/// Controller side and appends `"cli"` when `cli_tokens_present()` is true.
///
/// For other paths this is a transparent pass-through with no body buffering.
async fn cli_auth_status_augment(
    State(authorizer): State<Arc<crate::core::controller::authz::Authorizer>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // Only act on the exact auth/status path; pass everything else straight through.
    let is_status = req.uri().path() == "/api/v1/auth/status";
    let resp = next.run(req).await;

    if !is_status || !authorizer.cli_tokens_present() {
        return resp;
    }

    // Deserialize the JSON body, append "cli" to active_providers, re-serialize.
    // If anything fails (body read, parse, serialize) return the original response
    // unmodified -- this must never 500 on its own.
    let (parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 1 << 20).await else {
        // Body read failed: reconstruct original response with empty body (best-effort).
        return Response::from_parts(parts, Body::empty());
    };

    let mut value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            // Not JSON (unexpected) -- return as-is.
            return Response::from_parts(parts, Body::from(bytes));
        }
    };

    // Append "cli" to data.active_providers if present and not already listed.
    if let Some(providers) = value
        .get_mut("data")
        .and_then(|d| d.get_mut("active_providers"))
        .and_then(|p| p.as_array_mut())
    {
        if !providers.iter().any(|v| v.as_str() == Some("cli")) {
            providers.push(serde_json::Value::String("cli".to_string()));
        }
    }

    let Ok(new_bytes) = serde_json::to_vec(&value) else {
        return Response::from_parts(parts, Body::from(bytes));
    };

    let mut resp = Response::from_parts(parts, Body::from(new_bytes));
    // Re-set Content-Type (it may have been set to application/json already).
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    resp
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

// ============= Legacy Endpoints =============

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
            // Server not ready yet
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

    let cli_cfg = crate::core::common::config::EdgionControllerConfig {
        config_file: Some(path.clone()),
        ..Default::default()
    };

    let full = match crate::core::common::config::EdgionControllerConfig::load(cli_cfg) {
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
            crate::core::controller::authz::lint::lint_rbac_policy(rbac, "reload", "center");
        }
        let policy = crate::core::controller::fed_sync::default_policy::center_policy_from(center.rbac.as_ref());
        (policy, Some(center))
    } else {
        (crate::core::controller::authz::CenterPolicy::deny_all(), None)
    };

    // --- 2) Rebuild cli_tokens map ---
    // Uses the shared lint pass: per-entry shape validation, GLOBAL dedup
    // (no silent last-wins on duplicate name/hash), advisory WARNs for
    // unknown/non-canonical kinds, unknown verbs, and dangerous grants.
    // Map key = token_sha256 VERBATIM (already a hash; do NOT re-hash).
    let cli_map = crate::core::controller::authz::lint::build_cli_token_map(&full.cli_tokens, "reload");

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
async fn trigger_acme(Path((namespace, name)): Path<(String, String)>) -> Json<ApiResponse<String>> {
    let key = format!("{}/{}", namespace, name);
    tracing::info!(
        component = "admin_api",
        event = "acme_trigger",
        key = %key,
        "ACME trigger requested via Admin API"
    );

    crate::core::controller::services::acme::notify_resource_changed(key.clone());

    Json(ApiResponse::ok_body(format!("ACME check triggered for {}", key)))
}

// ============= Router Setup =============

/// Build the shared admin state. Used by the admin router and the dedicated
/// probe listener (readiness reads the same `conf_mgr`-backed state).
pub fn build_admin_state(
    conf_mgr: Arc<ConfMgr>,
    schema_validator: Arc<SchemaValidator>,
    config_file: Option<String>,
    authorizer: Arc<crate::core::controller::authz::Authorizer>,
    center_reload_tx: tokio::sync::watch::Sender<crate::core::common::config::CenterClientConfig>,
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
    Router::new().route(
        "/metrics",
        get(crate::core::controller::observe::status_metrics::metrics_handler),
    )
}

/// Create the admin API router with unified K8s-style endpoints
pub fn create_admin_router(
    conf_mgr: Arc<ConfMgr>,
    schema_validator: Arc<SchemaValidator>,
    config_file: Option<String>,
    authorizer: Arc<crate::core::controller::authz::Authorizer>,
    center_reload_tx: tokio::sync::watch::Sender<crate::core::common::config::CenterClientConfig>,
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
        // RegionRoute PluginMetaData endpoints
        .route(
            "/api/v1/cluster-region-routes",
            get(region_route_handlers::list_cluster_region_plugin_metadata),
        )
        .route(
            "/api/v1/service-region-routes",
            get(region_route_handlers::list_service_region_plugin_metadata),
        )
        // New-URL aliases for RegionRoute failover
        .route(
            "/api/v1/cluster-region-routes/failover",
            post(region_route_handlers::cluster_plugin_metadata_failover),
        )
        .route(
            "/api/v1/service-region-routes/failover",
            post(region_route_handlers::service_plugin_metadata_failover),
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

    if crate::core::common::config::is_test_mode() {
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
    auth_config: Option<&crate::core::common::auth::AdminAuthConfig>,
    local_auth_config: Option<&crate::core::common::local_auth::LocalAuthConfig>,
    require_auth: bool,
    config_file: Option<String>,
    authorizer: Arc<crate::core::controller::authz::Authorizer>,
    center_reload_tx: tokio::sync::watch::Sender<crate::core::common::config::CenterClientConfig>,
) -> anyhow::Result<()> {
    serve_with_shutdown(
        conf_mgr,
        schema_validator,
        addr,
        auth_config,
        local_auth_config,
        require_auth,
        config_file,
        authorizer,
        center_reload_tx,
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
    auth_config: Option<&crate::core::common::auth::AdminAuthConfig>,
    local_auth_config: Option<&crate::core::common::local_auth::LocalAuthConfig>,
    require_auth: bool,
    config_file: Option<String>,
    authorizer: Arc<crate::core::controller::authz::Authorizer>,
    center_reload_tx: tokio::sync::watch::Sender<crate::core::common::config::CenterClientConfig>,
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let auth_state = crate::core::common::unified_auth::UnifiedAuthState::from_configs(
        auth_config,
        local_auth_config,
        require_auth,
        "admin_api",
    )?;
    let local_auth_intent = local_auth_config.as_ref().is_some_and(|c| c.enabled);

    let base_router = create_admin_router(
        conf_mgr.clone(),
        schema_validator,
        config_file,
        authorizer.clone(),
        center_reload_tx,
    );

    // Wire authz route_layers on the local admin router.
    //
    // Axum route_layers are applied LIFO: the last `.route_layer()` call becomes
    // the outermost wrapper (runs first). To achieve:
    //   assign_superuser_if_no_role (runs before authz)
    //     -> authz_layer (innermost route_layer, classifies + enforces)
    //       -> handler
    //
    // We must add authz_layer first, then assign_superuser_if_no_role on top.
    // `authorizer.clone()` is required here: `authorizer` is moved into the
    // business route_layer, and the outer cli-pre `.layer()` below needs its own
    // Arc clone — without clone() this would be a borrow-after-move.
    let business = base_router
        .route_layer(axum::middleware::from_fn_with_state(
            authorizer.clone(),
            authz_middleware::authz_layer,
        ))
        .route_layer(axum::middleware::from_fn(authz_middleware::assign_superuser_if_no_role));

    // The composition order is fixed inside compose_admin_routes (see its doc).
    // compose_admin_routes adds unified_auth and then a cache-control layer; both
    // wrap the route_layers above and run before authz_layer/assign on each request.
    let composed = api::compose_admin_routes(business, auth_state.clone(), local_auth_intent);

    // Wrap the composed result with cli-token pre-authn as the outermost layer.
    // This only ADDS authentication (injects Role + AuthBypass for known cli tokens);
    // it does not add new routes, so compose's "final router" contract is satisfied.
    let app = composed
        .layer(axum::middleware::from_fn_with_state(
            authorizer.clone(),
            cli_token_middleware::cli_token_pre_authn,
        ))
        // Augment /api/v1/auth/status responses with "cli" active_provider when
        // cli tokens are registered. This is Controller-side only and does not
        // touch shared unified_auth code (which must never import authz).
        // TODO: if compose ever exposes a provider-augmentation hook, migrate this
        // interceptor to that hook and remove the body-rewriting approach.
        .layer(axum::middleware::from_fn_with_state(
            authorizer,
            cli_auth_status_augment,
        ));

    tracing::info!(
        component = "admin_api",
        addr = %addr,
        "Controller Admin API server listening with unified K8s-style routes"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await?;

    tracing::info!(component = "admin_api", "Admin API server stopped");
    Ok(())
}

#[cfg(test)]
mod auth_status_tests {
    use crate::core::common::local_auth::{AuthStatus, LocalAuthConfig, LocalAuthState};
    use crate::core::common::unified_auth::UnifiedAuthState;

    fn valid_local() -> LocalAuthConfig {
        let mut c = LocalAuthConfig::default();
        c.enabled = true;
        c.username = "admin".to_string();
        c.password = "a_long_enough_password_123".to_string();
        c.jwt_secret = "a_long_enough_jwt_secret_value_abcdef".to_string();
        c
    }

    #[test]
    fn build_auth_status_non_ready_states() {
        // When auth is required but no provider configured: Initializing, auth_required == true, providers empty
        // (The Disabled status is unreachable since require_auth is always true in production.)
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        let resp = state.build_status();
        assert!(resp.auth_required);
        assert!(resp.active_providers.is_empty());
        assert_eq!(resp.status, AuthStatus::Initializing);
    }

    #[test]
    fn build_auth_status_ready_states() {
        // When auth is required and local provider configured from the start: Ready, active_providers == ["local"]
        let cfg = valid_local();
        let state = UnifiedAuthState::from_configs(None, Some(&cfg), true, "test").unwrap();
        let resp = state.build_status();
        assert!(resp.auth_required);
        assert_eq!(resp.active_providers, vec!["local".to_string()]);
        assert_eq!(resp.status, AuthStatus::Ready);

        // Dynamic update: start Initializing -> call update_local -> becomes Ready
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        assert_eq!(state.build_status().status, AuthStatus::Initializing);
        state.update_local(LocalAuthState::from_config(&valid_local()));
        assert_eq!(state.build_status().status, AuthStatus::Ready);
    }
}

#[cfg(test)]
mod reload_center_tests {
    use super::*;
    use crate::core::controller::authz::{Authorizer, CenterPolicy, Verb};
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
        tokio::sync::watch::Receiver<crate::core::common::config::CenterClientConfig>,
        tempfile::TempDir, // kept alive so its path stays valid
    ) {
        use crate::core::controller::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

        let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
        let fs_config = FileSystemConfig::new(tmp_data.path().to_path_buf());
        let conf_mgr = Arc::new(
            ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
                .await
                .expect("ConfMgr::create"),
        );
        let schema_validator = Arc::new(SchemaValidator::empty());
        let (tx, rx) = tokio::sync::watch::channel(crate::core::common::config::CenterClientConfig::default());
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
    /// with an RBAC rule granting `get` on `PluginMetaData`.
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
        kinds: [PluginMetaData]
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
                .authorize(&state.authorizer.center_role(), Verb::Get, "PluginMetaData")
                .allowed(),
            "pre-condition: deny-all should deny"
        );

        // Mark the current value as seen so `has_changed()` is meaningful.
        rx.borrow_and_update();

        // Run the reload helper (the part that doesn't need ConfMgr to be live).
        apply_center_reload(&state);

        // Assertion 1: policy is now swapped -- Center can GET PluginMetaData.
        assert!(
            state
                .authorizer
                .authorize(&state.authorizer.center_role(), Verb::Get, "PluginMetaData")
                .allowed(),
            "post: authorizer must allow get/PluginMetaData after policy swap"
        );
        // Also ensure deny-all still applies for ungranted pairs.
        assert!(
            !state
                .authorizer
                .authorize(&state.authorizer.center_role(), Verb::Delete, "PluginMetaData")
                .allowed(),
            "post: delete/PluginMetaData must remain denied"
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
                .authorize(&state.authorizer.center_role(), Verb::Get, "PluginMetaData")
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
                .authorize(&state.authorizer.center_role(), Verb::Get, "PluginMetaData")
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
        use crate::core::controller::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

        let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
        let fs_config = FileSystemConfig::new(tmp_data.path().to_path_buf());
        let conf_mgr = Arc::new(
            ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
                .await
                .expect("ConfMgr::create"),
        );
        let schema_validator = Arc::new(SchemaValidator::empty());
        let (tx, mut rx) = tokio::sync::watch::channel(crate::core::common::config::CenterClientConfig::default());
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
                .authorize(&state.authorizer.center_role(), Verb::Get, "PluginMetaData")
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
        use crate::core::controller::authz::sha256_hex;

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
          kinds: [PluginMetaData]
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
            state.authorizer.authorize(&role, Verb::Get, "PluginMetaData").allowed(),
            "after reload: get/PluginMetaData must be allowed"
        );
        assert!(
            !state
                .authorizer
                .authorize(&role, Verb::Delete, "PluginMetaData")
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
        use crate::core::controller::authz::sha256_hex;

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
          kinds: [PluginMetaData]
  - name: bad-bot
    token_sha256: "notahexhash"
    rbac:
      allow:
        - verbs: [get]
          kinds: [PluginMetaData]
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
