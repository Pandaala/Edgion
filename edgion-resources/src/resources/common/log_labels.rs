//! Operator-defined log labels resolved from resource metadata.
//!
//! Any `metadata.annotations` or `metadata.labels` entry whose key starts with
//! [`LOG_LABEL_PREFIX`](crate::constants::annotations::edgion::LOG_LABEL_PREFIX)
//! (`edgion.io/log-label.`) contributes `<suffix> = <value>` to the resource's
//! `rsc.labels` field in access / TLS logs.
//!
//! Resolution happens **once, at resource-parse time** (route conf-handler build
//! or `ResourceMeta::pre_parse`), never on the per-request / per-handshake data
//! path. The result is an `Arc<BTreeMap<..>>` so it can be shared by reference
//! (cheap clone, deterministic ordering) into every log entry.

use std::collections::BTreeMap;
use std::sync::Arc;

use kube::api::ObjectMeta;

use crate::constants::annotations::edgion::LOG_LABEL_PREFIX;

/// Upper bounds for operator-defined log labels (guardrails against log bloat
/// and high downstream cardinality). Over-limit entries are dropped, not errored.
pub const LOG_LABEL_MAX_COUNT: usize = 16;
pub const LOG_LABEL_MAX_KEY_LEN: usize = 63;
pub const LOG_LABEL_MAX_VALUE_LEN: usize = 256;

/// Resolve operator-defined log labels from a resource's `metadata`.
///
/// Both `annotations` and `labels` are scanned for keys prefixed with
/// `LOG_LABEL_PREFIX`; the suffix after the prefix becomes the label key.
/// `labels` are applied first, then `annotations` override on key collision
/// (annotations are the more explicit, intentional source). Entries whose
/// key/value exceed the length caps are dropped, and the result is capped at
/// [`LOG_LABEL_MAX_COUNT`]. Returns `None` when nothing matched, so the field is
/// omitted from the log entry.
///
/// The returned map is wrapped in an `Arc` so the resolved set can be shared by
/// reference across log entries without re-allocating.
pub fn resolve_log_labels(meta: &ObjectMeta) -> Option<Arc<BTreeMap<String, String>>> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut ingest = |src: Option<&BTreeMap<String, String>>| {
        let Some(map) = src else { return };
        for (key, value) in map {
            let Some(suffix) = key.strip_prefix(LOG_LABEL_PREFIX) else {
                continue;
            };
            if suffix.is_empty() || suffix.len() > LOG_LABEL_MAX_KEY_LEN || value.len() > LOG_LABEL_MAX_VALUE_LEN {
                continue;
            }
            out.insert(suffix.to_string(), value.clone());
        }
    };
    ingest(meta.labels.as_ref());
    ingest(meta.annotations.as_ref());

    if out.is_empty() {
        return None;
    }
    if out.len() > LOG_LABEL_MAX_COUNT {
        // BTreeMap iterates in sorted key order, so the cap is deterministic.
        let capped: BTreeMap<String, String> = out.into_iter().take(LOG_LABEL_MAX_COUNT).collect();
        return Some(Arc::new(capped));
    }
    Some(Arc::new(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(annotations: &[(&str, &str)], labels: &[(&str, &str)]) -> ObjectMeta {
        let to_map = |pairs: &[(&str, &str)]| -> Option<BTreeMap<String, String>> {
            if pairs.is_empty() {
                None
            } else {
                Some(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
            }
        };
        ObjectMeta {
            annotations: to_map(annotations),
            labels: to_map(labels),
            ..Default::default()
        }
    }

    #[test]
    fn none_when_no_prefixed_entries() {
        let m = meta(&[("unrelated", "x"), ("edgion.io/sync-version", "5")], &[]);
        assert!(resolve_log_labels(&m).is_none());
        assert!(resolve_log_labels(&ObjectMeta::default()).is_none());
    }

    #[test]
    fn extracts_and_strips_prefix_from_annotations_and_labels() {
        let m = meta(
            &[(&format!("{LOG_LABEL_PREFIX}tenant"), "internal")],
            &[(&format!("{LOG_LABEL_PREFIX}env"), "prod")],
        );
        let out = resolve_log_labels(&m).expect("labels");
        assert_eq!(out.get("tenant").map(String::as_str), Some("internal"));
        assert_eq!(out.get("env").map(String::as_str), Some("prod"));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn annotation_overrides_label_on_key_collision() {
        let m = meta(
            &[(&format!("{LOG_LABEL_PREFIX}tier"), "gold")],
            &[(&format!("{LOG_LABEL_PREFIX}tier"), "bronze")],
        );
        let out = resolve_log_labels(&m).expect("labels");
        assert_eq!(out.get("tier").map(String::as_str), Some("gold"));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn drops_empty_key_and_over_limit_entries() {
        let long_key = "k".repeat(LOG_LABEL_MAX_KEY_LEN + 1);
        let long_val = "v".repeat(LOG_LABEL_MAX_VALUE_LEN + 1);
        let m = meta(
            &[
                (LOG_LABEL_PREFIX, "empty-suffix"),
                (&format!("{LOG_LABEL_PREFIX}{long_key}"), "x"),
                (&format!("{LOG_LABEL_PREFIX}good"), "y"),
                (&format!("{LOG_LABEL_PREFIX}toolong"), &long_val),
            ],
            &[],
        );
        let out = resolve_log_labels(&m).expect("labels");
        assert_eq!(out.len(), 1);
        assert_eq!(out.get("good").map(String::as_str), Some("y"));
    }

    #[test]
    fn caps_label_count() {
        let pairs: Vec<(String, String)> = (0..(LOG_LABEL_MAX_COUNT + 5))
            // zero-padded so BTreeMap sort order is the numeric order
            .map(|i| (format!("{LOG_LABEL_PREFIX}{i:03}"), "v".to_string()))
            .collect();
        let annotations: BTreeMap<String, String> = pairs.into_iter().collect();
        let m = ObjectMeta {
            annotations: Some(annotations),
            ..Default::default()
        };
        let out = resolve_log_labels(&m).expect("labels");
        assert_eq!(out.len(), LOG_LABEL_MAX_COUNT);
        // The deterministic cap keeps the lowest-sorted keys.
        assert!(out.contains_key("000"));
        assert!(!out.contains_key(&format!("{:03}", LOG_LABEL_MAX_COUNT)));
    }
}
