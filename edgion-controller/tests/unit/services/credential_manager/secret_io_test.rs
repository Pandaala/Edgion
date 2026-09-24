use super::*;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ManagedFieldsEntry;

fn identity() -> ManagedSecretIdentity {
    ManagedSecretIdentity {
        consumer: ConsumerIdentity {
            namespace: "tenant-a".to_string(),
            name: "provider-plugin".to_string(),
            uid: "consumer-uid".to_string(),
            generation: 7,
        },
        source: SourceIdentity {
            name: "ops/provider-a".to_string(),
            uid: "source-uid".to_string(),
            generation: 3,
        },
    }
}

fn metadata() -> PublicationMetadata {
    PublicationMetadata {
        publication_generation: "publication-old".to_string(),
        source_input_revision: "input-revision".to_string(),
        binding_revision: "binding-revision".to_string(),
        issued_at: "2026-08-27T00:00:00Z".to_string(),
        valid_until: "2026-08-27T01:00:00Z".to_string(),
        leader_term: "leader-old".to_string(),
    }
}

fn managed_secret() -> Secret {
    let identity = identity();
    let metadata = metadata();
    let publication = Publication {
        identity: &identity,
        metadata: &metadata,
        bearer_token: b"not-logged",
    };
    let mut secret = build_secret(&publication);
    secret.metadata.resource_version = Some("42".to_string());
    secret.metadata.uid = Some("secret-uid".to_string());
    secret.metadata.managed_fields = Some(vec![ManagedFieldsEntry {
        manager: Some(FIELD_MANAGER.to_string()),
        ..Default::default()
    }]);
    secret
}

#[test]
fn ownership_requires_exact_consumer_owner_and_markers() {
    let identity = identity();
    let secret = managed_secret();
    assert_eq!(classify_ownership(&secret, &identity), Ownership::Owned);
    assert!(can_adopt_create_race(&secret, &identity));

    let mut wrong_owner = secret.clone();
    wrong_owner.metadata.owner_references.as_mut().unwrap()[0].uid = "recreated-consumer".to_string();
    assert_eq!(classify_ownership(&wrong_owner, &identity), Ownership::Conflict);

    let mut unmanaged = secret.clone();
    unmanaged.metadata.labels.as_mut().unwrap().clear();
    assert_eq!(classify_ownership(&unmanaged, &identity), Ownership::Conflict);
    assert!(is_owned_by_consumer(&unmanaged, &identity));
}

#[test]
fn same_consumer_with_old_source_is_not_a_general_conflict() {
    let mut secret = managed_secret();
    secret
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(ANNOTATION_SOURCE_UID.to_string(), "old-source-uid".to_string());
    assert_eq!(
        classify_ownership(&secret, &identity()),
        Ownership::OwnedByConsumerForAnotherSource
    );
    assert!(!can_adopt_create_race(&secret, &identity()));
}

#[test]
fn create_race_adoption_requires_expected_field_manager() {
    let mut secret = managed_secret();
    secret.metadata.managed_fields = None;
    assert_eq!(classify_ownership(&secret, &identity()), Ownership::Owned);
    assert!(!can_adopt_create_race(&secret, &identity()));
}

#[test]
fn publication_patch_tests_old_state_before_atomic_replacement() {
    let identity = identity();
    let old_metadata = metadata();
    let new_metadata = PublicationMetadata {
        publication_generation: "publication-new".to_string(),
        leader_term: "leader-new".to_string(),
        ..old_metadata.clone()
    };
    let publication = Publication {
        identity: &identity,
        metadata: &new_metadata,
        bearer_token: b"new-token",
    };
    let cas = capture_publication_cas(&managed_secret(), &identity).unwrap();
    let patch = build_publication_patch(&cas, &publication).unwrap();
    let value = serde_json::to_value(patch).unwrap();
    let ops = value.as_array().unwrap();

    assert_eq!(
        ops[0],
        json!({"op":"test","path":"/metadata/resourceVersion","value":"42"})
    );
    assert_eq!(ops[1]["op"], "test");
    assert_eq!(ops[2]["value"], "consumer-uid");
    assert!(ops.iter().any(|op| {
        op["op"] == "test"
            && op["path"] == "/metadata/annotations/edgion.io~1credential-binding-revision"
            && op["value"] == "binding-revision"
    }));
    assert!(ops.iter().any(|op| op["path"] == "/metadata/annotations"));
    assert!(ops
        .iter()
        .any(|op| { op["path"] == "/data" && op["value"][DATA_BEARER_TOKEN] == "bmV3LXRva2Vu" }));
    assert!(ops.iter().any(|op| op["path"] == "/immutable" && op["value"] == false));
    assert!(ops.iter().any(|op| op["path"] == "/type" && op["value"] == "Opaque"));
}

#[test]
fn delete_uses_secret_uid_and_resource_version_preconditions() {
    let params = delete_params(&managed_secret()).unwrap();
    let preconditions = params.preconditions.unwrap();
    assert_eq!(preconditions.uid.as_deref(), Some("secret-uid"));
    assert_eq!(preconditions.resource_version.as_deref(), Some("42"));
}

#[test]
fn delete_expectation_captures_object_and_publication_identity() {
    let expected = capture_delete_expectation(&managed_secret(), identity(), "binding-revision".to_string()).unwrap();
    assert_eq!(expected.secret_uid, "secret-uid");
    assert_eq!(expected.publication_generation, "publication-old");
    assert_eq!(expected.source_input_revision, "input-revision");
    assert_eq!(expected.leader_term, "leader-old");
}
