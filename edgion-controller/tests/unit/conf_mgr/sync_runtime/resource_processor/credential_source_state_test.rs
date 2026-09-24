use super::*;
use edgion_resources::ResourceKind;

fn consumer(source_key: &str) -> DesiredCredentialConsumer {
    DesiredCredentialConsumer {
        resource: ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("apps".to_string()),
            "provider".to_string(),
        ),
        uid: "consumer-uid".to_string(),
        generation: 3,
        source_key: source_key.to_string(),
        source_uid: "source-uid".to_string(),
        source_generation: 7,
        target_secret_namespace: "apps".to_string(),
        target_secret_name: "provider".to_string(),
        authorization_revision: "same-namespace".to_string(),
    }
}

#[test]
fn consumer_replacement_is_atomic_by_logical_identity() {
    let registry = CredentialSourceRegistry::new();
    registry.replace_consumer(consumer("ops/source-a"));
    registry.replace_consumer(consumer("ops/source-b"));

    assert!(registry.consumers_for_source("ops/source-a").is_empty());
    assert_eq!(registry.consumers_for_source("ops/source-b").len(), 1);
}

#[test]
fn clear_removes_all_desired_state() {
    let registry = CredentialSourceRegistry::new();
    registry.replace_consumer(consumer("ops/source"));
    registry.clear();
    assert!(registry.consumer("apps/provider").is_none());
    assert_eq!(registry.stats(), CredentialSourceRegistryStats::default());
    assert!(registry.keys().is_empty());
}

#[test]
fn publication_reconcile_requests_coalesce_until_health_is_observed() {
    let registry = CredentialSourceRegistry::new();
    let current = consumer("ops/source");
    registry.replace_consumer(current.clone());

    assert!(registry.request_consumer_reconcile_if_current(&current, "missing".to_string()));
    let first = registry.reconcile_epoch_for_source("ops/source");
    assert_ne!(first, 0);
    assert!(registry.request_consumer_reconcile_if_current(&current, "missing".to_string()));
    assert_eq!(registry.reconcile_epoch_for_source("ops/source"), first);

    registry.clear_consumer_reconcile("apps/provider");
    assert!(registry.request_consumer_reconcile_if_current(&current, "missing".to_string()));
    assert!(registry.reconcile_epoch_for_source("ops/source") > first);
    let second = registry.reconcile_epoch_for_source("ops/source");
    assert!(registry.request_consumer_reconcile_if_current(&current, "uid:43".to_string()));
    assert!(registry.reconcile_epoch_for_source("ops/source") > second);
}

#[test]
fn stale_consumer_cannot_resurrect_reconcile_bookkeeping_after_delete() {
    let registry = CredentialSourceRegistry::new();
    let stale = consumer("ops/source");
    registry.replace_consumer(stale.clone());
    registry.forget_consumer("apps/provider");

    assert!(!registry.request_consumer_reconcile_if_current(&stale, "late".to_string()));
    assert_eq!(registry.stats(), CredentialSourceRegistryStats::default());

    let current = consumer("ops/source");
    registry.replace_consumer(current.clone());
    assert!(registry.request_consumer_reconcile_if_current(&current, "current".to_string()));
    assert_eq!(registry.reconcile_epoch_for_source("ops/source"), 1);
}

#[test]
fn replaced_consumer_rejects_notifications_from_the_previous_identity() {
    let registry = CredentialSourceRegistry::new();
    let stale = consumer("ops/source");
    registry.replace_consumer(stale.clone());

    let mut current = stale.clone();
    current.uid = "replacement-uid".to_string();
    current.generation += 1;
    registry.replace_consumer(current.clone());

    assert!(!registry.request_consumer_reconcile_if_current(&stale, "late".to_string()));
    assert_eq!(registry.stats().reconcile_pending_count, 0);
    assert!(registry.request_consumer_reconcile_if_current(&current, "current".to_string()));
    assert_eq!(registry.stats().reconcile_pending_count, 1);
}
