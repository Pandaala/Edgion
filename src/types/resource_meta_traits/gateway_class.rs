//! ResourceMeta implementation for GatewayClass

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::GatewayClass;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for GatewayClass {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::GatewayClass
    }
    
    fn kind_name() -> &'static str {
        "GatewayClass"
    }
    
    fn key_name(&self) -> String {
        // GatewayClass is cluster-scoped, so no namespace
        self.metadata.name.as_deref().unwrap_or("").to_string()
    }
}

