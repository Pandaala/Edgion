use super::*;
use std::time::Duration;

fn service(kind: &str, mode: Option<&str>) -> Service {
    let mut annotations = serde_json::Map::new();
    if let Some(mode) = mode {
        annotations.insert(DNS_RESOLUTION_MODE.into(), mode.into());
    }
    serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "app", "name": "source", "uid": "uid", "annotations": annotations},
        "spec": {"type": kind, "externalName": "example.com"}
    }))
    .unwrap()
}

async fn woke(targets: &DnsResolverTargets) -> bool {
    tokio::time::timeout(Duration::from_millis(10), targets.changed())
        .await
        .is_ok()
}

#[tokio::test]
async fn only_controller_mode_external_name_services_are_targets() {
    let targets = DnsResolverTargets::new();
    for (kind, mode) in [
        ("ClusterIP", None),
        ("ExternalName", Some("gateway")),
        ("ExternalName", Some("per-request")),
    ] {
        targets.sync_published(&service(kind, mode));
        assert!(targets.snapshot().is_empty(), "{kind} {mode:?}");
    }
    for mode in [None, Some("controller")] {
        targets.sync_published(&service("ExternalName", mode));
        let snapshot = targets.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].0, "app/source");
    }
}

#[tokio::test]
async fn content_changes_bump_revision_and_wake_identical_republish_does_not() {
    let targets = DnsResolverTargets::new();
    let mut target = service("ExternalName", None);
    targets.sync_published(&target);
    assert!(woke(&targets).await);
    let first = targets.snapshot()[0].1.revision;

    targets.sync_published(&target);
    assert!(!woke(&targets).await, "identical content must not wake the resolver");
    assert_eq!(targets.snapshot()[0].1.revision, first);

    target.spec.as_mut().unwrap().external_name = Some("other.example.com".into());
    targets.sync_published(&target);
    assert!(woke(&targets).await);
    let second = targets.snapshot()[0].1.revision;
    assert!(second > first);
    assert_eq!(
        targets.snapshot()[0]
            .1
            .service
            .spec
            .as_ref()
            .unwrap()
            .external_name
            .as_deref(),
        Some("other.example.com")
    );
}

#[tokio::test]
async fn edits_off_target_unpublish_and_clear_release_the_entry() {
    let targets = DnsResolverTargets::new();
    let target = service("ExternalName", None);

    targets.sync_published(&target);
    let _ = woke(&targets).await;
    targets.sync_published(&service("ExternalName", Some("gateway")));
    assert!(targets.snapshot().is_empty());
    assert!(woke(&targets).await, "removal must wake the resolver for GC");

    targets.sync_published(&target);
    targets.remove_published(&target);
    assert!(targets.snapshot().is_empty());

    targets.sync_published(&target);
    let _ = woke(&targets).await;
    targets.clear();
    assert!(targets.snapshot().is_empty());
    assert!(woke(&targets).await);

    targets.remove_published(&target);
    assert!(!woke(&targets).await, "removing an absent entry is silent");
}
