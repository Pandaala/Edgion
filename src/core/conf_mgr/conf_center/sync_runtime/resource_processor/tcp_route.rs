//! TCPRoute Processor
//!
//! Handles TCPRoute resources with ReferenceGrant validation

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::ref_grant::validate_tcp_route_if_enabled;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::TCPRoute;

/// TCPRoute processor
pub struct TcpRouteProcessor;

impl TcpRouteProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TcpRouteProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<TCPRoute> for TcpRouteProcessor {
    fn kind(&self) -> &'static str {
        "TCPRoute"
    }

    fn validate(&self, route: &TCPRoute, _ctx: &ProcessContext) -> Vec<String> {
        validate_tcp_route_if_enabled(route)
    }

    fn parse(&self, route: TCPRoute, _ctx: &ProcessContext) -> ProcessResult<TCPRoute> {
        ProcessResult::Continue(route)
    }

    fn save(&self, cs: &ConfigServer, route: TCPRoute) {
        cs.tcp_routes.apply_change(ResourceChange::EventUpdate, route);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.tcp_routes.get_by_key(key) {
            cs.tcp_routes.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<TCPRoute> {
        cs.tcp_routes.get_by_key(key)
    }
}
