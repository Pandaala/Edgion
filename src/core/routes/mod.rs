pub mod http_routes;
pub mod tcp_routes;

// Re-export commonly used types for convenience
pub use http_routes::{
    get_global_route_manager,
    DomainRouteRules,
    RouteManager,
    HttpRouteRuleUnit,
    create_route_manager_handler,
    EdgionHttp,  // HTTP 代理类型
};

pub use tcp_routes::{
    get_global_tcp_route_manager,
    TcpRouteManager,
    create_tcp_route_handler,
};
