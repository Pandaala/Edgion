use super::*;
use edgion_resources::constants::annotations::edgion::LOG_LABEL_PREFIX;
use kube::api::ObjectMeta;

fn json_of(labels: &Option<LogLabels>) -> String {
    let rsc = Rsc::new("HTTPRoute", "ns", "name").with_labels(labels.clone());
    serde_json::to_string(&rsc).unwrap()
}

// Resolution semantics (prefix strip, precedence, guardrails, cap) are
// covered by `edgion_resources::resources::common::log_labels`. These tests
// pin the gateway-side serialization shape and the delegating wrapper.

#[test]
fn extract_serializes_sorted_object_with_prefix_stripped() {
    let meta = ObjectMeta {
        annotations: Some(
            [(format!("{LOG_LABEL_PREFIX}tenant"), "internal".to_string())]
                .into_iter()
                .collect(),
        ),
        labels: Some(
            [(format!("{LOG_LABEL_PREFIX}env"), "prod".to_string())]
                .into_iter()
                .collect(),
        ),
        ..Default::default()
    };
    let out = extract_log_labels(&meta).expect("labels");
    assert_eq!(
        json_of(&Some(out)),
        r#"{"kind":"HTTPRoute","ns":"ns","name":"name","labels":{"env":"prod","tenant":"internal"}}"#
    );
}

#[test]
fn extract_returns_none_without_prefixed_entries() {
    let meta = ObjectMeta {
        annotations: Some([("unrelated".to_string(), "x".to_string())].into_iter().collect()),
        ..Default::default()
    };
    assert!(extract_log_labels(&meta).is_none());
}

#[test]
fn from_resolved_wraps_cached_arc_without_rescanning() {
    let map = Arc::new([("env".to_string(), "prod".to_string())].into_iter().collect());
    let out = LogLabels::from_resolved(Arc::clone(&map));
    assert_eq!(
        json_of(&Some(out)),
        r#"{"kind":"HTTPRoute","ns":"ns","name":"name","labels":{"env":"prod"}}"#
    );
}

#[test]
fn labels_field_omitted_when_absent() {
    assert_eq!(json_of(&None), r#"{"kind":"HTTPRoute","ns":"ns","name":"name"}"#);
}
