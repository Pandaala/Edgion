use super::*;
use edgion_resources::ResourceKind;

fn consumer(source_key: &str) -> DesiredCredentialConsumer {
    DesiredCredentialConsumer {
        reference: crate::reference_test_support::injector_reference(&crate::reference_test_support::injector_owner(
            "apps",
            "provider",
            "consumer-uid",
            3,
            source_key,
        )),
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

#[test]
fn generated_target_is_derived_and_valid_binding_hash_encoding_is_preserved() {
    let original = consumer("ops/source");
    let target = original.output_target().unwrap();
    assert_eq!(target.scope().namespace(), Some("apps"));
    assert_eq!(target.name(), "provider");
    let mut old_encoding = Sha256::new();
    for part in [
        "consumer-uid",
        "3",
        "source-uid",
        "7",
        "apps",
        "provider",
        "same-namespace",
        original.reference.owner_revision(),
        &original.reference.slot_revision(),
    ] {
        old_encoding.update(part.as_bytes());
        old_encoding.update([0]);
    }
    assert_eq!(original.binding_revision(), hex::encode(old_encoding.finalize()));
    for case in 0..3 {
        let mut changed = original.clone();
        match case {
            0 => changed.resource.name = "other".into(),
            1 => changed.resource.namespace = Some("other".into()),
            _ => changed.resource.kind = ResourceKind::LinkSys,
        }
        assert!(changed.output_target().is_err());
    }
}

fn source() -> DesiredCredentialSource {
    let owner: LinkSys = serde_yaml::from_str(
        r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: source
  namespace: ops
  uid: source-uid
  generation: 7
spec:
  type: credentialSource
  config:
    provider:
      type: oauth2ClientCredentials
      tokenEndpoint: https://auth.example.com/token
      clientAuthentication:
        activeSecretRef:
          name: bootstrap
"#,
    )
    .unwrap();
    let prepared = edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences::from_link_sys(&owner);
    let bound = prepared.bind(&owner).unwrap();
    let reference = bound
        .references()
        .unwrap()
        .active_bootstrap_secret()
        .unwrap()
        .freeze()
        .unwrap();
    DesiredCredentialSource {
        resource: ResourceRef::new(ResourceKind::LinkSys, Some("ops".into()), "source".into()),
        uid: "source-uid".into(),
        generation: 7,
        persist: true,
        config: ControllerCredentialSourceConfig::new(owner).unwrap(),
        secret_inputs: vec![CredentialSecretInput {
            reference,
            role: "active-bootstrap",
            namespace: "ops".into(),
            name: "bootstrap".into(),
            uid: Some("bootstrap-uid".into()),
            resource_version: Some("1".into()),
            authorization_revision: "same-namespace".into(),
        }],
    }
}

fn result(source: &DesiredCredentialSource) -> CredentialManagerResult {
    CredentialManagerResult {
        observed_generation: source.generation,
        state: CredentialSourceState::Valid,
        published_generation: Some("publication".into()),
        issued_at: None,
        valid_until: None,
        last_successful_refresh_time: None,
        next_refresh_time: None,
        desired_consumer_count: 1,
        published_consumer_count: 1,
        conflicted_consumer_count: 0,
    }
}

#[test]
fn result_after_delete_cannot_resurrect_bookkeeping() {
    let registry = CredentialSourceRegistry::new();
    let source = source();
    registry.replace_source(source.clone());
    assert!(registry.replace_result(&source, result(&source)));
    registry.forget_source(&source.logical_key());
    assert!(!registry.replace_result(&source, result(&source)));
    assert!(registry.result(&source.logical_key()).is_none());
    assert_eq!(registry.stats(), CredentialSourceRegistryStats::default());
    assert!(registry.keys().is_empty());
}

#[test]
fn results_follow_source_uid_generation_and_input_revision() {
    for change in 0..3 {
        let registry = CredentialSourceRegistry::new();
        let old = source();
        registry.replace_source(old.clone());
        assert!(registry.replace_result(&old, result(&old)));
        let mut current = old.clone();
        match change {
            0 => current.uid = "replacement-uid".into(),
            1 => current.generation += 1,
            _ => current.secret_inputs[0].resource_version = Some("2".into()),
        }
        registry.replace_source(current.clone());
        assert!(registry.result(&old.logical_key()).is_none(), "change {change}");
        assert!(!registry.replace_result(&old, result(&old)), "change {change}");
        let mut fresh = result(&current);
        fresh.published_generation = Some("new-publication".into());
        assert!(registry.replace_result(&current, fresh.clone()));
        assert!(!registry.replace_result(&old, result(&old)));
        assert_eq!(registry.result(&current.logical_key()), Some(fresh));
        assert_eq!(registry.stats().result_count, 1);
    }
}

#[test]
fn identical_registration_preserves_result_and_suppresses_requeue() {
    let registry = CredentialSourceRegistry::new();
    let source = source();
    registry.replace_source(source.clone());
    let result = result(&source);
    assert!(registry.replace_result(&source, result.clone()));
    registry.remove_source(&source.logical_key());
    assert!(registry.result(&source.logical_key()).is_none());
    assert!(!registry.replace_result(&source, result.clone()));
    registry.replace_source(source.clone());
    assert_eq!(registry.result(&source.logical_key()), Some(result.clone()));
    assert!(!registry.replace_result(&source, result.clone()));
    let mut wrong_generation = result.clone();
    wrong_generation.observed_generation += 1;
    assert!(!registry.replace_result(&source, wrong_generation));
    assert_eq!(registry.remove_result(&source.logical_key()), Some(result));
    assert_eq!(registry.stats().result_count, 0);
}

#[test]
fn source_revision_preserves_encoding_and_tracks_proof_inputs() {
    let original = source();
    let input = &original.secret_inputs[0];
    let mut old_encoding = vec![
        format!(
            "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
            input.reference.owner_revision(),
            input.reference.slot_revision(),
            input.role,
            input.namespace,
            input.name,
            input.uid.as_deref().unwrap(),
            input.resource_version.as_deref().unwrap(),
            input.authorization_revision
        ),
        "source-uid\0source-uid".into(),
        format!("source-generation\0{}", 7),
    ];
    old_encoding.sort();
    let mut digest = Sha256::new();
    for part in old_encoding {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    assert_eq!(source_input_revision(&original), hex::encode(digest.finalize()));
    let mut changed = original.clone();
    changed.secret_inputs[0].authorization_revision = "new-grant".into();
    assert_ne!(source_input_revision(&original), source_input_revision(&changed));
    changed.secret_inputs.push(original.secret_inputs[0].clone());
    let forward = source_input_revision(&changed);
    changed.secret_inputs.reverse();
    assert_eq!(forward, source_input_revision(&changed));
}
