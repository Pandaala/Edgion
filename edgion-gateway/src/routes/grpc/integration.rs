//! Integration helpers for HTTP route handling
//!
//! This module provides clean integration interfaces for the HTTP pipeline to call,
//! encapsulating all gRPC-specific logic within the `grpc` module.

use crate::lb::{ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
use crate::routes::http::session_persistence::{sp_extract_token, sp_resolve_target_rule_scoped};
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use crate::EdgionHttpContext;
use edgion_resources::err::EdError;
use edgion_resources::EdgionErrorCode;
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
        ctx.request_info
            .set_grpc_method(service.to_string(), method.to_string());
    }

    match grpc_routes.match_route(session, ctx, index, &ctx.request_info.hostname) {
        Ok((grpc_route_unit, matched_gi)) => {
            ctx.grpc_route_unit = Some(grpc_route_unit);
            ctx.gateway_info = Some(matched_gi);
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
        // Arc-clone so the unit is independent of `ctx` — the SP resolution
        // below takes `&mut ctx` while iterating the unit's backends.
        Some(unit) => unit.clone(),
        None => return Ok(None), // No gRPC route
    };

    // Session Persistence step 1 of 2: extract token + set initial state
    // (mirrors select_http_backend). The `selected_grpc_backend.is_some()`
    // early return above guarantees this never re-runs on a retry attempt.
    let sp_token = grpc_route_unit.rule.session_persistence.as_ref().and_then(|sp| {
        sp_extract_token(
            session,
            &mut ctx.ensure_session_persistence().state,
            Some(sp),
            crate::routes::http::session_persistence::now_unix_secs(),
        )
    });

    // Session Persistence step 2 of 2: resolve the token BEFORE weighted
    // backendRef selection, across ALL of the rule's backendRefs (GEP-1619:
    // weights govern only new sessions; an established session keeps routing
    // to its original Service/endpoint even at weight 0). Mirrors
    // select_http_backend; gRPC has no InternalJump, so no jump precedence arm.
    let sp_idx = sp_resolve_target_rule_scoped(
        ctx,
        grpc_route_unit
            .backends
            .iter()
            .map(|b| (b.schema.name.as_str(), b.schema.namespace.as_deref(), b.schema.port)),
        &grpc_route_unit.matched_info.rsc.ns,
        sp_token.as_deref(),
    );
    let sticky_backend = sp_idx.and_then(|idx| {
        let cb = &grpc_route_unit.backends[idx];
        if cb.schema.ref_denied.is_none() {
            Some(idx)
        } else {
            // The sticky backendRef's cross-namespace reference is now denied:
            // treat as a miss — drop the sticky target, refresh the token, and
            // fall back to weighted selection below.
            let persistence = ctx.ensure_session_persistence();
            persistence.target_addr = None;
            persistence.sticky_addr = None;
            persistence.state = crate::SessionPersistenceState::InjectNew;
            None
        }
    });

    let backend_index = match sticky_backend {
        Some(index) => index,
        None => grpc_route_unit
            .backend_finder
            .select()
            .map_err(|err_code| match err_code {
                ERR_NO_BACKEND_REFS => EdError::BackendNotFound(),
                ERR_INCONSISTENT_WEIGHT => EdError::InconsistentWeight(),
                _ => EdError::BackendNotFound(),
            })?,
    };

    let logical = super::GrpcBackendHandle::new(Arc::clone(&grpc_route_unit.backends), backend_index)
        .ok_or_else(EdError::BackendNotFound)?;
    let backend_ref = logical.compiled();

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

    let target = logical.policy_target_key().ok_or_else(EdError::BackendNotFound)?;
    let backend_tls_policy = crate::backends::policy::query_backend_tls_policy_for_target(target, backend_ref.port);

    // Run backend-level request plugins.
    // TODO: evaluate whether backend plugins should run before route-level plugins
    let backend_plugin_runtime = Arc::clone(&backend_ref.plugin_runtime);
    backend_plugin_runtime.run_request_plugins(session, ctx).await;

    if ctx.plugin_terminated() {
        if !ctx.response_write_started() {
            ctx.record_error(EdgionErrorCode::PluginTerminateWithoutResponse);
        }
        return Err(EdError::PluginTerminated());
    }

    // Backend context is initialized in upstream_peer_grpc by init_backend_context_if_needed
    ctx.selected_grpc_backend = Some(super::SelectedGrpcBackend::new(logical, backend_tls_policy));

    Ok(Some(())) // Handled
}
