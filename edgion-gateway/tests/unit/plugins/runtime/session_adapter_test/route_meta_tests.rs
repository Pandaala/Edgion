use super::route_meta_field;
use kube::api::ObjectMeta;
use std::collections::BTreeMap;

fn meta() -> ObjectMeta {
    let mut labels = BTreeMap::new();
    labels.insert("app".to_string(), "web".to_string());
    labels.insert("edgion.io/team".to_string(), "team-a".to_string());
    let mut ann = BTreeMap::new();
    ann.insert("owner".to_string(), "platform".to_string());
    ObjectMeta {
        name: Some("route1".to_string()),
        namespace: Some("ns1".to_string()),
        labels: Some(labels),
        annotations: Some(ann),
        ..Default::default()
    }
}

#[test]
fn resolves_identity_and_metadata() {
    let m = meta();
    assert_eq!(route_meta_field(&m, "name").as_deref(), Some("route1"));
    assert_eq!(route_meta_field(&m, "namespace").as_deref(), Some("ns1"));
    assert_eq!(route_meta_field(&m, "label.app").as_deref(), Some("web"));
    // dotted/slashed key round-trips (split on first '.')
    assert_eq!(route_meta_field(&m, "label.edgion.io/team").as_deref(), Some("team-a"));
    assert_eq!(route_meta_field(&m, "annotation.owner").as_deref(), Some("platform"));
}

#[test]
fn missing_and_unknown_return_none() {
    let m = meta();
    assert_eq!(route_meta_field(&m, "label.absent"), None);
    assert_eq!(route_meta_field(&m, "annotation.absent"), None);
    assert_eq!(route_meta_field(&m, "bogus"), None);
    assert_eq!(route_meta_field(&ObjectMeta::default(), "name"), None);
}
