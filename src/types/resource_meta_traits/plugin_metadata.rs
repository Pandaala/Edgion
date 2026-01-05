//! ResourceMeta implementation for PluginMetaData

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::PluginMetaData;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for PluginMetaData {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }

    fn resource_kind() -> ResourceKind {
        ResourceKind::PluginMetaData
    }

    fn kind_name() -> &'static str {
        "PluginMetaData"
    }

    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }

    fn pre_parse(&mut self) {
        // Validate that only one data type is set
        if let Err(e) = self.validate() {
            tracing::warn!("PluginMetaData validation failed for {}: {}", self.key_name(), e);
        }

        // Optional: Validate CIDR formats in IpList
        if let Some(ip_list) = &self.spec.ip_list {
            for cidr in &ip_list.items {
                // Basic validation - could be enhanced with ipnetwork crate
                if !cidr.contains('/') && !cidr.contains(':') && !cidr.chars().all(|c| c.is_ascii_digit() || c == '.') {
                    tracing::warn!("PluginMetaData {}: Invalid IP/CIDR format: {}", self.key_name(), cidr);
                }
            }
        }

        // Optional: Validate regex patterns in RegexList
        if let Some(regex_list) = &self.spec.regex_list {
            for item in &regex_list.items {
                if let Err(e) = regex::Regex::new(&item.key) {
                    tracing::warn!(
                        "PluginMetaData {}: Invalid regex pattern '{}': {}",
                        self.key_name(),
                        item.key,
                        e
                    );
                }
            }
        }
    }
}
