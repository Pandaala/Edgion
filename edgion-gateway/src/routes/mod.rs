pub(crate) mod common;
pub mod grpc;
pub mod http;
mod l4_duplex;
mod l4_partial_update;
mod l4_rebuild;
pub mod tcp;
pub mod tls;
pub mod udp;

pub use http::{
    create_route_manager_handler, get_global_http_route_managers, DomainRouteRules, EdgionHttpProxy,
    GlobalHttpRouteManagers, HttpRouteRuleUnit,
};

pub use grpc::{create_grpc_route_handler, get_global_grpc_route_managers, GlobalGrpcRouteManagers};
pub use tcp::{create_tcp_route_handler, get_global_tcp_route_managers, EdgionTcpProxy, GlobalTcpRouteManagers};
pub use tls::{
    create_tls_route_handler, get_global_tls_route_managers, EdgionTlsTcpProxy, GlobalTlsRouteManagers, TlsRouteManager,
};
pub use udp::{create_udp_route_handler, get_global_udp_route_managers, EdgionUdpProxy, GlobalUdpRouteManagers};
