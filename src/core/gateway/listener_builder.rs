//! Listener builder module for adding different types of listeners to Pingora Server
//!
//! This module provides a clean abstraction for adding HTTP/HTTPS, TCP, UDP, and other
//! protocol listeners to the gateway server. It separates listener construction logic
//! from the main gateway bootstrap process.

use anyhow::Result;
use pingora_core::apps::HttpServerOptions;
use pingora_core::connectors::TransportConnector;
use pingora_core::listeners::Listeners;
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::Server;
use pingora_core::services::listening::Service;
use pingora_proxy::http_proxy_service;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::net::UdpSocket;

use crate::core::observe::AccessLogger;
use crate::core::routes::get_global_route_manager;
use crate::core::routes::http_routes::EdgionHttp;
use crate::core::routes::tcp_routes::{EdgionTcp, get_global_tcp_route_manager};
use crate::core::routes::tls_routes::{EdgionTls, get_global_tls_route_manager};
use crate::core::routes::udp_routes::{EdgionUdp, get_global_udp_route_manager};
use crate::core::tls::tls_pingora::TlsCallback;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::resources::gateway::Listener;

/// Annotation key to control HTTP/2 support
/// Set to "false" to disable HTTP/2 (both h2c and ALPN)
/// Default: "true" (enabled)
pub const ANNOTATION_ENABLE_HTTP2: &str = "edgion.com/enable-http2";

/// Annotation key to specify backend protocol for TLS listeners
/// Set to "tcp" for TLS terminate to TCP backend
pub const ANNOTATION_BACKEND_PROTOCOL: &str = "edgion.io/backend-protocol";

/// Context passed to listener builders containing gateway-level information and listener config
#[derive(Clone)]
pub struct ListenerContext {
    pub gateway_class_name: Option<String>,
    pub gateway_namespace: Option<String>,
    pub gateway_name: String,
    pub gateway_key: String,
    pub listener: Listener,
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
/// This function creates an EdgionHttp proxy service and adds it to the server
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
    let host = context.listener.hostname.as_deref().unwrap_or("0.0.0.0");
    let addr = format!("{}:{}", host, context.listener.port);

    // Get or create domain routes from global RouteManager
    let route_manager = get_global_route_manager();
    let namespace_str = context.gateway_namespace.as_deref().unwrap_or("");
    let domain_routes = route_manager.get_or_create_domain_routes(namespace_str, &context.gateway_name);

    // Get or create gRPC routes for this gateway (same pattern as HTTP routes)
    let grpc_route_manager = crate::core::routes::grpc_routes::get_global_grpc_route_manager();
    let grpc_routes = grpc_route_manager.get_or_create_domain_grpc_routes(namespace_str, &context.gateway_name);

    // Pre-parse timeout configurations once at initialization
    let parsed_timeouts = crate::core::routes::http_routes::edgion_http::ParsedTimeouts::from_config(
        &context.edgion_gateway_config
    );

    // Create EdgionHttp proxy handler
    let edgion_http = EdgionHttp {
        gateway_class_name: context.gateway_class_name.clone(),
        gateway_namespace: context.gateway_namespace.clone(),
        gateway_name: context.gateway_name.clone(),
        listener: context.listener.clone(),
        server_start_time: SystemTime::now(),
        server_header_opts: Default::default(),
        domain_routes,
        grpc_routes,
        access_logger: context.access_logger.clone(),
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        parsed_timeouts,
        enable_http2: context.enable_http2,
    };

    // Create HTTP proxy service
    let mut http_service = http_proxy_service(&context.server_conf, edgion_http);

    // Enable h2c (HTTP/2 Cleartext) for non-TLS listeners if enable_http2 is true
    if !enable_tls && enable_http2 {
        if let Some(http_logic) = http_service.app_logic_mut() {
            let mut http_server_options = HttpServerOptions::default();
            http_server_options.h2c = true;  // Enable HTTP/2 without TLS
            http_logic.server_options = Some(http_server_options);
            tracing::info!(
                gateway=%context.gateway_key,
                listener=%listener_name,
                "Enabled h2c (HTTP/2 Cleartext) support"
            );
        }
    }

    // Add listener with or without TLS
    if enable_tls {
        let mut tls_settings = TlsCallback::new_tls_settings_with_callback(true)?;
        // Enable HTTP/2 for HTTPS if enable_http2 is true
        if enable_http2 {
            tls_settings.enable_h2();
        }
        http_service.add_tls_with_settings(&addr, None, tls_settings);
        let protocol = if enable_http2 { "HTTPS (HTTP/2 enabled)" } else { "HTTPS" };
        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            addr=%addr,
            protocol=%protocol,
            "Adding TLS listener"
        );
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
#[allow(dead_code)]
pub fn add_tcp_listener(
    server: &mut Server,
    context: &ListenerContext,
) -> Result<()> {
    let listener_name = context.listener.name.clone();
    let host = context.listener.hostname.as_deref().unwrap_or("0.0.0.0");
    let addr = format!("{}:{}", host, context.listener.port);
    let port = context.listener.port as u16;
    
    // 预获取该 gateway 的 TCP 路由（类似 HTTP 的方式）
    let tcp_route_manager = get_global_tcp_route_manager();
    let namespace_str = context.gateway_namespace.as_deref().unwrap_or("");
    let gateway_tcp_routes = tcp_route_manager.get_or_create_gateway_tcp_routes(
        namespace_str,
        &context.gateway_name
    );
    
    // 创建 EdgionTcp
    let edgion_tcp = EdgionTcp {
        gateway_name: context.gateway_name.clone(),
        gateway_namespace: context.gateway_namespace.clone(),
        listener_port: port,
        gateway_tcp_routes,  // 传入预获取的路由
        access_logger: context.access_logger.clone(),
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        connector: TransportConnector::new(None),
    };
    
    // 创建 TCP 服务
    let tcp_service = Service::with_listeners(
        format!("TCP-{}", listener_name),
        Listeners::tcp(&addr),
        edgion_tcp,
    );
    
    // 添加到 server
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
/// UDP listeners don't use Pingora's Service abstraction - they run as independent tokio tasks
#[allow(dead_code)]
pub fn add_udp_listener(
    _server: &mut Server,
    context: &ListenerContext,
) -> Result<()> {
    let listener_name = context.listener.name.clone();
    let host = context.listener.hostname.as_deref().unwrap_or("0.0.0.0");
    let addr = format!("{}:{}", host, context.listener.port);
    let port = context.listener.port as u16;
    
    // Get UDP routes for this gateway
    let udp_route_manager = get_global_udp_route_manager();
    let namespace_str = context.gateway_namespace.as_deref().unwrap_or("");
    let gateway_udp_routes = udp_route_manager.get_or_create_gateway_udp_routes(
        namespace_str,
        &context.gateway_name
    );
    
    // Create UDP socket
    // Note: This is blocking, but it's called during server initialization
    let socket = std::net::UdpSocket::bind(&addr)?;
    socket.set_nonblocking(true)?;
    let socket = UdpSocket::from_std(socket)?;
    
    // Create EdgionUdp service
    let edgion_udp = Arc::new(EdgionUdp::new(
        context.gateway_name.clone(),
        context.gateway_namespace.clone(),
        port,
        gateway_udp_routes,
        context.edgion_gateway_config.clone(),
        socket,
    ));
    
    // Start UDP service in a background task
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
pub fn add_tls_terminate_to_tcp_listener(
    server: &mut Server,
    context: &ListenerContext,
) -> Result<()> {
    let listener_name = context.listener.name.clone();
    let host = context.listener.hostname.as_deref().unwrap_or("0.0.0.0");
    let addr = format!("{}:{}", host, context.listener.port);
    let port = context.listener.port as u16;
    
    // Get TLS routes for this gateway
    let tls_route_manager = get_global_tls_route_manager();
    let namespace_str = context.gateway_namespace.as_deref().unwrap_or("");
    let gateway_tls_routes = tls_route_manager.get_or_create_gateway_tls_routes(
        namespace_str,
        &context.gateway_name
    );
    
    // Create EdgionTls service
    let edgion_tls = EdgionTls {
        gateway_name: context.gateway_name.clone(),
        gateway_namespace: context.gateway_namespace.clone(),
        listener_port: port,
        gateway_tls_routes,
        access_logger: context.access_logger.clone(),
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        connector: TransportConnector::new(None),
    };
    
    // Create TLS settings with callback for certificate loading
    let tls_settings = TlsCallback::new_tls_settings_with_callback(false)?;
    
    // Create TLS service with Listeners
    let mut tls_service = Service::with_listeners(
        format!("TLS-TCP-{}", listener_name),
        Listeners::tcp(&addr),
        edgion_tls,
    );
    
    // Add TLS settings to the service
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

/// Main entry point for adding a listener to the server
///
/// This function dispatches to the appropriate listener builder based on the
/// protocol specified in the listener configuration.
pub fn add_listener(
    server: &mut Server,
    context: ListenerContext,
) -> Result<()> {
    match context.listener.protocol.to_uppercase().as_str() {
        "HTTP" => {
            add_http_listener(server, &context, false, context.enable_http2)
        }
        "HTTPS" => {
            add_http_listener(server, &context, true, context.enable_http2)
        }
        "TCP" => {
            add_tcp_listener(server, &context)
        }
        "UDP" => {
            add_udp_listener(server, &context)
        }
        "TLS" => {
            // Check Gateway annotation for backend protocol
            let backend_protocol = context.gateway_annotations
                .get(ANNOTATION_BACKEND_PROTOCOL)
                .map(|s| s.as_str());
            
            match backend_protocol {
                Some("tcp") => add_tls_terminate_to_tcp_listener(server, &context),
                _ => anyhow::bail!(
                    "TLS protocol requires '{}' annotation set to 'tcp'",
                    ANNOTATION_BACKEND_PROTOCOL
                ),
            }
        }
        "GRPC" | "GRPCWeb"=> {
            // GRPC always requires HTTP/2
            tracing::info!(
                listener=%context.listener.name,
                "GRPC/GRPCWeb protocol detected, treating as HTTP/2 with TLS (force enabled)"
            );
            add_http_listener(server, &context, true, true)  // Force enable HTTP/2 for gRPC
        }
        protocol => {
            anyhow::bail!("Unsupported protocol: {}", protocol)
        }
    }
}

