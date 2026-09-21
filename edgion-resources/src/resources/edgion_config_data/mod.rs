//! EdgionConfigData resource definition
//!
//! EdgionConfigData is a general-purpose, hot-swappable metadata/overlay primitive.
//! Each EdgionConfigData contains exactly one `ConfigEntry` variant:
//!
//! | Group | Variant | Notes |
//! |-------|---------|-------|
//! | Standard primitive | `KeyList` | Unified string+regex list |
//! | Standard primitive | `IpList` | CIDR range list |
//! | Policy data | `RequestAccessUrlAllowList` | URL allow candidates bound by RequestAccessPolicy |
//! | Security policy | `ProxyProtocolTrust` | Strict inbound PROXY sender trust policy |
//! | Selector | `Selector` | Active-profile selector via `activeProfileRef` (GlobalConnectionIpRestriction) |
//! | First-party overlay | `RegionRouteOverride` | All-Optional region table overlay via `overrideRef` |
//! | Escape hatch | `Misc` | Third-party / unknown payloads |
//!
//! The top-level envelope carries `enable` (kill switch, default `true`) and
//! `active` (profile selector, optional).  See [`effective_switch`].

pub mod ip_group;
pub mod ip_list;
pub mod key_list;
pub mod overlay;
pub mod region_route;
pub mod request_access_url_allow_list;
pub mod selector;

pub use ip_group::IpGroup;
pub use ip_list::IpListData;
pub use key_list::{KeyGroup, KeyListData, KeyMatchMode, MetaDataItem};
pub use overlay::{apply_override, OverlaySignal};
pub use region_route::{merge_region_route_override, validate_regions, RegionRouteOverride, RegionTableValidation};
pub use request_access_url_allow_list::RequestAccessUrlAllowListData;
pub use selector::Selector;

use crate::resources::common::Condition;
use crate::resources::proxy_protocol::{
    CanonicalProxyProtocolTrustPolicy, ProxyProtocolConfigErrorCode, ProxyProtocolTrustPolicy,
};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Controls which namespaces may reference this EdgionConfigData cross-namespace.
/// `Namespace` (default): cross-ns refs require a matching `ReferenceGrant`.
/// `Cluster`: any namespace may reference it (no grant needed).
/// Only consulted when `is_reference_grant_validation_enabled()` is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "PascalCase")]
pub enum ConfigDataVisibility {
    #[default]
    Namespace,
    Cluster,
}

fn default_enable() -> bool {
    true
}

pub use crate::resources::config_data_ref::EdgionConfigDataRef;

/// EdgionConfigData is a general-purpose, hot-swappable metadata/overlay primitive
/// referenced by EdgionPlugins and EdgionStreamPlugins.
#[derive(CustomResource, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "EdgionConfigData",
    plural = "edgionconfigdata",
    namespaced,
    status = "EdgionConfigDataStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct EdgionConfigDataSpec {
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<EdgionConfigDataStatus>,

    /// Top-level kill switch. When `false`, consumers must ignore this EdgionConfigData's
    /// content and fall back to their own base defaults. Defaults to `true`.
    ///
    /// Wired by the H1 envelope kill-switch — both the RegionRoute and the
    /// GlobalConnIp gateway plugins short-circuit to base + emit an
    /// `overlay_disabled` Prometheus counter when this is `false`.
    #[serde(default = "default_enable")]
    pub enable: bool,

    /// Optional profile selector. When set, consumers that support preloaded
    /// profiles use this value to select the active profile from the base config.
    /// Consumers that do not support profiles ignore this field.
    ///
    /// Not yet wired: RegionRoute has no profile concept (reserved per D-S3-A);
    /// GlobalConnIp uses its own variant-inner `activeProfile` field, not this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,

    /// Cross-namespace reference visibility. See `ConfigDataVisibility`.
    #[serde(default)]
    pub visibility: ConfigDataVisibility,

    /// The data entry. Each EdgionConfigData contains exactly one entry,
    /// discriminated by the `type` tag.
    pub data: ConfigEntry,

    /// Unrecognized key paths found in the submitted document. Populated only on
    /// operator-authored ingest paths; never serialized and never exposed in the
    /// CRD schema.
    #[serde(skip)]
    #[schemars(skip)]
    pub unknown_fields: Vec<String>,
}

/// Deserialize-only mirror of [`EdgionConfigDataSpec`]. `unknown_fields` is
/// derived from the submitted document and therefore is intentionally absent.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EdgionConfigDataSpecShadow {
    #[serde(default)]
    current_status: Option<EdgionConfigDataStatus>,
    #[serde(default = "default_enable")]
    enable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active: Option<String>,
    #[serde(default)]
    visibility: ConfigDataVisibility,
    data: ConfigEntry,
}

impl From<EdgionConfigDataSpecShadow> for EdgionConfigDataSpec {
    fn from(shadow: EdgionConfigDataSpecShadow) -> Self {
        Self {
            current_status: shadow.current_status,
            enable: shadow.enable,
            active: shadow.active,
            visibility: shadow.visibility,
            data: shadow.data,
            unknown_fields: Vec::new(),
        }
    }
}

impl EdgionConfigDataSpec {
    /// Validate a ProxyProtocolTrust entry without losing ingest-time unknown
    /// field findings. Consumers must use this instead of canonicalizing the
    /// policy directly so a typo can never be accepted after lenient capture.
    pub fn canonical_proxy_protocol_trust_policy(
        &self,
    ) -> Option<Result<CanonicalProxyProtocolTrustPolicy, ProxyProtocolConfigErrorCode>> {
        let ConfigEntry::ProxyProtocolTrust(policy) = &self.data else {
            return None;
        };
        if self
            .unknown_fields
            .iter()
            .any(|path| path == "data.config" || path.starts_with("data.config."))
        {
            return Some(Err(ProxyProtocolConfigErrorCode::InvalidPolicy));
        }
        Some(policy.canonicalize())
    }
}

/// Cached production schema used by all EdgionConfigData unknown-key checks.
pub fn edgion_config_data_spec_schema() -> &'static schemars::Schema {
    static SCHEMA: std::sync::OnceLock<schemars::Schema> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(|| schemars::schema_for!(EdgionConfigDataSpec))
}

impl<'de> Deserialize<'de> for EdgionConfigDataSpec {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if !crate::schema_check::strict_ingest_enabled() {
            return Ok(EdgionConfigDataSpecShadow::deserialize(deserializer)?.into());
        }
        let value = serde_json::Value::deserialize(deserializer)?;
        let unknown = crate::schema_check::unknown_key_paths(&value, edgion_config_data_spec_schema());
        let mut spec: Self = EdgionConfigDataSpecShadow::deserialize(value)
            .map_err(serde::de::Error::custom)?
            .into();
        spec.unknown_fields = unknown;
        Ok(spec)
    }
}

/// Status of EdgionConfigData.
///
/// Conditions follow Gateway API conventions:
/// - `Accepted`: spec is syntactically and semantically valid (per-variant validation passed).
/// - `ResolvedRefs`: cross-resource references resolve. Currently all variants always report
///   `True` since no typed variant carries external cross-resource references.
#[derive(Default, Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
pub struct EdgionConfigDataStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

/// A single data entry — the core payload of an EdgionConfigData resource.
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
pub enum ConfigEntry {
    /// Key-value list with configurable match mode (exact string or regex).
    /// Merges the legacy StringList and RegexList into a single type.
    KeyList(KeyListData),

    /// IP address / CIDR range list for IP-based filtering.
    IpList(IpListData),

    /// Named URL allow candidates imported by RequestAccessPolicy. The
    /// Controller binds nested IP/Secret references into the referring plugin.
    RequestAccessUrlAllowList(RequestAccessUrlAllowListData),

    /// Strict, non-overlay inbound PROXY protocol sender trust policy. This
    /// variant is all-or-nothing and never participates in fail-to-base.
    ProxyProtocolTrust(ProxyProtocolTrustPolicy),

    /// Generic active-profile selector. Carries only `active` (the profile name to
    /// activate) + an optional `description`. Referenced via `activeProfileRef` from a
    /// `GlobalConnectionIpRestriction` stream plugin (and reusable by other
    /// profile-based consumers later). Switches among the consumer's git-owned
    /// profiles; fail-to-base on absent/unknown/type-mismatch.
    Selector(Selector),

    /// Typed overlay for RegionRoute. All fields are optional; `regions` (if `Some`)
    /// whole-replaces the base region table. Logic fields are not overridable.
    /// Referenced via `overrideRef` in `RegionRouteConfig` (EdgionPlugins).
    /// Fail-to-base applies on absent/invalid/type-mismatch.
    RegionRouteOverride(RegionRouteOverride),

    /// Escape hatch for third-party, experimental, or unknown payloads.
    /// The `config` content is not validated — use sparingly and only when no
    /// typed first-party variant fits.  `x-kubernetes-preserve-unknown-fields`
    /// applies at the CRD level (no schema validation by the API server).
    Misc(serde_json::Value),
}

impl EdgionConfigData {
    /// Get the data type name of this data entry.
    pub fn data_type(&self) -> &str {
        match &self.spec.data {
            ConfigEntry::KeyList(_) => "KeyList",
            ConfigEntry::IpList(_) => "IpList",
            ConfigEntry::RequestAccessUrlAllowList(_) => "RequestAccessUrlAllowList",
            ConfigEntry::ProxyProtocolTrust(_) => "ProxyProtocolTrust",
            ConfigEntry::Selector(_) => "Selector",
            ConfigEntry::RegionRouteOverride(_) => "RegionRouteOverride",
            ConfigEntry::Misc(_) => "Misc",
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
            // IpList compilation is partial-good (see the partial-good spec): bad entries are
            // skipped at index-build time and surfaced via the PartiallyInvalid status condition,
            // NOT rejected here. KeyList/RegionRouteOverride remain all-or-nothing.
            ConfigEntry::IpList(_) => {}
            ConfigEntry::RequestAccessUrlAllowList(data) => {
                if let Err(error) = data.validate() {
                    errors.push(error);
                }
            }
            ConfigEntry::ProxyProtocolTrust(_) => {
                if self
                    .spec
                    .canonical_proxy_protocol_trust_policy()
                    .is_some_and(|result| result.is_err())
                {
                    errors.push("ProxyProtocolTrust invalid: policy validation failed".to_string());
                }
            }
            ConfigEntry::KeyList(key_list) => {
                if let Err(e) = crate::matcher::key_group_helpers::validate_key_groups_structure(
                    &key_list.items,
                    &key_list.match_mode,
                ) {
                    errors.push(format!("KeyList invalid: {}", e));
                }
            }
            // Selector is always structurally valid; the active-name ↔ profile check is a
            // consumer-side concern (done at connection time against the plugin's profiles).
            ConfigEntry::Selector(_) => {}
            ConfigEntry::RegionRouteOverride(ov) => {
                // Standalone validation of the overlay (only validates present fields).
                if let Err(e) = ov.validate_and_init() {
                    errors.push(format!("RegionRouteOverride invalid: {}", e));
                }
            }
            // Misc: the payload is opaque; no schema validation is performed here.
            ConfigEntry::Misc(_) => {}
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

/// Compute the effective switch values from a resolved [`EdgionConfigDataSpec`].
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
/// let (enable, active) = effective_switch(&edgion_config_data.spec);
/// if !enable {
///     return base_config;
/// }
/// // ... proceed to apply_override ...
/// ```
pub fn effective_switch(spec: &EdgionConfigDataSpec) -> (bool, Option<&str>) {
    (spec.enable, spec.active.as_deref())
}

#[cfg(test)]
mod ref_denied_tests {
    use super::*;

    #[test]
    fn ref_denied_absent_round_trips_and_skips_serialization() {
        let r = EdgionConfigDataRef {
            name: "x".into(),
            namespace: None,
            ref_denied: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        // ref_denied is skipped when None
        assert!(!json.contains("refDenied"), "got: {json}");
        let back: EdgionConfigDataRef = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn ref_denied_present_serializes_and_round_trips() {
        let r = EdgionConfigDataRef {
            name: "x".into(),
            namespace: Some("team-b".into()),
            ref_denied: Some(crate::resources::common::RefDenied {
                target_namespace: "team-b".into(),
                target_name: "x".into(),
                reason: Some("NoMatchingReferenceGrant".into()),
            }),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("refDenied"), "got: {json}");
        let back: EdgionConfigDataRef = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn old_payload_without_ref_denied_deserializes() {
        let back: EdgionConfigDataRef = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!(
            back,
            EdgionConfigDataRef {
                name: "x".into(),
                namespace: None,
                ref_denied: None
            }
        );
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;

    #[test]
    fn visibility_defaults_to_namespace_when_absent() {
        // a minimal spec JSON without `visibility`
        let spec: EdgionConfigDataSpec = serde_json::from_str(r#"{"data":{"type":"Misc","config":{}}}"#).unwrap();
        assert_eq!(spec.visibility, ConfigDataVisibility::Namespace);
    }

    #[test]
    fn visibility_cluster_round_trips_pascal_case() {
        let spec: EdgionConfigDataSpec =
            serde_json::from_str(r#"{"visibility":"Cluster","data":{"type":"Misc","config":{}}}"#).unwrap();
        assert_eq!(spec.visibility, ConfigDataVisibility::Cluster);
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains(r#""visibility":"Cluster""#), "got: {json}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    #[test]
    fn iplist_with_duplicate_cidr_is_partial_not_pre_parse_error() {
        use crate::matcher::ip_cidr_helpers::collect_entry_errors;
        let items = vec![
            IpGroup {
                name: "a".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".into()],
            },
            IpGroup {
                name: "b".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".into()],
            },
        ];
        let spec = EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: ConfigDataVisibility::Namespace,
            data: ConfigEntry::IpList(IpListData { items: items.clone() }),
            unknown_fields: Vec::new(),
        };
        let ed = EdgionConfigData {
            metadata: ObjectMeta {
                name: Some("dup".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec,
            status: None,
        };
        // Partial-good: no pre_parse error any more.
        assert!(ed.validate_pre_parse().is_empty());
        // The duplicate is surfaced by collect_entry_errors (drives PartiallyInvalid).
        let errs = collect_entry_errors(&items);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].reason.contains("duplicate CIDR"));
    }

    #[test]
    fn iplist_host_bit_duplicate_is_partial_not_pre_parse_error() {
        // 10.0.0.0/8 and 10.0.0.1/8 normalize to the same /8 prefix in the radix builder.
        let items = vec![
            IpGroup {
                name: "a".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".into()],
            },
            IpGroup {
                name: "b".into(),
                description: None,
                cidrs: vec!["10.0.0.1/8".into()],
            },
        ];
        let spec = EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: ConfigDataVisibility::Namespace,
            data: ConfigEntry::IpList(IpListData { items: items.clone() }),
            unknown_fields: Vec::new(),
        };
        let ed = EdgionConfigData {
            metadata: ObjectMeta {
                name: Some("host-bit-dup".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec,
            status: None,
        };
        assert!(ed.validate_pre_parse().is_empty());
        // Note: collect_entry_errors compares normalized CIDR strings; 10.0.0.1/8 vs 10.0.0.0/8
        // normalize differently as strings, so this case is caught at radix build() / index time,
        // not here. Assert at least that pre_parse no longer rejects it.
    }
}
