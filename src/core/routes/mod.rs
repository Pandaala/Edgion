pub mod http_routes;
pub mod tcp_routes;
pub mod udp_routes;
pub mod grpc_routes;

// Re-export commonly used types for convenience
pub use http_routes::{
    get_global_route_manager,
    DomainRouteRules,
    RouteManager,
    HttpRouteRuleUnit,
    create_route_manager_handler,
    EdgionHttp,  // HTTP proxy type
};

pub use tcp_routes::{
    get_global_tcp_route_manager,
    TcpRouteManager,
    create_tcp_route_handler,
    EdgionTcp,  // TCP proxy type
};

pub use udp_routes::{
    get_global_udp_route_manager,
    UdpRouteManager,
    create_udp_route_handler,
    EdgionUdp,  // UDP proxy type
};

pub use grpc_routes::{
    get_global_grpc_route_manager,
    GrpcRouteManager,
    create_grpc_route_handler,
};
