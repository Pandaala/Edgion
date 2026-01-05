//! ResourceMeta implementation for EdgionStreamPlugins

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::EdgionStreamPlugins;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for EdgionStreamPlugins {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }

    fn resource_kind() -> ResourceKind {
        ResourceKind::EdgionStreamPlugins
    }

    fn kind_name() -> &'static str {
        "EdgionStreamPlugins"
    }

    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }

    fn pre_parse(&mut self) {
        // Initialize plugin runtime from stream plugins
        self.init_stream_plugin_runtime();
    }
}
