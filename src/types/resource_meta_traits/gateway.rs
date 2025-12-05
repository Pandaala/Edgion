//! ResourceMeta implementation for Gateway

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::Gateway;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for Gateway {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::Gateway
    }
    
    fn kind_name() -> &'static str {
        "Gateway"
    }
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
}

