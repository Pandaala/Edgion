//! ResourceMeta implementation for EdgionPlugins

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::EdgionPlugins;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for EdgionPlugins {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }

    fn resource_kind() -> ResourceKind {
        ResourceKind::EdgionPlugins
    }

    fn kind_name() -> &'static str {
        "EdgionPlugins"
    }

    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }

    fn pre_parse(&mut self) {
        // Initialize plugin runtime from edgion_plugins
        self.init_plugin_runtime();
    }
}
