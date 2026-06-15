//! EdgionData resource definition
//!
//! EdgionData is a general-purpose, hot-swappable metadata/overlay primitive.
//! Each EdgionData contains exactly one `DataEntry` variant:
//!
//! | Group | Variant | Notes |
//! |-------|---------|-------|
//! | Standard primitive | `KeyList` | Unified string+regex list |
//! | Standard primitive | `IpList` | CIDR range list |
//! | Legacy full-config | `ClusterRegionRoute` | Full region topology |
//! | Legacy full-config | `ServiceRegionRoute` | Per-service failover |
//! | Legacy full-config | `GlobalConnectionIpRestriction` | L4 IP restriction (standalone) |
//! | First-party overlay | `GlobalConnIpRestrictionOverride` | All-Optional overlay via `overrideRef` |
//! | Escape hatch | `Misc` | Third-party / unknown payloads |
//!
//! The top-level envelope carries `enable` (kill switch, default `true`) and
//! `active` (profile selector, optional).  See [`effective_switch`].

pub mod global_connection_ip_restriction;
pub mod ip_list;
pub mod key_list;
pub mod overlay;
pub mod region_route;

pub use global_connection_ip_restriction::{
    merge_gcir_override, GlobalConnIpRestrictionOverride, GlobalConnectionIpRestrictionData, IpGroup, ProfileRules,
};
pub use ip_list::IpListData;
pub use key_list::{KeyGroup, KeyListData, KeyMatchMode, MetaDataItem};
pub use overlay::{apply_override, OverlaySignal};
pub use region_route::{
    ClusterRegionRouteConfig, ClusterRegionRouteEntry, RefPluginEntry, RouteByKeyConfMatchMeta, ServiceRegionDef,
    ServiceRegionRouteConfig, ServiceRegionRouteEntry,
};

use crate::resources::common::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn default_enable() -> bool {
    true
}

/// Kind for EdgionData
pub const EDGION_DATA_KIND: &str = "EdgionData";

/// Reference to an EdgionData resource by name (and optional namespace).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

/// EdgionData is a general-purpose, hot-swappable metadata/overlay primitive
/// referenced by EdgionPlugins and EdgionStreamPlugins.
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "EdgionData",
    plural = "edgiondata",
    namespaced,
    status = "EdgionDataStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct EdgionDataSpec {
    /// Top-level kill switch. When `false`, consumers must ignore this EdgionData's
    /// content and fall back to their own base defaults. Defaults to `true`.
    ///
    /// Additive field — no consumer reads it yet (wired in S4).
    #[serde(default = "default_enable")]
    pub enable: bool,

    /// Optional profile selector. When set, consumers that support preloaded
    /// profiles use this value to select the active profile from the base config.
    /// Consumers that do not support profiles ignore this field.
    ///
    /// Additive field — no consumer reads it yet (wired in S4/S5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,

    /// The data entry. Each EdgionData contains exactly one entry,
    /// discriminated by the `type` tag.
    pub data: DataEntry,
}

/// Status of EdgionData.
///
/// Conditions follow Gateway API conventions:
/// - `Accepted`: spec is syntactically and semantically valid (per-variant validation passed).
/// - `ResolvedRefs`: cross-resource references resolve. Currently only meaningful for the
///   `ServiceRegionRoute` variant (its `clusterRef` must point at an existing
///   `ClusterRegionRoute` EdgionData). All other variants always report `True`
///   since they carry no external references.
#[derive(Default, Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
pub struct EdgionDataStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

/// A single data entry — the core payload of an EdgionData resource.
///
/// Discriminated by `type` tag in serialized form:
/// ```yaml
/// data:
///   type: KeyList
///   config:
///     matchMode: exact
///     items: [...]
/// ```
///
/// The `Misc` variant is an escape hatch for third-party, experimental, or
/// unknown payloads whose schema is not defined here.  Its `config` is open
/// (`x-kubernetes-preserve-unknown-fields` at the CRD level).
/// **Use `Misc` only when no typed first-party variant fits the use-case.**
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config")]
pub enum DataEntry {
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

    /// L4 TCP connection-phase IP restriction with profile switching.
    /// Standalone full-config variant. Still valid for existing resources.
    GlobalConnectionIpRestriction(GlobalConnectionIpRestrictionData),

    /// Typed overlay for `GlobalConnectionIpRestriction` (D2). All fields are
    /// optional; only `Some` fields replace the corresponding base values.
    /// Referenced via `overrideRef` in `GlobalConnectionIpRestrictionEntry`
    /// (EdgionStreamPlugins). Fail-to-base applies on absent/invalid/type-mismatch.
    GlobalConnIpRestrictionOverride(GlobalConnIpRestrictionOverride),

    /// Escape hatch for third-party, experimental, or unknown payloads.
    /// The `config` content is not validated — use sparingly and only when no
    /// typed first-party variant fits.  `x-kubernetes-preserve-unknown-fields`
    /// applies at the CRD level (no schema validation by the API server).
    Misc(serde_json::Value),
}

impl EdgionData {
    /// Get the data type name of this data entry.
    pub fn data_type(&self) -> &str {
        match &self.spec.data {
            DataEntry::KeyList(_) => "KeyList",
            DataEntry::IpList(_) => "IpList",
            DataEntry::ClusterRegionRoute(_) => "ClusterRegionRoute",
            DataEntry::ServiceRegionRoute(_) => "ServiceRegionRoute",
            DataEntry::GlobalConnectionIpRestriction(_) => "GlobalConnectionIpRestriction",
            DataEntry::GlobalConnIpRestrictionOverride(_) => "GlobalConnIpRestrictionOverride",
            DataEntry::Misc(_) => "Misc",
        }
    }

    /// Validate the data payload and return a list of human-readable error messages.
    ///
    /// An empty result means "no problems found". Errors are suitable for inclusion in an
    /// `Accepted=False/reason=Invalid` status condition by the Controller, and are also
    /// emitted as warnings by the data-plane `pre_parse` hook (see `impl_resource_meta!`).
    pub fn validate_pre_parse(&self) -> Vec<String> {
        let mut errors = Vec::new();
        match &self.spec.data {
            DataEntry::IpList(ip_list) => {
                if let Err(e) = crate::matcher::ip_cidr_helpers::validate_groups_structure(&ip_list.items) {
                    errors.push(format!("IpList invalid: {}", e));
                }
            }
            DataEntry::KeyList(key_list) => {
                if let Err(e) = crate::matcher::key_group_helpers::validate_key_groups_structure(
                    &key_list.items,
                    &key_list.match_mode,
                ) {
                    errors.push(format!("KeyList invalid: {}", e));
                }
            }
            DataEntry::GlobalConnectionIpRestriction(data) => {
                // Early validation: clone and run full validate_and_init to surface errors.
                let mut cloned = data.clone();
                if let Err(e) = cloned.validate_and_init() {
                    errors.push(format!("GlobalConnectionIpRestriction invalid: {}", e));
                }
            }
            DataEntry::GlobalConnIpRestrictionOverride(overlay) => {
                // Standalone validation of the overlay (only validates present fields).
                let mut cloned = overlay.clone();
                if let Err(e) = cloned.validate_and_init() {
                    errors.push(format!("GlobalConnIpRestrictionOverride invalid: {}", e));
                }
            }
            // RegionRoute variants: no additional validation at pre-parse (cross-EdgionData
            // reference checks live in the Controller handler's update_status via
            // ResolvedRefs, not here).
            DataEntry::ClusterRegionRoute(_) | DataEntry::ServiceRegionRoute(_) => {}
            // Misc: the payload is opaque; no schema validation is performed here.
            DataEntry::Misc(_) => {}
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

/// Compute the effective switch values from a resolved [`EdgionDataSpec`].
///
/// Returns `(effective_enable, effective_active)`:
/// - `effective_enable`: `false` means consumers must skip this overlay entirely
///   and fall back to base defaults (the kill switch).
/// - `effective_active`: optional profile selector; `None` means no profile override
///   (consumers that support profiles keep their own base default).
///
/// Consumers should call this helper before applying any data-level overrides.
/// If `effective_enable` is `false`, skip the overlay; do not call
/// [`apply_override`] in that case.
///
/// # Example
/// ```rust,ignore
/// let (enable, active) = effective_switch(&edgion_data.spec);
/// if !enable {
///     return base_config;
/// }
/// // ... proceed to apply_override ...
/// ```
pub fn effective_switch(spec: &EdgionDataSpec) -> (bool, Option<&str>) {
    (spec.enable, spec.active.as_deref())
}
