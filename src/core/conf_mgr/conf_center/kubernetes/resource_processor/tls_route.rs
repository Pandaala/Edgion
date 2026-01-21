//! TLSRoute Processor
//!
//! Handles TLSRoute resources

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::TLSRoute;

/// TLSRoute processor
pub struct TlsRouteProcessor;

impl TlsRouteProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TlsRouteProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<TLSRoute> for TlsRouteProcessor {
    fn kind(&self) -> &'static str {
        "TLSRoute"
    }

    fn parse(&self, route: TLSRoute, _ctx: &ProcessContext) -> ProcessResult<TLSRoute> {
        // TODO: 后续可添加 ref_grant 验证等逻辑
        ProcessResult::Continue(route)
    }

    fn save(&self, cs: &ConfigServer, route: TLSRoute) {
        cs.tls_routes.apply_change(ResourceChange::EventUpdate, route);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.tls_routes.get_by_key(key) {
            cs.tls_routes.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<TLSRoute> {
        cs.tls_routes.get_by_key(key)
    }
}
