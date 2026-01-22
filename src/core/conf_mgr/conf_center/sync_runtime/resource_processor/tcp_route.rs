//! TCPRoute Processor
//!
//! Handles TCPRoute resources

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server::ConfigServer;
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

    fn parse(&self, route: TCPRoute, _ctx: &ProcessContext) -> ProcessResult<TCPRoute> {
        // TODO: 后续可添加 ref_grant 验证等逻辑
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
