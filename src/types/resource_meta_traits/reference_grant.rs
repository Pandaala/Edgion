//! ResourceMeta implementation for ReferenceGrant

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::ReferenceGrant;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for ReferenceGrant {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }

    fn resource_kind() -> ResourceKind {
        ResourceKind::ReferenceGrant
    }

    fn kind_name() -> &'static str {
        "ReferenceGrant"
    }

    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }

    fn pre_parse(&mut self) {
        // No pre-parsing needed for ReferenceGrant at this stage
        // Validation logic will be added in future iterations
    }
}
