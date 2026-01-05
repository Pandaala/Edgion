//! ResourceMeta implementation for TCPRoute

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::TCPRoute;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for TCPRoute {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }

    fn resource_kind() -> ResourceKind {
        ResourceKind::TCPRoute
    }

    fn kind_name() -> &'static str {
        "TCPRoute"
    }

    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }

    fn pre_parse(&mut self) {
        // TCPRoute does not require special pre-parsing at this time
        // Can be extended in the future if needed
    }
}
