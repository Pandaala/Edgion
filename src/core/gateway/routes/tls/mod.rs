//! TLS Routes and Proxy Implementation
//!
//! This module contains all TLS-related functionality:
//! - Route matching and management ([`routes_mgr`])
//! - TLS proxy implementation ([`proxy`]) — Terminate mode
//! - TLS passthrough proxy ([`passthrough_proxy`]) — Passthrough mode
//! - ClientHello SNI extraction ([`sni_parser`])
//! - Per-port route table ([`gateway_tls_routes`])

mod conf_handler_impl;
pub(crate) mod gateway_tls_routes;
mod routes_mgr;

// TLS proxy modules
pub mod passthrough_proxy;
pub mod proxy;
pub mod sni_parser;

pub use routes_mgr::{get_global_tls_route_managers, GlobalTlsRouteManagers, TlsRouteManager, TlsRouteManagerStats};

pub use conf_handler_impl::create_tls_route_handler;

use crate::core::gateway::plugins::stream::get_global_stream_plugin_store;
use crate::core::gateway::plugins::{StreamContext, StreamPluginResult, TlsRouteContext};
use proxy::TlsContext;

/// Shared Stage 2 TLS route plugin execution.
///
/// Used by both terminate and passthrough proxy.
/// `tls_id` and `is_mtls` differ by mode: passthrough passes `None`/`false`.
pub(super) async fn run_tls_route_plugins(
    rule: &crate::types::resources::tls_route::TLSRouteRule,
    ctx: &mut TlsContext,
    sni: &str,
    listener_port: u16,
    tls_id: Option<String>,
    is_mtls: bool,
) -> bool {
    let Some(store_key) = &rule.stream_plugin_store_key else {
        return true;
    };
    let Ok(client_ip) = ctx.client_addr.parse() else {
        return true;
    };

    let store = get_global_stream_plugin_store();
    let Some(resource) = store.get(store_key) else {
        ctx.push_log("stream plugin resource not found, allowing");
        return true;
    };

    // Prefer Stage 2 (TlsRoute) runtime
    let tls_runtime = &resource.spec.tls_route_plugin_runtime;
    if !tls_runtime.is_empty() {
        let tls_ctx = TlsRouteContext {
            client_ip,
            listener_port,
            sni: sni.to_string(),
            tls_id,
            rsc: ctx
                .matched
                .as_ref()
                .map(|m| m.rsc.clone())
                .unwrap_or_else(|| crate::types::Rsc {
                    kind: "TLSRoute".to_string(),
                    ns: String::new(),
                    name: String::new(),
                }),
            is_mtls,
        };
        return match tls_runtime.run(&tls_ctx).await {
            StreamPluginResult::Allow => {
                ctx.push_log("tls route plugin allowed");
                true
            }
            StreamPluginResult::Deny(reason) => {
                ctx.err_log = Some(format!("tls route plugin denied: {reason}"));
                false
            }
        };
    }

    // Backward compat: fall back to Stage 1 (ConnectionFilter) runtime
    let stream_runtime = &resource.spec.stream_plugin_runtime;
    if !stream_runtime.is_empty() {
        let stream_ctx = StreamContext::new(client_ip, listener_port);
        return match stream_runtime.run(&stream_ctx).await {
            StreamPluginResult::Allow => {
                ctx.push_log("stream plugin allowed (compat)");
                true
            }
            StreamPluginResult::Deny(reason) => {
                ctx.err_log = Some(format!("stream plugin denied (compat): {reason}"));
                false
            }
        };
    }

    true
}

pub use gateway_tls_routes::TlsRouteTable;

// Export TLS proxy types
pub use passthrough_proxy::EdgionTlsPassthroughProxy;
pub use proxy::EdgionTlsTcpProxy;
