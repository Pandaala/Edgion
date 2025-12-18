//! gRPC Routes Module
//!
//! This module implements gRPC and gRPC-Web routing based on GatewayAPI GRPCRoute CRD.
//! It provides service/method based routing with minimal integration into http_routes.

mod match_unit;
mod match_engine;
mod routes_mgr;
mod integration;
mod conf_handler_impl;

// Export core types
pub use match_unit::{GrpcRouteRuleUnit, GrpcMatchInfo};
pub use match_engine::GrpcMatchEngine;
pub use routes_mgr::{
    GrpcRouteManager,
    GrpcRouteRules,
    DomainGrpcRouteRules,
    get_global_grpc_route_manager,
};
pub use conf_handler_impl::create_grpc_route_handler;

// Export integration interface (for http_routes to call)
pub use integration::{
    is_grpc_protocol,
    try_match_grpc_route,
    handle_grpc_upstream,
    run_grpc_route_plugins,
};

