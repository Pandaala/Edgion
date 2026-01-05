//! ResourceMeta implementation for BackendTLSPolicy

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::BackendTLSPolicy;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for BackendTLSPolicy {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }

    fn resource_kind() -> ResourceKind {
        ResourceKind::BackendTLSPolicy
    }

    fn kind_name() -> &'static str {
        "BackendTLSPolicy"
    }

    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }

    fn pre_parse(&mut self) {
        // No pre-parsing needed for BackendTLSPolicy at this stage
        // TLS configuration processing will be added when implementing actual backend TLS
    }
}
