///! Integration helpers for http_routes
///!
///! This module provides clean integration interfaces for http_routes to call,
///! encapsulating all gRPC-specific logic within grpc_routes module.

use pingora_proxy::Session;
use crate::types::EdgionHttpContext;
use crate::types::err::EdError;
use crate::types::filters::PluginRunningResult;
use std::sync::Arc;

use super::GrpcRouteRules;

/// Check if the request is a gRPC request (based on context)
///
/// This is the preferred method. The protocol was already identified in 
/// early_request_filter and stored in ctx.request_info.discover_protocol.
///
/// Note: In most cases, you should check ctx.request_info.discover_protocol directly
/// rather than calling this helper function.
#[inline]
pub fn is_grpc_protocol(ctx: &EdgionHttpContext) -> bool {
    ctx.request_info.discover_protocol
        .as_ref()
        .map(|p| p == "grpc" || p == "grpc-web")
        .unwrap_or(false)
}

/// Try to match gRPC route
///
/// Returns: Ok(true) - matched successfully and handled
///          Ok(false) - not matched, should fallback to HTTP routes
///          Err - error occurred
#[inline]
pub async fn try_match_grpc_route(
    grpc_routes: &Arc<crate::core::routes::grpc_routes::DomainGrpcRouteRules>,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    listener_name: &str,
) -> Result<bool, EdError> {
    // 1. Parse gRPC service/method from path
    if let Ok((service, method)) =
        super::match_engine::parse_grpc_path(&ctx.request_info.path)
    {
        ctx.request_info.grpc_service = Some(service);
        ctx.request_info.grpc_method = Some(method);
    }

    // 2. Try to match route (based on service/method, section_name, and hostname)
    match grpc_routes.match_route(session, listener_name, &ctx.request_info.hostname) {
        Ok(grpc_route_unit) => {
            ctx.grpc_route_unit = Some(grpc_route_unit);
            ctx.is_grpc_route_matched = true;
            Ok(true) // Matched successfully
        }
        Err(_) => {
            Ok(false) // Not matched, fallback
        }
    }
}

/// Handle gRPC upstream peer selection
///
/// Should be called in upstream_peer.
/// Returns: Ok(Some(())) - gRPC backend handled
///          Ok(None) - no gRPC backend, should use HTTP logic
///          Err - error occurred
pub async fn handle_grpc_upstream(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> Result<Option<()>, EdError> {
    // Check if backend is already selected
    if ctx.selected_grpc_backend.is_some() {
        return Ok(Some(())); // Already handled
    }

    // Get reference to grpc_route_unit without cloning
    let grpc_route_unit = match ctx.grpc_route_unit.as_ref() {
        Some(unit) => unit,
        None => return Ok(None), // No gRPC route
    };

    // Select gRPC backend
    let backend_ref = GrpcRouteRules::select_backend(&grpc_route_unit.rule)?;

    // Run backend-level request edgion_plugins
    // todo need keep here or change to run before route plugin run?
    backend_ref
        .plugin_runtime
        .run_request_plugins(session, ctx)
        .await;

    if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
        return Err(EdError::PluginTerminated());
    }

    // Backend context initialization is now handled by init_backend_context_if_needed
    // in upstream_peer_grpc to avoid duplication
    ctx.selected_grpc_backend = Some(backend_ref);

    Ok(Some(())) // Handled
}

