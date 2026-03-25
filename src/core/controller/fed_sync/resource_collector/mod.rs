//! Reads raw resources from ConfCenter and converts them to ResourceKey (metadata only).
//! Filters out FED_NO_SYNC_KINDS on the controller side before reporting to center.

use crate::core::common::fed_sync::proto::ResourceKey;
use crate::core::controller::conf_mgr::conf_center::traits::CenterApi;
use anyhow::Result;
use std::collections::HashMap;

/// Resource kinds excluded from federation sync (controller side filter).
pub const FED_NO_SYNC_KINDS: &[&str] = &["Secret", "ConfigMap", "Endpoint", "EndpointSlice", "ReferenceGrant"];

pub fn is_fed_no_sync_kind(kind: &str) -> bool {
    FED_NO_SYNC_KINDS.iter().any(|k| *k == kind)
}

/// Extract a ResourceKey from raw YAML content.
/// Returns None if YAML is unparseable (logs a warning, does not fail the whole list).
pub fn extract_resource_key(kind: &str, yaml_content: &str) -> Option<ResourceKey> {
    let value: serde_yaml::Value = serde_yaml::from_str(yaml_content).ok()?;
    let metadata = value.get("metadata")?;

    let name = metadata.get("name")?.as_str()?.to_string();
    let namespace = metadata
        .get("namespace")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let resource_version = metadata
        .get("resourceVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let labels = extract_string_map(metadata.get("labels"));
    let annotations = extract_string_map(metadata.get("annotations"));

    Some(ResourceKey {
        kind: kind.to_string(),
        namespace,
        name,
        resource_version,
        labels,
        annotations,
    })
}

fn extract_string_map(value: Option<&serde_yaml::Value>) -> HashMap<String, String> {
    value
        .and_then(|v| v.as_mapping())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    let key = k.as_str()?.to_string();
                    let val = v.as_str()?.to_string();
                    Some((key, val))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Collect ResourceKeys for all non-excluded kinds from the ConfCenter.
///
/// `supported_kinds` is the full list of kinds available in this controller.
/// This function filters out FED_NO_SYNC_KINDS and fetches + parses each remaining kind.
pub async fn collect_resource_keys(
    conf_center: &dyn CenterApi,
    supported_kinds: &[String],
) -> Result<Vec<ResourceKey>> {
    let mut result = Vec::new();

    for kind in supported_kinds {
        if is_fed_no_sync_kind(kind) {
            continue;
        }

        let list_result = conf_center
            .get_list_by_kind(kind, None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list {}: {}", kind, e))?;

        for entry in list_result.items {
            match extract_resource_key(&entry.kind, &entry.content) {
                Some(key) => result.push(key),
                None => {
                    tracing::warn!(
                        component = "resource_collector",
                        kind = %entry.kind,
                        name = %entry.name,
                        "Failed to parse resource metadata, skipping"
                    );
                }
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_yaml(labels: &str, annotations: &str, resource_version: &str) -> String {
        format!(
            "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: my-route\n  namespace: default\n  resourceVersion: \"{resource_version}\"\n  labels:\n    {labels}\n  annotations:\n    {annotations}\nspec: {{}}\n"
        )
    }

    #[test]
    fn test_extract_metadata_from_yaml() {
        let yaml = make_yaml("app: foo", "note: bar", "12345");
        let key = extract_resource_key("HTTPRoute", &yaml).unwrap();
        assert_eq!(key.kind, "HTTPRoute");
        assert_eq!(key.resource_version, "12345");
        assert_eq!(key.labels.get("app").map(|s| s.as_str()), Some("foo"));
        assert_eq!(key.annotations.get("note").map(|s| s.as_str()), Some("bar"));
    }

    #[test]
    fn test_extract_missing_fields_defaults() {
        let yaml = "metadata:\n  name: bare\nspec: {}\n";
        let key = extract_resource_key("Gateway", yaml).unwrap();
        assert_eq!(key.name, "bare");
        assert_eq!(key.namespace, "");
        assert_eq!(key.resource_version, "");
        assert!(key.labels.is_empty());
    }

    #[test]
    fn test_fed_no_sync_kinds_are_excluded() {
        for kind in FED_NO_SYNC_KINDS {
            assert!(is_fed_no_sync_kind(kind));
        }
        assert!(!is_fed_no_sync_kind("HTTPRoute"));
        assert!(!is_fed_no_sync_kind("Gateway"));
    }
}
