//! GRPCRoute Processor
//!
//! Handles GRPCRoute resources

use super::{ProcessContext, ProcessResult, ResourceProcessor};
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

    fn parse(&self, route: GRPCRoute, _ctx: &ProcessContext) -> ProcessResult<GRPCRoute> {
        // TODO: 后续可添加 ref_grant 验证等逻辑
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
