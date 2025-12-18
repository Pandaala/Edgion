///! Integration helpers for http_routes
///!
///! This module provides clean integration interfaces for http_routes to call,
///! encapsulating all gRPC-specific logic within grpc_routes module.

use pingora_proxy::Session;
use crate::types::EdgionHttpContext;
use crate::types::err::EdError;
use crate::types::filters::PluginRunningResult;
use std::sync::Arc;

/// Check if the request is a gRPC request (based on context)
///
/// This is the preferred method. The protocol was already identified in 
/// early_request_filter and stored in ctx.request_info.discover_protocol.
///
/// Note: In most cases, you should check ctx.request_info.discover_protocol directly
/// rather than calling this helper function.
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
pub async fn try_match_grpc_route(
    grpc_routes: &Arc<crate::core::routes::grpc_routes::DomainGrpcRouteRules>,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> Result<bool, EdError> {
    // 1. Parse gRPC service/method from path
    if let Ok((service, method)) =
        super::match_engine::parse_grpc_path(&ctx.request_info.path)
    {
        ctx.request_info.grpc_service = Some(service);
        ctx.request_info.grpc_method = Some(method);
    }

    // 2. Try to match route
    match grpc_routes.match_route(&ctx.request_info.hostname, session) {
        Ok(grpc_route_unit) => {
            tracing::debug!(
                service = ?ctx.request_info.grpc_service,
                method = ?ctx.request_info.grpc_method,
                matched_info = %grpc_route_unit.matched_info,
                "gRPC route matched"
            );

            // 3. Store in context
            ctx.grpc_route_unit = Some(grpc_route_unit);

            Ok(true) // Matched successfully
        }
        Err(_) => {
            tracing::debug!(
                service = ?ctx.request_info.grpc_service,
                method = ?ctx.request_info.grpc_method,
                "gRPC route not found, falling back to HTTP routes"
            );
            Ok(false) // Not matched, fallback
        }
    }
}

/// Run gRPC route-level request plugins
///
/// Should be called in request_filter after route is matched.
/// Returns: Ok(true) - plugin terminated the request
///          Ok(false) - continue processing
pub async fn run_grpc_route_plugins(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> Result<bool, EdError> {
    // Clone the Arc to avoid borrow checker issues
    let grpc_route_unit = match ctx.grpc_route_unit.clone() {
        Some(unit) => unit,
        None => return Ok(false),
    };
    
    // Run rule-level request plugins
    grpc_route_unit
        .rule
        .plugin_runtime
        .run_request_plugins(session, ctx)
        .await;

    if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
        return Ok(true); // Terminate request
    }
    
    Ok(false) // Continue processing
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

    // Clone the Arc to avoid borrow checker issues
    let grpc_route_unit = match ctx.grpc_route_unit.clone() {
        Some(unit) => unit,
        None => return Ok(None), // No gRPC route
    };

    // Select gRPC backend
    use crate::core::routes::grpc_routes::GrpcRouteRules;
    let backend_ref = GrpcRouteRules::select_backend(&grpc_route_unit.rule)?;

    tracing::info!("Selected gRPC backend: {:?}", backend_ref);

    // Run backend-level request plugins
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

