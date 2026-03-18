//! gRPC Routes Module
//!
//! This module implements gRPC and gRPC-Web routing based on GatewayAPI GRPCRoute CRD.
//! It provides service/method based routing with minimal integration into the HTTP pipeline.

mod conf_handler_impl;
mod integration;
mod match_engine;
mod match_unit;
mod routes_mgr;

// Export core types
pub use conf_handler_impl::create_grpc_route_handler;
pub use match_engine::GrpcMatchEngine;
pub use match_unit::{GrpcMatchInfo, GrpcRouteInfo, GrpcRouteRuleUnit};
pub use routes_mgr::{
    get_global_grpc_route_manager, get_global_grpc_route_managers, DomainGrpcRouteRules, GlobalGrpcRouteManagers,
    GrpcRouteManager, GrpcRouteManagerStats, GrpcRouteRules,
};

// Export integration interface for the HTTP pipeline to call.
pub use integration::{handle_grpc_upstream, is_grpc_protocol, try_match_grpc_route};
