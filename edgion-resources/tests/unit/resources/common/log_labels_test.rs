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
fn log_keys_annotation_is_no_longer_honored() {
    // The carrier is deleted; only the log-label.<suffix> prefix remains, and
    // only TLS resources use it now.
    let m = meta(
        &[
            ("edgion.io/log-keys", r#"[{"key":"owner","type":"annotation"}]"#),
            ("owner", "platform"),
        ],
        &[],
    );
    assert!(resolve_log_labels(&m).is_none());
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
