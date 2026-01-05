//! ResourceMeta implementation for Secret

use k8s_openapi::api::core::v1::Secret;

use crate::types::resource_kind::ResourceKind;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for Secret {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }

    fn resource_kind() -> ResourceKind {
        ResourceKind::Secret
    }

    fn kind_name() -> &'static str {
        "Secret"
    }

    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
}
