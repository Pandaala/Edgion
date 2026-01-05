pub mod grpc_routes;
pub mod http_routes;
pub mod tcp_routes;
pub mod tls_routes;
pub mod udp_routes;

// Re-export commonly used types for convenience
pub use http_routes::{
    create_route_manager_handler,
    get_global_route_manager,
    DomainRouteRules,
    EdgionHttp, // HTTP proxy type
    HttpRouteRuleUnit,
    RouteManager,
};

pub use tcp_routes::{
    create_tcp_route_handler,
    get_global_tcp_route_manager,
    EdgionTcp, // TCP proxy type
    TcpRouteManager,
};

pub use udp_routes::{
    create_udp_route_handler,
    get_global_udp_route_manager,
    EdgionUdp, // UDP proxy type
    UdpRouteManager,
};

pub use grpc_routes::{create_grpc_route_handler, get_global_grpc_route_manager, GrpcRouteManager};

pub use tls_routes::{
    create_tls_route_handler,
    get_global_tls_route_manager,
    EdgionTls, // TLS proxy type
    TlsRouteManager,
};
