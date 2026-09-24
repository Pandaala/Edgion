use super::*;

#[test]
fn route_meta_is_resolved_once_and_shared() {
    use std::collections::BTreeMap;

    let meta = kube::api::ObjectMeta {
        labels: Some(BTreeMap::from([("app".to_string(), "web".to_string())])),
        annotations: Some(BTreeMap::from([("owner".to_string(), "platform".to_string())])),
        ..Default::default()
    };

    let (labels, annotations) = extract_route_meta(&meta);
    assert_eq!(labels.as_ref().unwrap().get("app").unwrap(), "web");
    assert_eq!(annotations.as_ref().unwrap().get("owner").unwrap(), "platform");
}

#[test]
fn route_meta_is_none_when_absent() {
    let (labels, annotations) = extract_route_meta(&kube::api::ObjectMeta::default());
    assert!(labels.is_none());
    assert!(annotations.is_none());
}

#[test]
fn route_meta_is_not_serialized_into_the_log_entry() {
    // The maps exist to answer `from: routeLabel` / `from: routeAnnotation`
    // lookups. They must never leak into `match_info` wholesale — only
    // declared keys reach `ext`. Populate both maps via `with_route_meta`
    // with distinctive values so the assertion actually exercises
    // `#[serde(skip)]` on both `labels_meta` and `annotations_meta`
    // (a prior version of this test left both maps `None` via `MatchInfo::new`
    // alone, which caught removing `#[serde(skip)]` from `annotations_meta` —
    // that field's name contains "annotations" — but not from `labels_meta`,
    // since `"labels_meta":null` doesn't contain "annotations").
    // `HTTPRouteMatch` has no `Default` impl, so build it explicitly with
    // every field empty/None (all optional matchers).
    use std::collections::BTreeMap;

    let match_item = HTTPRouteMatch {
        path: None,
        headers: None,
        query_params: None,
        method: None,
    };
    let labels = Arc::new(BTreeMap::from([(
        "canary-label-marker".to_string(),
        "canary-label-value".to_string(),
    )]));
    let annotations = Arc::new(BTreeMap::from([(
        "canary-annotation-marker".to_string(),
        "canary-annotation-value".to_string(),
    )]));
    let info = MatchInfo::new("default".to_string(), "r".to_string(), 0, 0, match_item, 0)
        .with_route_meta(Some(labels), Some(annotations));
    let json = serde_json::to_string(&info).unwrap();
    assert!(!json.contains("labels_meta"), "got: {json}");
    assert!(!json.contains("annotations_meta"), "got: {json}");
    assert!(!json.contains("canary-label-marker"), "got: {json}");
    assert!(!json.contains("canary-label-value"), "got: {json}");
    assert!(!json.contains("canary-annotation-marker"), "got: {json}");
    assert!(!json.contains("canary-annotation-value"), "got: {json}");
}
