//! PluginMetaData resource definition
//!
//! PluginMetaData defines named metadata resources referenced by EdgionPlugins.
//! Each PluginMetaData contains exactly one `MetaDataEntry` variant:
//!
//! - `KeyList`: Key-value list with exact or regex matching (for plugins like IpRestriction, KeyAuth)
//! - `IpList`: IP address / CIDR list (for IP-based filtering)
//! - `ClusterRegionRoute`: Full region topology and routing rules
//! - `ServiceRegionRoute`: Per-service failover overrides referencing a ClusterRegionRoute

pub mod global_connection_ip_restriction;
pub mod ip_list;
pub mod key_list;
pub mod region_route;

pub use global_connection_ip_restriction::{
    GlobalConnectionIpRestrictionData, IpGroup, ProfileRules,
};
pub use ip_list::IpListData;
pub use key_list::{KeyListData, KeyMatchMode, MetaDataItem};
pub use region_route::{ClusterRegionRouteConfig, RouteByKeyConfMatchMeta, ServiceRegionDef, ServiceRegionRouteConfig};

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Kind for PluginMetaData
pub const PLUGIN_METADATA_KIND: &str = "PluginMetaData";

/// PluginMetaData defines named metadata resources referenced by EdgionPlugins.
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "PluginMetaData",
    plural = "pluginmetadata",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct PluginMetaDataSpec {
    /// The metadata entry. Each PluginMetaData contains exactly one entry,
    /// discriminated by the `type` tag.
    pub metadata: MetaDataEntry,
}

/// A single metadata entry — the core payload of a PluginMetaData resource.
///
/// Discriminated by `type` tag in serialized form:
/// ```yaml
/// metadata:
///   type: KeyList
///   config:
///     matchMode: exact
///     items: [...]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config")]
pub enum MetaDataEntry {
    /// Key-value list with configurable match mode (exact string or regex).
    /// Merges the legacy StringList and RegexList into a single type.
    KeyList(KeyListData),

    /// IP address / CIDR range list for IP-based filtering.
    IpList(IpListData),

    /// Cluster RegionRoute: complete topology + route rules + matchMap.
    /// Created by operators / Center.
    ClusterRegionRoute(ClusterRegionRouteConfig),

    /// Service-level RegionRoute: failover overrides with clusterRef.
    /// Created by administrator.
    ServiceRegionRoute(ServiceRegionRouteConfig),
}

impl PluginMetaData {
    /// Get the data type name of this metadata entry.
    pub fn data_type(&self) -> &str {
        match &self.spec.metadata {
            MetaDataEntry::KeyList(_) => "KeyList",
            MetaDataEntry::IpList(_) => "IpList",
            MetaDataEntry::ClusterRegionRoute(_) => "ClusterRegionRoute",
            MetaDataEntry::ServiceRegionRoute(_) => "ServiceRegionRoute",
        }
    }

    /// Validate and pre-parse the metadata (called during pre_parse).
    pub fn validate_pre_parse(&self) {
        let key_name = self.key_name_str();

        match &self.spec.metadata {
            MetaDataEntry::IpList(ip_list) => {
                for cidr in &ip_list.items {
                    if !cidr.contains('/')
                        && !cidr.contains(':')
                        && !cidr.chars().all(|c| c.is_ascii_digit() || c == '.')
                    {
                        tracing::warn!("PluginMetaData {}: Invalid IP/CIDR format: {}", key_name, cidr);
                    }
                }
            }
            MetaDataEntry::KeyList(key_list) => {
                if matches!(key_list.match_mode, KeyMatchMode::Regex) {
                    for item in &key_list.items {
                        if let Err(e) = regex::Regex::new(&item.key) {
                            tracing::warn!(
                                "PluginMetaData {}: Invalid regex pattern '{}': {}",
                                key_name,
                                item.key,
                                e
                            );
                        }
                    }
                }
            }
            // RegionRoute variants: no additional validation needed at pre-parse
            MetaDataEntry::ClusterRegionRoute(_) | MetaDataEntry::ServiceRegionRoute(_) => {}
        }
    }

    /// Get key name string (namespace/name format)
    fn key_name_str(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
}
