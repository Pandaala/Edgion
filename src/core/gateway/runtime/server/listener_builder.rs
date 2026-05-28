//! Listener builder module for adding different types of listeners to Pingora Server
//!
//! This module provides a clean abstraction for adding HTTP/HTTPS, TCP, UDP, and other
//! protocol listeners to the gateway server. It separates listener construction logic
//! from the main gateway bootstrap process.

use crate::core::gateway::runtime::ServerHeaderOpts;
use anyhow::Result;
use pingora_core::apps::HttpServerOptions;
use pingora_core::connectors::TransportConnector;
use pingora_core::listeners::Listeners;
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::Server;
use pingora_core::services::listening::Service;
use pingora_proxy::{http_proxy_service, ProxyServiceBuilder};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::net::UdpSocket;

use crate::core::gateway::observe::AccessLogger;
use crate::core::gateway::plugins::stream::{get_global_stream_plugin_store, StreamPluginConnectionFilter};
use crate::core::gateway::routes::http::{EdgionHttpProxy, EdgionHttpRedirectProxy};
use crate::core::gateway::routes::tcp::{get_global_tcp_route_managers, EdgionTcpProxy};
use crate::core::gateway::routes::tls::{get_global_tls_route_managers, EdgionTlsPassthroughProxy, EdgionTlsTcpProxy};
use crate::core::gateway::routes::udp::{get_global_udp_route_managers, get_udp_listener_registry, EdgionUdpProxy};
#[cfg(feature = "boringssl")]
use crate::core::gateway::tls::runtime::gateway::tls_pingora::TlsCallback;
use crate::types::constants::annotations::edgion;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::resources::gateway::{GatewayTlsMode, Listener};
use crate::types::ListenerKey;
use tokio_util::sync::CancellationToken;

/// Context passed to listener builders containing gateway-level information and listener config
#[derive(Clone)]
pub struct ListenerContext {
    pub gateway_class_name: Option<String>,
    pub gateway_namespace: Option<String>,
    pub gateway_name: String,
    pub gateway_key: String,
    pub listener: Listener,
    pub listener_key: ListenerKey,
    pub access_logger: Arc<AccessLogger>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub server_conf: Arc<ServerConf>,
    /// Whether to enable HTTP/2 support (from Gateway annotation)
    pub enable_http2: bool,
    /// Gateway annotations
    pub gateway_annotations: std::collections::HashMap<String, String>,
}

/// Add an HTTP or HTTPS listener to the Pingora server
///
/// This function creates an HTTP proxy service and adds it to the server
/// with or without TLS based on the enable_tls parameter.
///
/// # Parameters
/// - `enable_tls`: Whether to enable TLS/HTTPS
/// - `enable_http2`: Whether to enable HTTP/2 support (h2c for HTTP, ALPN for HTTPS)
pub fn add_http_listener(
    server: &mut Server,
    context: &ListenerContext,
    enable_tls: bool,
    enable_http2: bool,
) -> Result<()> {
    let listener_name = context.listener.name.clone();
    // Hostname is for SNI matching, not for binding - always bind to 0.0.0.0
    let addr = format!("0.0.0.0:{}", context.listener.port);

    // Check if HTTP to HTTPS redirect is enabled (only for non-TLS listeners)
    let http_to_https_redirect = !enable_tls
        && context
            .gateway_annotations
            .get(edgion::HTTP_TO_HTTPS_REDIRECT)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

    // If redirect is enabled, use the simple redirect handler
    if http_to_https_redirect {
        let https_port: u16 = context
            .gateway_annotations
            .get(edgion::HTTPS_REDIRECT_PORT)
            .and_then(|v| v.parse().ok())
            .unwrap_or(443);

        let redirect_handler = EdgionHttpRedirectProxy::new(
            https_port,
            context.access_logger.clone(),
            context.edgion_gateway_config.clone(),
        );
        let mut http_service = http_proxy_service(&context.server_conf, redirect_handler);
        apply_connection_filter(&mut http_service, context);
        http_service.add_tcp(&addr);

        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            addr=%addr,
            https_port=%https_port,
            "Adding HTTP to HTTPS redirect listener"
        );

        server.add_service(http_service);
        return Ok(());
    }

    // Pre-parse timeout configurations once at initialization
    let parsed_timeouts =
        crate::core::gateway::routes::http::proxy_http::ParsedTimeouts::from_config(&context.edgion_gateway_config);

    // Initialize RealIpExtractor from configuration
    let real_ip_extractor = if let Some(real_ip_config) = &context.edgion_gateway_config.spec.real_ip {
        match crate::core::common::utils::RealIpExtractor::new(
            &real_ip_config.trusted_ips,
            real_ip_config.real_ip_header.clone(),
            real_ip_config.recursive,
            real_ip_config.max_trusted_hops,
        ) {
            Ok(extractor) => Some(Arc::new(extractor)),
            Err(e) => {
                tracing::warn!(
                    gateway = %context.gateway_name,
                    listener = %listener_name,
                    error = ?e,
                    "Failed to build RealIpExtractor, real IP extraction disabled"
                );
                None
            }
        }
    } else {
        None
    };

    // Create preflight handler from gateway config
    let preflight_handler = crate::core::gateway::routes::http::proxy_http::PreflightHandler::new(
        context.edgion_gateway_config.spec.preflight_policy.clone(),
    );

    let http_route_manager = crate::core::gateway::routes::http::get_global_http_route_managers()
        .get_or_create_listener_manager(&context.listener_key);
    let grpc_route_manager = crate::core::gateway::routes::grpc::get_global_grpc_route_managers()
        .get_or_create_listener_manager(&context.listener_key);

    let edgion_http = EdgionHttpProxy {
        gateway_class_name: context.gateway_class_name.clone(),
        listener: context.listener.clone(),
        listener_key: context.listener_key.clone(),
        server_start_time: SystemTime::now(),
        server_header_opts: {
            let mut opts = ServerHeaderOpts::default();
            if !enable_tls {
                opts.remove_hsts();
            }
            opts
        },
        access_logger: context.access_logger.clone(),
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        parsed_timeouts,
        enable_http2: context.enable_http2,
        real_ip_extractor,
        preflight_handler,
        http_route_manager,
        grpc_route_manager,
    };

    // Build HttpServerOptions: h2c + downstream keepalive request limit
    let mut opts = HttpServerOptions::default();

    if !enable_tls && enable_http2 {
        opts.h2c = true;
        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            "Enabled h2c (HTTP/2 Cleartext) support"
        );
    }

    // Downstream keepalive request limit (per-connection, HTTP/1.1 only)
    // 0 means unlimited; non-zero sets the limit
    let limit = context
        .edgion_gateway_config
        .spec
        .server
        .as_ref()
        .map(|s| s.downstream_keepalive_request_limit)
        .unwrap_or(1000);
    if limit > 0 {
        opts.keepalive_request_limit = Some(limit);
        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            limit=%limit,
            "Downstream keepalive request limit configured"
        );
    }

    let mut http_service = ProxyServiceBuilder::new(&context.server_conf, edgion_http)
        .server_options(opts)
        .build();

    // Apply connection filter if configured via annotation
    apply_connection_filter(&mut http_service, context);

    // Add listener with or without TLS
    if enable_tls {
        #[cfg(feature = "boringssl")]
        {
            let port = context.listener.port as u16;
            let mut tls_settings = TlsCallback::new_tls_settings_with_callback(
                port,
                context.listener_key.clone(),
                context.edgion_gateway_config.clone(),
                true,
            )?;
            // Enable HTTP/2 for HTTPS if enable_http2 is true
            if enable_http2 {
                tls_settings.enable_h2();
            }
            http_service.add_tls_with_settings(&addr, None, tls_settings);
            let protocol = if enable_http2 {
                "HTTPS (HTTP/2 enabled)"
            } else {
                "HTTPS"
            };
            tracing::info!(
                gateway=%context.gateway_key,
                listener=%listener_name,
                addr=%addr,
                protocol=%protocol,
                "Adding TLS listener"
            );
        }

        #[cfg(not(feature = "boringssl"))]
        {
            anyhow::bail!("TLS support requires the 'boringssl' feature");
        }
    } else {
        http_service.add_tcp(&addr);
        let protocol = if enable_http2 { "HTTP (h2c enabled)" } else { "HTTP" };
        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            addr=%addr,
            protocol=%protocol,
            "Adding TCP listener"
        );
    }

    // Add service to server
    server.add_service(http_service);

    Ok(())
}

/// Add a TCP listener to the Pingora server
pub fn add_tcp_listener(server: &mut Server, context: &ListenerContext) -> Result<()> {
    let listener_name = context.listener.name.clone();
    // Hostname is for SNI matching, not for binding - always bind to 0.0.0.0
    let addr = format!("0.0.0.0:{}", context.listener.port);
    let port = context.listener.port as u16;

    let tcp_route_manager = get_global_tcp_route_managers().get_or_create_listener_manager(&context.listener_key);

    let edgion_tcp = EdgionTcpProxy {
        listener_port: port,
        listener_key: context.listener_key.clone(),
        tcp_route_manager,
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        connector: TransportConnector::new(None),
    };

    // Create TCP service
    let mut tcp_service = Service::with_listeners(format!("TCP-{}", listener_name), Listeners::tcp(&addr), edgion_tcp);

    // Apply connection filter if configured via annotation
    apply_connection_filter(&mut tcp_service, context);

    // Add to server
    server.add_service(tcp_service);

    tracing::info!(
        gateway=%context.gateway_key,
        listener=%listener_name,
        addr=%addr,
        protocol="TCP",
        "Adding TCP listener"
    );

    Ok(())
}

/// Add a UDP listener to the Pingora server
///
/// UDP listeners don't use Pingora's Service abstraction — they run as
/// independent tokio tasks.  Each listener gets a per-port route manager
/// (mirroring TLS pattern) and a `CancellationToken` for graceful shutdown.
pub fn add_udp_listener(_server: &mut Server, context: &ListenerContext) -> Result<()> {
    let listener_name = context.listener.name.clone();
    let addr = format!("0.0.0.0:{}", context.listener.port);
    let port = context.listener.port as u16;

    let udp_route_manager = get_global_udp_route_managers().get_or_create_listener_manager(&context.listener_key);

    let socket = std::net::UdpSocket::bind(&addr)?;
    socket.set_nonblocking(true)?;
    let socket = UdpSocket::from_std(socket)?;

    let cancel_token = CancellationToken::new();
    get_udp_listener_registry().register(&context.listener_key, cancel_token.clone());

    let edgion_udp = Arc::new(EdgionUdpProxy::new(
        port,
        context.listener_key.clone(),
        udp_route_manager,
        context.edgion_gateway_config.clone(),
        socket,
        cancel_token,
    ));

    tokio::spawn(async move {
        edgion_udp.serve().await;
    });

    tracing::info!(
        gateway=%context.gateway_key,
        listener=%listener_name,
        addr=%addr,
        protocol="UDP",
        "Adding UDP listener"
    );

    Ok(())
}

/// Add a TLS terminate to TCP listener to the Pingora server
///
/// This function creates a TLS listener that terminates TLS and forwards
/// plain TCP traffic to backend services based on SNI routing.
#[cfg(feature = "boringssl")]
pub fn add_tls_terminate_to_tcp_listener(server: &mut Server, context: &ListenerContext) -> Result<()> {
    let listener_name = context.listener.name.clone();
    // Hostname is for SNI matching, not for binding - always bind to 0.0.0.0
    let addr = format!("0.0.0.0:{}", context.listener.port);
    let port = context.listener.port as u16;

    let tls_route_manager = get_global_tls_route_managers().get_or_create_listener_manager(&context.listener_key);

    let edgion_tls = EdgionTlsTcpProxy {
        listener_port: port,
        listener_key: context.listener_key.clone(),
        tls_route_manager,
        access_logger: context.access_logger.clone(),
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        connector: TransportConnector::new(None),
    };

    // Create TLS settings with callback for certificate loading (with port for SNI lookup)
    let tls_settings = TlsCallback::new_tls_settings_with_callback(
        port,
        context.listener_key.clone(),
        context.edgion_gateway_config.clone(),
        false,
    )?;

    let mut tls_service = Service::new(format!("TLS-TCP-{}", listener_name), edgion_tls);

    // Apply connection filter if configured via annotation
    apply_connection_filter(&mut tls_service, context);

    // Only bind the TLS endpoint; Listeners::tcp() would bind a plain TCP socket
    // on the same port, causing EEXIST when add_tls_with_settings tries to bind again.
    tls_service.add_tls_with_settings(&addr, None, tls_settings);

    // Add to server
    server.add_service(tls_service);

    tracing::info!(
        gateway=%context.gateway_key,
        listener=%listener_name,
        addr=%addr,
        protocol="TLS-TCP",
        "Adding TLS terminate to TCP listener"
    );

    Ok(())
}

/// Add a TLS Passthrough listener to the Pingora server.
///
/// This creates a **plain TCP** listener that extracts SNI from the TLS
/// ClientHello without performing a TLS handshake, then transparently
/// relays the encrypted traffic to the matched backend.
pub fn add_tls_passthrough_listener(server: &mut Server, context: &ListenerContext) -> Result<()> {
    let listener_name = context.listener.name.clone();
    let addr = format!("0.0.0.0:{}", context.listener.port);
    let port = context.listener.port as u16;

    let tls_route_manager = get_global_tls_route_managers().get_or_create_listener_manager(&context.listener_key);

    let proxy = EdgionTlsPassthroughProxy {
        listener_port: port,
        listener_key: context.listener_key.clone(),
        tls_route_manager,
        access_logger: context.access_logger.clone(),
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        connector: TransportConnector::new(None),
    };

    let mut service = Service::with_listeners(
        format!("TLS-Passthrough-{}", listener_name),
        Listeners::tcp(&addr),
        proxy,
    );

    apply_connection_filter(&mut service, context);

    server.add_service(service);

    tracing::info!(
        gateway=%context.gateway_key,
        listener=%listener_name,
        addr=%addr,
        protocol="TLS-Passthrough",
        "Adding TLS passthrough listener"
    );

    Ok(())
}

/// Main entry point for adding a listener to the server
///
/// This function dispatches to the appropriate listener builder based on the
/// protocol specified in the listener configuration.
pub fn add_listener(server: &mut Server, context: ListenerContext) -> Result<()> {
    match context.listener.protocol.to_uppercase().as_str() {
        "HTTP" => add_http_listener(server, &context, false, context.enable_http2),
        "HTTPS" => add_http_listener(server, &context, true, context.enable_http2),
        "TCP" => add_tcp_listener(server, &context),
        "UDP" => add_udp_listener(server, &context),
        "TLS" => {
            // Listener TLS mode: Passthrough or Terminate (default).
            // CRD/serde already reject any other value; the controller filters
            // unsupported combinations (e.g. mixed-mode listeners on the same
            // port) before pushing config to the gateway.
            let tls_mode = context.listener.tls.as_ref().and_then(|t| t.mode).unwrap_or_default();

            match tls_mode {
                GatewayTlsMode::Passthrough => add_tls_passthrough_listener(server, &context),
                GatewayTlsMode::Terminate => {
                    // Terminate mode — default to TCP backend (TLSRoute semantics).
                    // `protocol: TLS` + `mode: Terminate` inherently means TLS→TCP;
                    // HTTPS already covers TLS→HTTP. No annotation required.
                    #[cfg(feature = "boringssl")]
                    {
                        add_tls_terminate_to_tcp_listener(server, &context)
                    }

                    #[cfg(not(feature = "boringssl"))]
                    {
                        anyhow::bail!("TLS protocol requires the 'boringssl' feature")
                    }
                }
            }
        }
        protocol => {
            anyhow::bail!("Unsupported protocol: {}", protocol)
        }
    }
}

/// Apply connection filter to a Pingora Service if the Gateway has the
/// `edgion.io/edgion-stream-plugins` annotation set.
///
/// The annotation value may be `name` (expanded with the Gateway's namespace) or the
/// fully-qualified `namespace/name` referencing an EdgionStreamPlugins resource. This
/// matches the bare-name convention documented for the Gateway annotation and aligns
/// with the TCPRoute / TLSRoute handlers, which apply the same normalization before
/// looking up the global `StreamPluginStore` (keyed by `namespace/name`).
///
/// The filter runs at TCP level, before TLS handshake or HTTP parsing.
fn apply_connection_filter<A>(service: &mut Service<A>, context: &ListenerContext) {
    let Some(annotation_value) = context.gateway_annotations.get(edgion::EDGION_STREAM_PLUGINS) else {
        return;
    };

    let trimmed = annotation_value.trim();
    if trimmed.is_empty() {
        return;
    }
    // The store is always keyed by the fully-qualified `namespace/name`. Expand a bare
    // name using the Gateway's own namespace so doc-sanctioned short references resolve
    // instead of silently fail-closing the listener.
    let store_key = if trimmed.contains('/') {
        trimmed.to_string()
    } else {
        let ns = context.gateway_namespace.as_deref().unwrap_or("default");
        format!("{}/{}", ns, trimmed)
    };

    let store = get_global_stream_plugin_store();
    let port = context.listener.port as u16;

    if !store.contains(&store_key) {
        tracing::warn!(
            gateway=%context.gateway_key,
            listener=%context.listener.name,
            store_key=%store_key,
            "edgion-stream-plugins reference not found in gateway store; \
             connections will be rejected until resource is synced (fail-close)"
        );
    }

    let filter = Arc::new(StreamPluginConnectionFilter::new(store, store_key.clone(), port));
    service.set_connection_filter(filter);

    tracing::info!(
        gateway=%context.gateway_key,
        listener=%context.listener.name,
        store_key=%store_key,
        "ConnectionFilter enabled via edgion-stream-plugins annotation"
    );
}
