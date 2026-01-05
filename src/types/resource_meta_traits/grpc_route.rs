//! ResourceMeta implementation for GRPCRoute

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::GRPCRoute;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for GRPCRoute {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }

    fn resource_kind() -> ResourceKind {
        ResourceKind::GRPCRoute
    }

    fn kind_name() -> &'static str {
        "GRPCRoute"
    }

    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }

    fn pre_parse(&mut self) {
        // Parse extension_ref in backend_refs to populate extension_info (if needed in the future)
        // For now, GRPCRoute follows the same pattern as HTTPRoute
        self.preparse();

        // Parse timeouts to populate parsed_timeouts in rules
        self.parse_timeouts();
    }
}
