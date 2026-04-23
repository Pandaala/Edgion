//! PluginMetaData resource definition
//!
//! PluginMetaData defines named metadata resources referenced by EdgionPlugins.
//! Each PluginMetaData contains exactly one `MetaDataEntry` variant:
//!
//! - `KeyList`: Key-value list with exact or regex matching (for plugins like IpRestriction, KeyAuth)
//! - `IpList`: IP address / CIDR list (for IP-based filtering)
//! - `ClusterRegionRoute`: Full region topology and routing rules
//! - `ServiceRegionRoute`: Per-service failover overrides referencing a ClusterRegionRoute
//! - `GlobalConnectionIpRestriction`: L4 TCP connection-phase IP restriction with profile switching

pub mod global_connection_ip_restriction;
pub mod ip_list;
pub mod key_list;
pub mod region_route;

pub use global_connection_ip_restriction::{GlobalConnectionIpRestrictionData, IpGroup, ProfileRules};
pub use ip_list::IpListData;
pub use key_list::{KeyListData, KeyMatchMode, MetaDataItem};
pub use region_route::{ClusterRegionRouteConfig, RouteByKeyConfMatchMeta, ServiceRegionDef, ServiceRegionRouteConfig};

use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
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
    namespaced,
    status = "PluginMetaDataStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct PluginMetaDataSpec {
    /// The metadata entry. Each PluginMetaData contains exactly one entry,
    /// discriminated by the `type` tag.
    pub metadata: MetaDataEntry,
}

/// Status of PluginMetaData.
///
/// Conditions follow Gateway API conventions:
/// - `Accepted`: spec is syntactically and semantically valid (per-variant validation passed).
/// - `ResolvedRefs`: cross-resource references resolve. Currently only meaningful for the
///   `ServiceRegionRoute` variant (its `clusterRef` must point at an existing
///   `ClusterRegionRoute` PluginMetaData). All other variants always report `True`
///   since they carry no external references.
#[derive(Default, Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
pub struct PluginMetaDataStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
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

    /// L4 TCP connection-phase IP restriction with profile switching. Center-managed.
    GlobalConnectionIpRestriction(GlobalConnectionIpRestrictionData),
}

impl PluginMetaData {
    /// Get the data type name of this metadata entry.
    pub fn data_type(&self) -> &str {
        match &self.spec.metadata {
            MetaDataEntry::KeyList(_) => "KeyList",
            MetaDataEntry::IpList(_) => "IpList",
            MetaDataEntry::ClusterRegionRoute(_) => "ClusterRegionRoute",
            MetaDataEntry::ServiceRegionRoute(_) => "ServiceRegionRoute",
            MetaDataEntry::GlobalConnectionIpRestriction(_) => "GlobalConnectionIpRestriction",
        }
    }

    /// Validate the metadata payload and return a list of human-readable error messages.
    ///
    /// An empty result means "no problems found". Errors are suitable for inclusion in an
    /// `Accepted=False/reason=Invalid` status condition by the Controller, and are also
    /// emitted as warnings by the data-plane `pre_parse` hook (see `impl_resource_meta!`).
    pub fn validate_pre_parse(&self) -> Vec<String> {
        let mut errors = Vec::new();
        match &self.spec.metadata {
            MetaDataEntry::IpList(ip_list) => {
                for cidr in &ip_list.items {
                    if !cidr.contains('/')
                        && !cidr.contains(':')
                        && !cidr.chars().all(|c| c.is_ascii_digit() || c == '.')
                    {
                        errors.push(format!("Invalid IP/CIDR format: {}", cidr));
                    }
                }
            }
            MetaDataEntry::KeyList(key_list) => {
                if matches!(key_list.match_mode, KeyMatchMode::Regex) {
                    for item in &key_list.items {
                        if let Err(e) = regex::Regex::new(&item.key) {
                            errors.push(format!("Invalid regex pattern '{}': {}", item.key, e));
                        }
                    }
                }
            }
            MetaDataEntry::GlobalConnectionIpRestriction(data) => {
                // Early validation: clone and run full validate_and_init to surface errors.
                let mut cloned = data.clone();
                if let Err(e) = cloned.validate_and_init() {
                    errors.push(format!("GlobalConnectionIpRestriction invalid: {}", e));
                }
            }
            // RegionRoute variants: no additional validation at pre-parse (cross-PM
            // reference checks live in the Controller handler's update_status via
            // ResolvedRefs, not here).
            MetaDataEntry::ClusterRegionRoute(_) | MetaDataEntry::ServiceRegionRoute(_) => {}
        }
        errors
    }

    /// Return the namespace/name key string for this resource ("name" if cluster-scoped).
    pub fn key_name_str(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
}
