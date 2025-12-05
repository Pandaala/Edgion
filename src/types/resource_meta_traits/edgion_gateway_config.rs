//! ResourceMeta implementation for EdgionGatewayConfig

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::EdgionGatewayConfig;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for EdgionGatewayConfig {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::EdgionGatewayConfig
    }
    
    fn kind_name() -> &'static str {
        "EdgionGatewayConfig"
    }
    
    fn key_name(&self) -> String {
        // EdgionGatewayConfig is cluster-scoped, so no namespace
        self.metadata.name.as_deref().unwrap_or("").to_string()
    }
}

