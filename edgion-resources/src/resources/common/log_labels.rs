//! Operator-defined log projection resolved from resource metadata.
//!
//! Three annotation carriers feed the access / TLS log, all resolved here:
//! - [`LOG_LABEL_PREFIX`](crate::constants::annotations::edgion::LOG_LABEL_PREFIX)
//!   (`edgion.io/log-label.<suffix>`) — contributes `<suffix> = <value>` to the
//!   resource's `rsc.labels` (value given inline).
//! - [`LOG_KEYS`](crate::constants::annotations::edgion::LOG_KEYS)
//!   (`edgion.io/log-keys`) — a JSON array of `{key, type}`; `type: label` /
//!   `type: annotation` projects an existing metadata key's value into
//!   `rsc.labels`, and `type: ctx` opts a `ctx_map` key into the access log.
//! - [`LOG_CTX_KEYS`](crate::constants::annotations::edgion::LOG_CTX_KEYS)
//!   (`edgion.io/log-ctx-keys`) — a comma-separated shorthand for `log-keys`
//!   `type: ctx` entries.
//!
//! `resolve_log_labels` covers the label/annotation carriers; `resolve_log_ctx_keys`
//! covers the ctx allowlist (`log-ctx-keys` ∪ `log-keys` `type: ctx`).
//!
//! Resolution happens **once, at resource-parse time** (route conf-handler build
//! or `ResourceMeta::pre_parse`), never on the per-request / per-handshake data
//! path. The result is an `Arc<..>` so it can be shared by reference (cheap
//! clone, deterministic ordering) into every log entry.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use kube::api::ObjectMeta;
use serde::Deserialize;

use crate::constants::annotations::edgion::{LOG_CTX_KEYS, LOG_KEYS, LOG_LABEL_PREFIX};

/// Upper bounds for operator-defined log labels (guardrails against log bloat
/// and high downstream cardinality). Over-limit entries are dropped, not errored.
pub const LOG_LABEL_MAX_COUNT: usize = 16;
pub const LOG_LABEL_MAX_KEY_LEN: usize = 63;
pub const LOG_LABEL_MAX_VALUE_LEN: usize = 256;

/// The projection source of one `edgion.io/log-keys` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LogKeySource {
    /// A plugin-set context variable (`ctx_map`) key — gated into the access log
    /// by the same allowlist as `log-ctx-keys`.
    Ctx,
    /// An existing `metadata.labels` key whose value is projected into `rsc.labels`.
    Label,
    /// An existing `metadata.annotations` key whose value is projected into `rsc.labels`.
    Annotation,
}

/// One entry of the `edgion.io/log-keys` JSON array.
#[derive(Debug, Clone, Deserialize)]
struct LogKeyEntry {
    key: String,
    #[serde(rename = "type")]
    source: LogKeySource,
}

/// Parse the `edgion.io/log-keys` annotation into entries. The value is a JSON
/// array; malformed *individual* entries are skipped (lenient), and a value that
/// is not a JSON array at all yields an empty list. Resolution is parse-time only.
fn parse_log_keys(meta: &ObjectMeta) -> Vec<LogKeyEntry> {
    let Some(raw) = meta.annotations.as_ref().and_then(|m| m.get(LOG_KEYS)) else {
        return Vec::new();
    };
    let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    values
        .into_iter()
        .filter_map(|v| serde_json::from_value::<LogKeyEntry>(v).ok())
        .collect()
}

/// Resolve operator-defined log labels from a resource's `metadata`.
///
/// Two carriers feed `rsc.labels`, merged in this order (later overrides on key
/// collision):
/// 1. **`LOG_LABEL_PREFIX` prefix** — `annotations` and `labels` keys prefixed
///    with `edgion.io/log-label.`; the suffix after the prefix becomes the label
///    key (`labels` first, then `annotations` override). The value is given
///    inline by the prefixed entry.
/// 2. **`edgion.io/log-keys`** entries of `type: label` / `type: annotation` —
///    reference an existing `metadata.labels` / `metadata.annotations` key by
///    name; its value is projected under that same key. These explicit entries
///    are applied last, so they override prefix-derived labels on collision.
///
/// Entries whose key/value exceed the length caps are dropped, and the result is
/// capped at [`LOG_LABEL_MAX_COUNT`]. Returns `None` when nothing matched, so the
/// field is omitted from the log entry. The returned map is wrapped in an `Arc`
/// so the resolved set can be shared by reference across log entries.
pub fn resolve_log_labels(meta: &ObjectMeta) -> Option<Arc<BTreeMap<String, String>>> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();

    // (1) `edgion.io/log-label.<suffix>` prefix: labels first, annotations override.
    for src in [meta.labels.as_ref(), meta.annotations.as_ref()] {
        let Some(map) = src else { continue };
        for (key, value) in map {
            let Some(suffix) = key.strip_prefix(LOG_LABEL_PREFIX) else {
                continue;
            };
            if suffix.is_empty() || suffix.len() > LOG_LABEL_MAX_KEY_LEN || value.len() > LOG_LABEL_MAX_VALUE_LEN {
                continue;
            }
            out.insert(suffix.to_string(), value.clone());
        }
    }

    // (2) `edgion.io/log-keys` entries of type label/annotation: reference an
    // existing metadata key by name (explicit entries override prefix ones).
    for entry in parse_log_keys(meta) {
        let value = match entry.source {
            LogKeySource::Label => meta.labels.as_ref().and_then(|m| m.get(&entry.key)),
            LogKeySource::Annotation => meta.annotations.as_ref().and_then(|m| m.get(&entry.key)),
            LogKeySource::Ctx => continue,
        };
        let Some(value) = value else { continue };
        if entry.key.is_empty() || entry.key.len() > LOG_LABEL_MAX_KEY_LEN || value.len() > LOG_LABEL_MAX_VALUE_LEN {
            continue;
        }
        out.insert(entry.key.clone(), value.clone());
    }

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

/// Resolve the opt-in allowlist of plugin-set `ctx_map` keys to project into the
/// access log. Two carriers feed the set (union):
/// 1. **`edgion.io/log-ctx-keys`** — a comma-separated shorthand list of ctx-var
///    names.
/// 2. **`edgion.io/log-keys`** entries of `type: ctx`.
///
/// Entries are trimmed, empty/over-length keys dropped, duplicates collapsed (a
/// `BTreeSet` also yields deterministic ordering), and the set is capped at
/// [`LOG_LABEL_MAX_COUNT`]. Returns `None` when nothing matched — meaning **no**
/// context variable is logged (default-off allowlist). Resolution happens once at
/// resource-parse time; the result is wrapped in an `Arc` for cheap sharing onto
/// each match-item unit.
pub fn resolve_log_ctx_keys(meta: &ObjectMeta) -> Option<Arc<BTreeSet<String>>> {
    let mut out: BTreeSet<String> = BTreeSet::new();

    // (1) `edgion.io/log-ctx-keys` comma-separated shorthand.
    if let Some(raw) = meta.annotations.as_ref().and_then(|m| m.get(LOG_CTX_KEYS)) {
        for tok in raw.split(',') {
            let key = tok.trim();
            if !key.is_empty() && key.len() <= LOG_LABEL_MAX_KEY_LEN {
                out.insert(key.to_string());
            }
        }
    }

    // (2) `edgion.io/log-keys` entries of type ctx.
    for entry in parse_log_keys(meta) {
        if entry.source == LogKeySource::Ctx && !entry.key.is_empty() && entry.key.len() <= LOG_LABEL_MAX_KEY_LEN {
            out.insert(entry.key);
        }
    }

    if out.is_empty() {
        return None;
    }
    if out.len() > LOG_LABEL_MAX_COUNT {
        // BTreeSet iterates in sorted order, so the cap keeps the lowest-sorted
        // keys deterministically across reloads.
        let capped: BTreeSet<String> = out.into_iter().take(LOG_LABEL_MAX_COUNT).collect();
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
    fn ctx_keys_none_when_annotation_absent() {
        assert!(resolve_log_ctx_keys(&ObjectMeta::default()).is_none());
        let m = meta(&[("unrelated", "x")], &[]);
        assert!(resolve_log_ctx_keys(&m).is_none());
    }

    #[test]
    fn ctx_keys_parsed_trimmed_and_deduped() {
        let m = meta(&[(LOG_CTX_KEYS, " tenant_id , tier ,tenant_id, ")], &[]);
        let out = resolve_log_ctx_keys(&m).expect("ctx keys");
        assert_eq!(out.len(), 2);
        assert!(out.contains("tenant_id"));
        assert!(out.contains("tier"));
    }

    #[test]
    fn ctx_keys_drops_over_length_and_empty_tokens() {
        let long_key = "k".repeat(LOG_LABEL_MAX_KEY_LEN + 1);
        let value = format!("good,,{long_key}");
        let m = meta(&[(LOG_CTX_KEYS, &value)], &[]);
        let out = resolve_log_ctx_keys(&m).expect("ctx keys");
        assert_eq!(out.len(), 1);
        assert!(out.contains("good"));
    }

    #[test]
    fn ctx_keys_none_when_only_blank() {
        let m = meta(&[(LOG_CTX_KEYS, " , , ")], &[]);
        assert!(resolve_log_ctx_keys(&m).is_none());
    }

    #[test]
    fn ctx_keys_capped_deterministically() {
        let value = (0..(LOG_LABEL_MAX_COUNT + 5))
            .map(|i| format!("{i:03}"))
            .collect::<Vec<_>>()
            .join(",");
        let m = meta(&[(LOG_CTX_KEYS, &value)], &[]);
        let out = resolve_log_ctx_keys(&m).expect("ctx keys");
        assert_eq!(out.len(), LOG_LABEL_MAX_COUNT);
        assert!(out.contains("000"));
        assert!(!out.contains(&format!("{:03}", LOG_LABEL_MAX_COUNT)));
    }

    #[test]
    fn log_keys_ctx_unions_with_shorthand() {
        let m = meta(
            &[
                (LOG_CTX_KEYS, "tenant_id"),
                (
                    LOG_KEYS,
                    r#"[{"key":"tier","type":"ctx"},{"key":"tenant_id","type":"ctx"}]"#,
                ),
            ],
            &[],
        );
        let out = resolve_log_ctx_keys(&m).expect("ctx keys");
        assert_eq!(out.len(), 2);
        assert!(out.contains("tenant_id"));
        assert!(out.contains("tier"));
    }

    #[test]
    fn log_keys_projects_existing_label_and_annotation() {
        let m = meta(
            &[
                (
                    LOG_KEYS,
                    r#"[{"key":"owner","type":"annotation"},{"key":"app","type":"label"}]"#,
                ),
                ("owner", "platform"),
            ],
            &[("app", "web")],
        );
        let out = resolve_log_labels(&m).expect("labels");
        assert_eq!(out.get("owner").map(String::as_str), Some("platform"));
        assert_eq!(out.get("app").map(String::as_str), Some("web"));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn log_keys_label_type_does_not_leak_into_ctx_allowlist() {
        // A `type: label` entry must not be treated as a ctx allowlist key.
        let m = meta(&[(LOG_KEYS, r#"[{"key":"app","type":"label"}]"#)], &[("app", "web")]);
        assert!(resolve_log_ctx_keys(&m).is_none());
    }

    #[test]
    fn log_keys_ctx_type_does_not_project_labels() {
        // A `type: ctx` entry must not produce an `rsc.labels` entry.
        let m = meta(&[(LOG_KEYS, r#"[{"key":"tenant_id","type":"ctx"}]"#)], &[]);
        assert!(resolve_log_labels(&m).is_none());
    }

    #[test]
    fn log_keys_missing_metadata_value_is_skipped() {
        // Referencing a non-existent label/annotation key yields nothing.
        let m = meta(&[(LOG_KEYS, r#"[{"key":"ghost","type":"label"}]"#)], &[]);
        assert!(resolve_log_labels(&m).is_none());
    }

    #[test]
    fn log_keys_explicit_entry_overrides_prefix_label() {
        // `log-keys` label entries are applied after the prefix pass, so they win.
        let m = meta(
            &[
                (&format!("{LOG_LABEL_PREFIX}tier"), "from-prefix"),
                (LOG_KEYS, r#"[{"key":"tier","type":"annotation"}]"#),
                ("tier", "from-log-keys"),
            ],
            &[],
        );
        let out = resolve_log_labels(&m).expect("labels");
        assert_eq!(out.get("tier").map(String::as_str), Some("from-log-keys"));
    }

    #[test]
    fn log_keys_malformed_entries_are_skipped_individually() {
        // One bad entry (unknown type) must not discard the valid sibling.
        let m = meta(
            &[(
                LOG_KEYS,
                r#"[{"key":"tenant_id","type":"bogus"},{"key":"tier","type":"ctx"}]"#,
            )],
            &[],
        );
        let out = resolve_log_ctx_keys(&m).expect("ctx keys");
        assert_eq!(out.len(), 1);
        assert!(out.contains("tier"));
    }

    #[test]
    fn log_keys_non_array_value_is_ignored() {
        let m = meta(&[(LOG_KEYS, "not-json"), (LOG_CTX_KEYS, "tenant_id")], &[]);
        // Bad log-keys is ignored; the shorthand still resolves.
        let out = resolve_log_ctx_keys(&m).expect("ctx keys");
        assert_eq!(out.len(), 1);
        assert!(out.contains("tenant_id"));
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
