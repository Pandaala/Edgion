//! TLSRoute Processor
//!
//! Handles TLSRoute resources with ReferenceGrant validation

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::ref_grant::validate_tls_route_if_enabled;
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

    fn validate(&self, route: &TLSRoute, _ctx: &ProcessContext) -> Vec<String> {
        validate_tls_route_if_enabled(route)
    }

    fn parse(&self, route: TLSRoute, _ctx: &ProcessContext) -> ProcessResult<TLSRoute> {
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
