//! HTTPRoute Processor
//!
//! Handles HTTPRoute resources with ReferenceGrant validation

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server_old::ConfigServer;
use crate::core::ref_grant::validate_http_route_if_enabled;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::HTTPRoute;

/// HTTPRoute processor
pub struct HttpRouteProcessor;

impl HttpRouteProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HttpRouteProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<HTTPRoute> for HttpRouteProcessor {
    fn kind(&self) -> &'static str {
        "HTTPRoute"
    }

    fn validate(&self, route: &HTTPRoute, _ctx: &ProcessContext) -> Vec<String> {
        validate_http_route_if_enabled(route)
    }

    fn parse(&self, route: HTTPRoute, _ctx: &ProcessContext) -> ProcessResult<HTTPRoute> {
        ProcessResult::Continue(route)
    }

    fn save(&self, cs: &ConfigServer, route: HTTPRoute) {
        cs.routes.apply_change(ResourceChange::EventUpdate, route);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.routes.get_by_key(key) {
            cs.routes.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<HTTPRoute> {
        cs.routes.get_by_key(key)
    }
}
