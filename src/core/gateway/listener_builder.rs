//! Listener builder module for adding different types of listeners to Pingora Server
//!
//! This module provides a clean abstraction for adding HTTP/HTTPS, TCP, UDP, and other
//! protocol listeners to the gateway server. It separates listener construction logic
//! from the main gateway bootstrap process.

use anyhow::Result;
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::Server;
use pingora_proxy::http_proxy_service;
use std::sync::Arc;
use std::time::SystemTime;

use crate::core::routes::http_routes::EdgionHttp;
use crate::core::observe::AccessLogger;
use crate::core::tls::tls_pingora::TlsCallback;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::resources::gateway::Listener;

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
}

/// Add an HTTP or HTTPS listener to the Pingora server
///
/// This function creates an EdgionHttp proxy service and adds it to the server
/// with or without TLS based on the enable_tls parameter.
pub fn add_http_listener(
    server: &mut Server,
    context: &ListenerContext,
    enable_tls: bool,
) -> Result<()> {
    use crate::core::routes::get_global_route_manager;
    
    let listener_name = context.listener.name.clone();
    let host = context.listener.hostname.as_deref().unwrap_or("0.0.0.0");
    let addr = format!("{}:{}", host, context.listener.port);

    // Get or create domain routes from global RouteManager
    let route_manager = get_global_route_manager();
    let namespace_str = context.gateway_namespace.as_deref().unwrap_or("");
    let domain_routes = route_manager.get_or_create_domain_routes(namespace_str, &context.gateway_name);

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
        access_logger: context.access_logger.clone(),
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        parsed_timeouts,
    };

    // Create HTTP proxy service
    let mut http_service = http_proxy_service(&context.server_conf, edgion_http);

    // Add listener with or without TLS
    if enable_tls {
        let tls_settings = TlsCallback::new_tls_settings_with_callback(true)?;
        http_service.add_tls_with_settings(&addr, None, tls_settings);
        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            addr=%addr,
            protocol="HTTPS",
            "Adding TLS listener"
        );
    } else {
        http_service.add_tcp(&addr);
        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            addr=%addr,
            protocol="HTTP",
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
    use pingora_core::services::listening::Service;
    use pingora_core::listeners::Listeners;
    use pingora_core::connectors::TransportConnector;
    use crate::core::routes::tcp_routes::EdgionTcp;
    use crate::core::routes::tcp_routes::get_global_tcp_route_manager;
    
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
    use crate::core::routes::udp_routes::{EdgionUdp, get_global_udp_route_manager};
    use tokio::net::UdpSocket;
    
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
            add_http_listener(server, &context, false)
        }
        "HTTPS" => {
            add_http_listener(server, &context, true)
        }
        "TCP" => {
            add_tcp_listener(server, &context)
        }
        "UDP" => {
            add_udp_listener(server, &context)
        }
        "GRPC" => {
            // GRPC is essentially HTTP/2, so treat it as HTTPS
            tracing::info!(
                listener=%context.listener.name,
                "GRPC protocol detected, treating as HTTP/2 with TLS"
            );
            add_http_listener(server, &context, true)
        }
        protocol => {
            anyhow::bail!("Unsupported protocol: {}", protocol)
        }
    }
}

