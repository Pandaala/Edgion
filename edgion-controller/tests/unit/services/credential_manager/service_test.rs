use super::*;
use edgion_resources::ResourceKind;

fn consumer(name: &str, generation: i64) -> DesiredCredentialConsumer {
    DesiredCredentialConsumer {
        reference: crate::reference_test_support::injector_reference(&crate::reference_test_support::injector_owner(
            "apps",
            name,
            &format!("uid-{name}"),
            generation,
            "ops/source",
        )),
        resource: crate::conf_mgr::sync_runtime::resource_processor::ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("apps".to_string()),
            name.to_string(),
        ),
        uid: format!("uid-{name}"),
        generation,
        source_key: "ops/source".to_string(),
        source_uid: "source-uid".to_string(),
        source_generation: 2,
        target_secret_namespace: "apps".to_string(),
        target_secret_name: name.to_string(),
        authorization_revision: "grant-rv".to_string(),
    }
}

#[test]
fn consumer_set_revision_is_order_independent_and_change_sensitive() {
    let a = consumer("a", 1);
    let b = consumer("b", 1);
    assert_eq!(
        consumer_revision(&[a.clone(), b.clone()], 0),
        consumer_revision(&[b.clone(), a.clone()], 0)
    );
    assert_ne!(
        consumer_revision(&[a, b.clone()], 0),
        consumer_revision(&[consumer("a", 2), b], 0)
    );
    assert_ne!(consumer_revision(&[], 1), consumer_revision(&[], 2));
}

#[test]
fn binding_revision_covers_consumer_and_source_generations() {
    let original = consumer("a", 1);
    let mut changed = original.clone();
    changed.source_generation += 1;
    assert_ne!(original.binding_revision(), changed.binding_revision());
}

#[test]
fn recovery_never_restarts_rotation_interval_from_failover_time() {
    let issued = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let valid_until = issued + Duration::from_secs(3_570);
    let now_wall = issued + Duration::from_secs(15 * 60);
    let now_mono = Instant::now();
    let refresh = recovered_refresh_at(
        issued,
        valid_until,
        Duration::from_secs(15 * 60),
        "source/publication",
        now_wall,
        now_mono,
    );
    assert_eq!(refresh, now_mono, "passed periodic deadline must refresh immediately");
}
