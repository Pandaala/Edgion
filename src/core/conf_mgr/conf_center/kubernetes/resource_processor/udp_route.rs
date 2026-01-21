//! UDPRoute Processor
//!
//! Handles UDPRoute resources

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::UDPRoute;

/// UDPRoute processor
pub struct UdpRouteProcessor;

impl UdpRouteProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UdpRouteProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<UDPRoute> for UdpRouteProcessor {
    fn kind(&self) -> &'static str {
        "UDPRoute"
    }

    fn parse(&self, route: UDPRoute, _ctx: &ProcessContext) -> ProcessResult<UDPRoute> {
        // TODO: 后续可添加 ref_grant 验证等逻辑
        ProcessResult::Continue(route)
    }

    fn save(&self, cs: &ConfigServer, route: UDPRoute) {
        cs.udp_routes.apply_change(ResourceChange::EventUpdate, route);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.udp_routes.get_by_key(key) {
            cs.udp_routes.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<UDPRoute> {
        cs.udp_routes.get_by_key(key)
    }
}
