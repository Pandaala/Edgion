//! GRPCRoute Processor
//!
//! Handles GRPCRoute resources with ReferenceGrant validation

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_mgr::resource_check::validate_grpc_route;
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::GRPCRoute;

/// GRPCRoute processor
pub struct GrpcRouteProcessor;

impl GrpcRouteProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GrpcRouteProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<GRPCRoute> for GrpcRouteProcessor {
    fn kind(&self) -> &'static str {
        "GRPCRoute"
    }

    fn validate(&self, route: &GRPCRoute, _ctx: &ProcessContext) -> Vec<String> {
        validate_grpc_route(route)
    }

    fn parse(&self, route: GRPCRoute, _ctx: &ProcessContext) -> ProcessResult<GRPCRoute> {
        ProcessResult::Continue(route)
    }

    fn save(&self, cs: &ConfigServer, route: GRPCRoute) {
        cs.grpc_routes.apply_change(ResourceChange::EventUpdate, route);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.grpc_routes.get_by_key(key) {
            cs.grpc_routes.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<GRPCRoute> {
        cs.grpc_routes.get_by_key(key)
    }
}
