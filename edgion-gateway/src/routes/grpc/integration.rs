//! Integration helpers for HTTP route handling
//!
//! This module provides clean integration interfaces for the HTTP pipeline to call,
//! encapsulating all gRPC-specific logic within the `grpc` module.

use crate::lb::{ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
use crate::routes::http::session_persistence::{sp_extract_token, sp_resolve_target};
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use crate::EdgionHttpContext;
use edgion_resources::err::EdError;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::EdgionStatus;
use pingora_proxy::Session;
use std::sync::Arc;

/// Returns `true` when `ctx.request_info.discover_protocol` is `"grpc"` or `"grpc-web"`.
///
/// The protocol is identified in `early_request_filter` and stored on `ctx` before
/// this helper is called.
#[inline]
pub fn is_grpc_protocol(ctx: &EdgionHttpContext) -> bool {
    ctx.request_info
        .discover_protocol
        .as_ref()
        .map(|p| p == "grpc" || p == "grpc-web")
        .unwrap_or(false)
}

/// Try to match a gRPC route against the global gRPC route table.
///
/// Returns `Ok(true)` on a match, `Ok(false)` to fall back to HTTP routing, or `Err` on error.
#[inline]
pub async fn try_match_grpc_route(
    grpc_routes: &Arc<crate::routes::grpc::DomainGrpcRouteRules>,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    index: &GatewayInfoIndex,
) -> Result<bool, EdError> {
    if let Ok((service, method)) = super::match_engine::parse_grpc_path(&ctx.request_info.normalized_current_path) {
        ctx.request_info.grpc_service = Some(service.to_string());
        ctx.request_info.grpc_method = Some(method.to_string());
    }

    match grpc_routes.match_route(session, ctx, index, &ctx.request_info.hostname) {
        Ok((grpc_route_unit, matched_gi)) => {
            ctx.grpc_route_unit = Some(grpc_route_unit);
            ctx.gateway_info = matched_gi;
            ctx.is_grpc_route_matched = true;
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

/// Select a gRPC upstream backend and populate `ctx.selected_grpc_backend`.
///
/// Called from `upstream_peer`. Returns `Ok(Some(()))` when the backend is selected,
/// `Ok(None)` when no gRPC route is active (fall through to HTTP logic), or `Err` on error.
pub async fn handle_grpc_upstream(session: &mut Session, ctx: &mut EdgionHttpContext) -> Result<Option<()>, EdError> {
    if ctx.selected_grpc_backend.is_some() {
        return Ok(Some(()));
    }

    let grpc_route_unit = match ctx.grpc_route_unit.as_ref() {
        Some(unit) => unit,
        None => return Ok(None), // No gRPC route
    };

    // Session Persistence step 1 of 2: extract token + set initial state
    // (mirrors select_http_backend). Step 2 (affinity lookup) runs below after
    // backend_ref is selected, since the affinity map is keyed by
    // `{namespace}/{backend_ref.name}`. The `selected_grpc_backend.is_some()`
    // early return above guarantees this never re-runs on a retry attempt.
    let sp_token = sp_extract_token(
        session,
        &mut ctx.sp_state,
        grpc_route_unit.rule.session_persistence.as_ref(),
    );

    let mut backend_ref = grpc_route_unit
        .backend_finder
        .select()
        .map_err(|err_code| match err_code {
            ERR_NO_BACKEND_REFS => EdError::BackendNotFound(),
            ERR_INCONSISTENT_WEIGHT => EdError::InconsistentWeight(),
            _ => EdError::BackendNotFound(),
        })?;

    if let Some(ref denied) = backend_ref.ref_denied {
        return Err(EdError::RefDenied {
            target_namespace: denied.target_namespace.clone(),
            target_name: denied.target_name.clone(),
            reason: denied
                .reason
                .clone()
                .unwrap_or_else(|| "NoMatchingReferenceGrant".to_string()),
        });
    }

    // Query BackendTLSPolicy using route namespace for proper namespace inheritance
    let service_name = &backend_ref.name;
    let service_namespace = backend_ref
        .namespace
        .as_deref()
        .or(Some(grpc_route_unit.matched_info.rsc.ns.as_str()));

    backend_ref.schema.backend_tls_policy =
        crate::backends::query_backend_tls_policy_for_service(service_name, service_namespace, backend_ref.port);

    // Pre-extract route namespace before mutable ctx borrows (borrow checker),
    // mirroring select_http_backend.
    let route_ns = grpc_route_unit.matched_info.rsc.ns.clone();

    // Run backend-level request plugins.
    // TODO: evaluate whether backend plugins should run before route-level plugins
    backend_ref.plugin_runtime.run_request_plugins(session, ctx).await;

    if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
        if !ctx.request_info.response_committed {
            ctx.add_error(EdgionStatus::PluginTerminateWithoutResponse);
        }
        return Err(EdError::PluginTerminated());
    }

    // Session Persistence step 2 of 2: resolve token to a sticky target via the
    // affinity map. Step 1 ran above before backend_ref selection.
    sp_resolve_target(
        ctx,
        &backend_ref.name,
        backend_ref.namespace.as_deref(),
        &route_ns,
        sp_token.as_deref(),
    );

    // Backend context is initialized in upstream_peer_grpc by init_backend_context_if_needed
    ctx.selected_grpc_backend = Some(backend_ref);

    Ok(Some(())) // Handled
}
