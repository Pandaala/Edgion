pub mod grpc;
pub mod http;
pub mod tcp;
pub mod tls;
pub mod udp;

// Re-export commonly used types for convenience
pub use http::{
    create_route_manager_handler,
    get_global_route_manager,
    DomainRouteRules,
    EdgionHttpProxy,
    HttpRouteRuleUnit,
    RouteManager,
};

pub use grpc::{create_grpc_route_handler, get_global_grpc_route_manager, GrpcRouteManager};
pub use tcp::{create_tcp_route_handler, get_global_tcp_route_manager, EdgionTcpProxy, TcpRouteManager};
pub use tls::{
    create_tls_route_handler, get_global_tls_route_managers, EdgionTlsTcpProxy, GlobalTlsRouteManagers,
    TlsRouteManager,
};
pub use udp::{create_udp_route_handler, get_global_udp_route_manager, EdgionUdpProxy, UdpRouteManager};
