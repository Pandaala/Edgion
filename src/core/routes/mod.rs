pub mod http_routes;
pub mod tcp_routes;

// Re-export commonly used types for convenience
pub use http_routes::{
    get_global_route_manager,
    DomainRouteRules,
    RouteManager,
    HttpRouteRuleUnit,
    create_route_manager_handler,
};

pub use tcp_routes::{
    get_global_tcp_route_manager,
    TcpRouteManager,
};
