//! ResourceMeta implementation for TLSRoute

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::TLSRoute;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for TLSRoute {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::TLSRoute
    }
    
    fn kind_name() -> &'static str {
        "TLSRoute"
    }
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
    
    fn pre_parse(&mut self) {
        // TLSRoute does not require special pre-parsing at this time
        // Can be extended in the future if needed
    }
}

