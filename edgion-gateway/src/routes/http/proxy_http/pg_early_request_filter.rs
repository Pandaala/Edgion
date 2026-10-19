use super::EdgionHttpProxy;
use crate::EdgionHttpContext;
use pingora_core::modules::http::grpc_web::GrpcWebBridge;
use pingora_proxy::Session;
#[cfg(test)]
use std::sync::Arc;

fn invalid_request_id_attachment() -> Box<pingora_core::Error> {
    pingora_core::Error::explain(
        pingora_core::ErrorType::HTTPStatus(500),
        "invalid early RequestId attachment",
    )
}

/// Capture policy only when the publication has one serving logical Gateway.
pub(super) async fn initialize_request_ids(
    proxy: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    initialize_request_ids_with_publisher(proxy, session, ctx, crate::plugins::publication::global_publisher()).await
}

async fn initialize_request_ids_with_publisher(
    proxy: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    publisher: &crate::plugins::publication::PluginPublisher,
) -> pingora_core::Result<()> {
    if ctx.listener_publication().is_none() {
        ctx.capture_listener_publication(publisher.snapshot().http_listener(&proxy.listener_key));
    }
    if ctx.global_request_ids_initialized {
        return Ok(());
    }
    ctx.request_info.trace_id.initialize(&session.req_header().headers);
    ctx.request_info.trace_id.reconcile_request(session.req_header_mut());
    super::pg_request_filter::capture_tls_metadata(session, ctx);
    initialize_scoped_request_ids(session, ctx).await
}

pub(super) fn capture_matched_scope(ctx: &mut EdgionHttpContext) -> pingora_core::Result<()> {
    let gateway = ctx.gateway_info.as_ref().ok_or_else(invalid_request_id_attachment)?;
    let key = gateway.gateway_key();
    if let Some(scope) = ctx.plugin_scope() {
        if scope.gateway_key != key {
            return Err(invalid_request_id_attachment());
        }
        return Ok(());
    }
    ctx.capture_matched_plugin_scope()
        .then_some(())
        .ok_or_else(unavailable_policy)
}

fn unavailable_policy() -> Box<pingora_core::Error> {
    pingora_core::Error::explain(pingora_core::ErrorType::HTTPStatus(503), "plugin policy unavailable")
}

pub(super) async fn initialize_gateway_request_ids(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    initialize_scoped_request_ids(session, ctx).await
}

async fn initialize_scoped_request_ids(session: &mut Session, ctx: &mut EdgionHttpContext) -> pingora_core::Result<()> {
    let Some(owner) = ctx.plugin_scope().cloned() else {
        return Ok(());
    };
    if !owner.scope.available {
        return Err(unavailable_policy());
    }
    if !ctx.global_request_ids_initialized {
        let plugins = super::resolve_global_plugins(&owner.scope);
        ctx.install_resolved_global_plugins(plugins);
    }
    if !ctx.gateway_request_ids_initialized {
        let plugins = super::resolve_gateway_plugins(&owner.scope, &owner.http_keys);
        ctx.install_resolved_gateway_plugins(plugins);
    }
    let global = ctx.resolved_global_plugins();
    let gateway = ctx.resolved_gateway_plugins();
    for runtime in global
        .iter()
        .flat_map(|plugins| plugins.iter())
        .chain(gateway.iter().flat_map(|plugins| plugins.iter()))
    {
        runtime.run_preflight_gate(session, ctx).await;
        if ctx.plugin_terminated() {
            return Ok(());
        }
    }
    if !ctx.global_request_ids_initialized {
        ctx.global_request_ids_initialized = true;
        for runtime in global.iter().flat_map(|plugins| plugins.iter()) {
            if !runtime.run_early_request_ids(session, ctx).await {
                return Err(invalid_request_id_attachment());
            }
        }
    }
    if !ctx.gateway_request_ids_initialized {
        ctx.gateway_request_ids_initialized = true;
        for runtime in gateway.iter().flat_map(|plugins| plugins.iter()) {
            if !runtime.run_early_request_ids(session, ctx).await {
                return Err(invalid_request_id_attachment());
            }
        }
    }
    Ok(())
}

#[inline]
fn apply_client_keepalive_timeout(session: &mut Session, keepalive_timeout: u64, process_shutting_down: bool) {
    // Preserve transport-level refusals such as ambiguous H1 framing. Setting
    // a timeout here must only narrow an already reusable session, never turn
    // a core-disabled connection back on.
    let keepalive = (!process_shutting_down && session.get_keepalive().is_some()).then_some(keepalive_timeout);
    session.set_keepalive(keepalive);
}

pub(crate) fn apply_client_policy(
    session: &mut Session,
    config: &crate::config::ResolvedGatewayConfig,
    keepalive_initialized: &mut bool,
) {
    let timeout = &config.parsed_timeouts.client;
    session.set_read_timeout(Some(timeout.read_timeout));
    session.set_write_timeout(Some(timeout.write_timeout));
    apply_client_keepalive_timeout(session, timeout.keepalive_timeout, session.is_process_shutting_down());
    if !*keepalive_initialized && session.downstream_session.as_http1().is_some() {
        // The operator value counts the first request, while Pingora stores only
        // the number of later reuses. Zero remains the unlimited sentinel.
        let request_limit = config.downstream_keepalive_request_limit;
        session.set_keepalive_reuses_remaining(request_limit.checked_sub(1));
        *keepalive_initialized = true;
    }
}

#[inline]
fn classify_and_initialize_grpc_web(session: &mut Session, ctx: &mut EdgionHttpContext) {
    super::pg_request_filter::detect_request_protocol(session.req_header(), ctx);
    if ctx.request_info.discover_protocol == Some("grpc-web") {
        if let Some(bridge) = session.downstream_modules_ctx.get_mut::<GrpcWebBridge>() {
            bridge.init();
        }
    }
}

#[inline]
pub async fn early_request_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    apply_client_policy(session, &ctx.egc, &mut ctx.downstream_keepalive_initialized);

    // This hook precedes Pingora's module request filter. Select the captured
    // policy before it decides whether to parse Accept-Encoding.
    if let Some(compression) = session
        .downstream_modules_ctx
        .get_mut::<pingora_core::modules::http::compression::ResponseCompression>()
    {
        compression.adjust_level(if ctx.egc.enable_compression {
            super::pg_init_downstream_modules::DEFAULT_COMPRESSION_LEVEL
        } else {
            0
        });
    }

    // Freeze protocol classification from the raw downstream headers before
    // Pingora's built-in request_header_filter rewrites gRPC-Web to gRPC. Only
    // the canonical gRPC-Web classification activates the bridge, preserving
    // WebSocket precedence and leaving native gRPC and ordinary HTTP inert.
    classify_and_initialize_grpc_web(session, ctx);

    initialize_request_ids(edgion_http, session, ctx).await?;

    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_early_request_filter_test.rs"]
mod tests;
