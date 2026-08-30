//! Kubernetes persistence primitives for controller-issued OAuth2 credentials.
//!
//! These helpers deliberately keep ownership checks, optimistic concurrency,
//! and mutation construction together. A caller must never infer ownership
//! from the deterministic Secret name alone.

use std::collections::BTreeMap;

use base64::Engine as _;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use k8s_openapi::ByteString;
use kube::api::{Api, DeleteParams, Patch, PatchParams, PostParams, Preconditions};
use serde_json::{json, Value};
use thiserror::Error;

use crate::conf_mgr::sync_runtime::resource_processor::credential_source_state::{
    BINDING_REVISION_ANNOTATION, CONSUMER_UID_ANNOTATION, MANAGED_BY_LABEL,
    MANAGED_BY_VALUE as REGISTRY_MANAGED_BY_VALUE, SOURCE_GENERATION_ANNOTATION, SOURCE_UID_ANNOTATION,
    VALID_UNTIL_ANNOTATION,
};

pub const FIELD_MANAGER: &str = "edgion-oauth2-credential-controller";
pub const DATA_BEARER_TOKEN: &str = "bearerToken";

pub const LABEL_MANAGED_BY: &str = MANAGED_BY_LABEL;
pub const MANAGED_BY_VALUE: &str = REGISTRY_MANAGED_BY_VALUE;

pub const ANNOTATION_CONSUMER_NAME: &str = "edgion.io/credential-consumer-name";
pub const ANNOTATION_CONSUMER_UID: &str = CONSUMER_UID_ANNOTATION;
pub const ANNOTATION_CONSUMER_GENERATION: &str = "edgion.io/credential-consumer-generation";
pub const ANNOTATION_SOURCE_NAME: &str = "edgion.io/credential-source-name";
pub const ANNOTATION_SOURCE_UID: &str = SOURCE_UID_ANNOTATION;
pub const ANNOTATION_SOURCE_GENERATION: &str = SOURCE_GENERATION_ANNOTATION;
pub const ANNOTATION_PUBLICATION_GENERATION: &str = "edgion.io/credential-publication-generation";
pub const ANNOTATION_SOURCE_INPUT_REVISION: &str = "edgion.io/credential-source-input-revision";
pub const ANNOTATION_BINDING_REVISION: &str = BINDING_REVISION_ANNOTATION;
pub const ANNOTATION_ISSUED_AT: &str = "edgion.io/credential-issued-at";
pub const ANNOTATION_VALID_UNTIL: &str = VALID_UNTIL_ANNOTATION;
pub const ANNOTATION_LEADER_TERM: &str = "edgion.io/credential-leader-term";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsumerIdentity {
    pub namespace: String,
    pub name: String,
    pub uid: String,
    pub generation: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceIdentity {
    /// Canonical `namespace/name` identity. This is metadata, never a URL.
    pub name: String,
    pub uid: String,
    pub generation: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedSecretIdentity {
    pub consumer: ConsumerIdentity,
    pub source: SourceIdentity,
}

/// All non-secret publication metadata written atomically with the token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationMetadata {
    pub publication_generation: String,
    pub source_input_revision: String,
    pub binding_revision: String,
    pub issued_at: String,
    pub valid_until: String,
    pub leader_term: String,
}

/// A publication payload intentionally has no `Debug` implementation.
pub struct Publication<'a> {
    pub identity: &'a ManagedSecretIdentity,
    pub metadata: &'a PublicationMetadata,
    pub bearer_token: &'a [u8],
}

/// Markers captured by a reconcile attempt before it performs issuer I/O.
///
/// Tests for every field below are included in the JSON Patch. A publication
/// from an obsolete source, binding, generation, or leader term therefore
/// fails with HTTP 409 instead of overwriting a successor's Secret.
#[derive(Clone, Debug, PartialEq)]
pub struct PublicationCas {
    resource_version: String,
    owner_references: Vec<OwnerReference>,
    consumer_uid: String,
    consumer_generation: String,
    source_uid: String,
    source_generation: String,
    publication_generation: String,
    source_input_revision: String,
    binding_revision: String,
    leader_term: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ownership {
    /// Exact managed marker and exact consuming EdgionPlugins owner UID.
    Owned,
    /// A same-owner object whose source marker belongs to a prior binding.
    OwnedByConsumerForAnotherSource,
    /// Missing managed markers, missing owner, or a different owner UID.
    Conflict,
}

#[derive(Debug, Error)]
pub enum SecretIoError {
    #[error("managed credential Secret target is owned by another actor")]
    TargetSecretConflict,
    #[error("managed credential Secret is missing required Kubernetes metadata")]
    MissingMetadata,
    #[error("managed credential Secret mutation failed")]
    Kubernetes(#[source] kube::Error),
    #[error("failed to construct managed credential Secret patch")]
    InvalidPatch(#[source] serde_json::Error),
}

impl From<kube::Error> for SecretIoError {
    fn from(error: kube::Error) -> Self {
        Self::Kubernetes(error)
    }
}

pub fn owner_reference(identity: &ManagedSecretIdentity) -> OwnerReference {
    OwnerReference {
        api_version: "edgion.io/v1".to_string(),
        kind: "EdgionPlugins".to_string(),
        name: identity.consumer.name.clone(),
        uid: identity.consumer.uid.clone(),
        controller: Some(true),
        block_owner_deletion: None,
    }
}

fn is_exact_owner(owner: &OwnerReference, identity: &ManagedSecretIdentity) -> bool {
    owner.api_version == "edgion.io/v1"
        && owner.kind == "EdgionPlugins"
        && owner.name == identity.consumer.name
        && owner.uid == identity.consumer.uid
        && owner.controller == Some(true)
        && owner.block_owner_deletion != Some(true)
}

pub fn is_owned_by_consumer(secret: &Secret, identity: &ManagedSecretIdentity) -> bool {
    let owners = secret.metadata.owner_references.as_deref().unwrap_or_default();
    owners.len() == 1 && owners.first().is_some_and(|owner| is_exact_owner(owner, identity))
}

pub fn classify_ownership(secret: &Secret, identity: &ManagedSecretIdentity) -> Ownership {
    let labels = secret.metadata.labels.as_ref();
    let annotations = secret.metadata.annotations.as_ref();

    let managed = labels.and_then(|m| m.get(LABEL_MANAGED_BY)).map(String::as_str) == Some(MANAGED_BY_VALUE);
    let consumer_markers_match = annotations
        .and_then(|m| m.get(ANNOTATION_CONSUMER_NAME))
        .map(String::as_str)
        == Some(identity.consumer.name.as_str())
        && annotations
            .and_then(|m| m.get(ANNOTATION_CONSUMER_UID))
            .map(String::as_str)
            == Some(identity.consumer.uid.as_str());
    let exact_owner = is_owned_by_consumer(secret, identity);

    if !(managed && consumer_markers_match && exact_owner) {
        return Ownership::Conflict;
    }

    let source_matches = annotations
        .and_then(|m| m.get(ANNOTATION_SOURCE_NAME))
        .map(String::as_str)
        == Some(identity.source.name.as_str())
        && annotations
            .and_then(|m| m.get(ANNOTATION_SOURCE_UID))
            .map(String::as_str)
            == Some(identity.source.uid.as_str());
    if source_matches {
        Ownership::Owned
    } else {
        Ownership::OwnedByConsumerForAnotherSource
    }
}

pub fn has_expected_manager(secret: &Secret) -> bool {
    secret
        .metadata
        .managed_fields
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|entry| entry.manager.as_deref() == Some(FIELD_MANAGER))
}

/// A 409 winner from an initial POST is adoptable only when both identity
/// markers and the expected Kubernetes field manager match.
pub fn can_adopt_create_race(secret: &Secret, identity: &ManagedSecretIdentity) -> bool {
    classify_ownership(secret, identity) == Ownership::Owned && has_expected_manager(secret)
}

fn required_annotation(secret: &Secret, key: &'static str) -> Result<String, SecretIoError> {
    secret
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(key))
        .cloned()
        .ok_or(SecretIoError::MissingMetadata)
}

/// Capture the only valid publication CAS token after an exact ownership gate.
pub fn capture_publication_cas(
    secret: &Secret,
    identity: &ManagedSecretIdentity,
) -> Result<PublicationCas, SecretIoError> {
    if classify_ownership(secret, identity) == Ownership::Conflict {
        return Err(SecretIoError::TargetSecretConflict);
    }
    Ok(PublicationCas {
        resource_version: secret
            .metadata
            .resource_version
            .clone()
            .ok_or(SecretIoError::MissingMetadata)?,
        owner_references: secret
            .metadata
            .owner_references
            .clone()
            .ok_or(SecretIoError::MissingMetadata)?,
        consumer_uid: required_annotation(secret, ANNOTATION_CONSUMER_UID)?,
        consumer_generation: required_annotation(secret, ANNOTATION_CONSUMER_GENERATION)?,
        source_uid: required_annotation(secret, ANNOTATION_SOURCE_UID)?,
        source_generation: required_annotation(secret, ANNOTATION_SOURCE_GENERATION)?,
        publication_generation: required_annotation(secret, ANNOTATION_PUBLICATION_GENERATION)?,
        source_input_revision: required_annotation(secret, ANNOTATION_SOURCE_INPUT_REVISION)?,
        binding_revision: required_annotation(secret, ANNOTATION_BINDING_REVISION)?,
        leader_term: required_annotation(secret, ANNOTATION_LEADER_TERM)?,
    })
}

fn ownership_annotations(identity: &ManagedSecretIdentity) -> BTreeMap<String, String> {
    BTreeMap::from([
        (ANNOTATION_CONSUMER_NAME.to_string(), identity.consumer.name.clone()),
        (ANNOTATION_CONSUMER_UID.to_string(), identity.consumer.uid.clone()),
        (
            ANNOTATION_CONSUMER_GENERATION.to_string(),
            identity.consumer.generation.to_string(),
        ),
        (ANNOTATION_SOURCE_NAME.to_string(), identity.source.name.clone()),
        (ANNOTATION_SOURCE_UID.to_string(), identity.source.uid.clone()),
        (
            ANNOTATION_SOURCE_GENERATION.to_string(),
            identity.source.generation.to_string(),
        ),
    ])
}

fn publication_annotations(publication: &Publication<'_>) -> BTreeMap<String, String> {
    let mut annotations = ownership_annotations(publication.identity);
    annotations.extend([
        (
            ANNOTATION_PUBLICATION_GENERATION.to_string(),
            publication.metadata.publication_generation.clone(),
        ),
        (
            ANNOTATION_SOURCE_INPUT_REVISION.to_string(),
            publication.metadata.source_input_revision.clone(),
        ),
        (
            ANNOTATION_BINDING_REVISION.to_string(),
            publication.metadata.binding_revision.clone(),
        ),
        (ANNOTATION_ISSUED_AT.to_string(), publication.metadata.issued_at.clone()),
        (
            ANNOTATION_VALID_UNTIL.to_string(),
            publication.metadata.valid_until.clone(),
        ),
        (
            ANNOTATION_LEADER_TERM.to_string(),
            publication.metadata.leader_term.clone(),
        ),
    ]);
    annotations
}

fn build_secret(publication: &Publication<'_>) -> Secret {
    Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(publication.identity.consumer.name.clone()),
            namespace: Some(publication.identity.consumer.namespace.clone()),
            labels: Some(BTreeMap::from([(
                LABEL_MANAGED_BY.to_string(),
                MANAGED_BY_VALUE.to_string(),
            )])),
            annotations: Some(publication_annotations(publication)),
            owner_references: Some(vec![owner_reference(publication.identity)]),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            DATA_BEARER_TOKEN.to_string(),
            ByteString(publication.bearer_token.to_vec()),
        )])),
        immutable: Some(false),
        type_: Some("Opaque".to_string()),
        ..Default::default()
    }
}

/// POST a complete initial publication using the stable field manager.
pub async fn create(publication: &Publication<'_>, api: &Api<Secret>) -> Result<Secret, SecretIoError> {
    let params = PostParams {
        field_manager: Some(FIELD_MANAGER.to_string()),
        ..PostParams::default()
    };
    api.create(&params, &build_secret(publication))
        .await
        .map_err(Into::into)
}

fn annotation_test(path: &str, value: &str) -> Value {
    json!({ "op": "test", "path": path, "value": value })
}

/// Build the atomic RFC 6902 publication patch.
pub fn build_publication_patch(
    cas: &PublicationCas,
    publication: &Publication<'_>,
) -> Result<json_patch::Patch, SecretIoError> {
    if cas.owner_references.len() != 1
        || !is_exact_owner(&cas.owner_references[0], publication.identity)
        || cas.consumer_uid != publication.identity.consumer.uid
    {
        return Err(SecretIoError::TargetSecretConflict);
    }
    let annotations = publication_annotations(publication);
    let data = BTreeMap::from([(
        DATA_BEARER_TOKEN.to_string(),
        base64::engine::general_purpose::STANDARD.encode(publication.bearer_token),
    )]);
    let operations = json!([
        { "op": "test", "path": "/metadata/resourceVersion", "value": cas.resource_version },
        { "op": "test", "path": "/metadata/ownerReferences", "value": cas.owner_references },
        annotation_test("/metadata/annotations/edgion.io~1credential-consumer-uid", &cas.consumer_uid),
        annotation_test("/metadata/annotations/edgion.io~1credential-consumer-generation", &cas.consumer_generation),
        annotation_test("/metadata/annotations/edgion.io~1credential-source-uid", &cas.source_uid),
        annotation_test("/metadata/annotations/edgion.io~1credential-source-generation", &cas.source_generation),
        annotation_test("/metadata/annotations/edgion.io~1credential-publication-generation", &cas.publication_generation),
        annotation_test("/metadata/annotations/edgion.io~1credential-source-input-revision", &cas.source_input_revision),
        annotation_test("/metadata/annotations/edgion.io~1credential-binding-revision", &cas.binding_revision),
        annotation_test("/metadata/annotations/edgion.io~1credential-leader-term", &cas.leader_term),
        { "op": "add", "path": "/metadata/labels", "value": {
            (LABEL_MANAGED_BY): MANAGED_BY_VALUE
        }},
        { "op": "replace", "path": "/metadata/annotations", "value": annotations },
        { "op": "add", "path": "/data", "value": data },
        { "op": "add", "path": "/immutable", "value": false },
        { "op": "add", "path": "/type", "value": "Opaque" }
    ]);
    serde_json::from_value(operations).map_err(SecretIoError::InvalidPatch)
}

pub async fn publish(
    api: &Api<Secret>,
    cas: &PublicationCas,
    publication: &Publication<'_>,
) -> Result<Secret, SecretIoError> {
    let patch = build_publication_patch(cas, publication)?;
    let params = PatchParams {
        field_manager: Some(FIELD_MANAGER.to_string()),
        ..PatchParams::default()
    };
    api.patch(&publication.identity.consumer.name, &params, &Patch::Json::<()>(patch))
        .await
        .map_err(Into::into)
}

/// Atomically restore all managed metadata and publish data when the exact
/// EdgionPlugins owner is intact but one or more controller markers were
/// removed. The caller must validate the live leader fence immediately before
/// invoking this RV/owner-fenced patch.
pub async fn repair_and_publish_owned_consumer(
    api: &Api<Secret>,
    current: &Secret,
    publication: &Publication<'_>,
) -> Result<Secret, SecretIoError> {
    if !is_owned_by_consumer(current, publication.identity) {
        return Err(SecretIoError::TargetSecretConflict);
    }
    let resource_version = current
        .metadata
        .resource_version
        .clone()
        .ok_or(SecretIoError::MissingMetadata)?;
    let owners = current
        .metadata
        .owner_references
        .clone()
        .ok_or(SecretIoError::MissingMetadata)?;
    let annotations = publication_annotations(publication);
    let data = BTreeMap::from([(
        DATA_BEARER_TOKEN.to_string(),
        base64::engine::general_purpose::STANDARD.encode(publication.bearer_token),
    )]);
    let operations = json!([
        { "op": "test", "path": "/metadata/resourceVersion", "value": resource_version },
        { "op": "test", "path": "/metadata/ownerReferences", "value": owners },
        { "op": "add", "path": "/metadata/labels", "value": {
            (LABEL_MANAGED_BY): MANAGED_BY_VALUE
        }},
        { "op": "add", "path": "/metadata/annotations", "value": annotations },
        { "op": "add", "path": "/data", "value": data },
        { "op": "add", "path": "/immutable", "value": false },
        { "op": "add", "path": "/type", "value": "Opaque" }
    ]);
    let patch = serde_json::from_value(operations).map_err(SecretIoError::InvalidPatch)?;
    let params = PatchParams {
        field_manager: Some(FIELD_MANAGER.to_string()),
        ..PatchParams::default()
    };
    api.patch(&publication.identity.consumer.name, &params, &Patch::Json::<()>(patch))
        .await
        .map_err(Into::into)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteExpectation {
    pub identity: ManagedSecretIdentity,
    pub binding_revision: String,
    pub secret_uid: String,
    pub publication_generation: String,
    pub source_input_revision: String,
    pub leader_term: String,
}

pub fn capture_delete_expectation(
    secret: &Secret,
    identity: ManagedSecretIdentity,
    binding_revision: String,
) -> Result<DeleteExpectation, SecretIoError> {
    if classify_ownership(secret, &identity) != Ownership::Owned {
        return Err(SecretIoError::TargetSecretConflict);
    }
    Ok(DeleteExpectation {
        identity,
        binding_revision,
        secret_uid: secret.metadata.uid.clone().ok_or(SecretIoError::MissingMetadata)?,
        publication_generation: required_annotation(secret, ANNOTATION_PUBLICATION_GENERATION)?,
        source_input_revision: required_annotation(secret, ANNOTATION_SOURCE_INPUT_REVISION)?,
        leader_term: required_annotation(secret, ANNOTATION_LEADER_TERM)?,
    })
}

fn delete_params(secret: &Secret) -> Result<DeleteParams, SecretIoError> {
    let uid = secret.metadata.uid.clone().ok_or(SecretIoError::MissingMetadata)?;
    let resource_version = secret
        .metadata
        .resource_version
        .clone()
        .ok_or(SecretIoError::MissingMetadata)?;
    Ok(DeleteParams {
        preconditions: Some(Preconditions {
            uid: Some(uid),
            resource_version: Some(resource_version),
        }),
        ..DeleteParams::default()
    })
}

pub async fn delete_owned_consumer_object(
    api: &Api<Secret>,
    secret: &Secret,
    identity: &ManagedSecretIdentity,
) -> Result<(), SecretIoError> {
    if !is_owned_by_consumer(secret, identity) {
        return Err(SecretIoError::TargetSecretConflict);
    }
    let params = delete_params(secret)?;
    api.delete(&identity.consumer.name, &params).await?;
    Ok(())
}

/// Re-GET, recheck exact old markers, then delete with Secret UID and RV
/// preconditions. Returns `false` when cleanup is obsolete or the object is gone.
pub async fn delete_if_matches(api: &Api<Secret>, expected: &DeleteExpectation) -> Result<bool, SecretIoError> {
    let Some(secret) = api.get_opt(&expected.identity.consumer.name).await? else {
        return Ok(false);
    };
    if classify_ownership(&secret, &expected.identity) != Ownership::Owned {
        return Ok(false);
    }
    if secret.metadata.uid.as_deref() != Some(expected.secret_uid.as_str()) {
        return Ok(false);
    }
    let annotations = secret.metadata.annotations.as_ref();
    let matches =
        |key: &str, expected: &str| annotations.and_then(|m| m.get(key)).map(String::as_str) == Some(expected);
    if !matches(ANNOTATION_BINDING_REVISION, &expected.binding_revision)
        || !matches(ANNOTATION_PUBLICATION_GENERATION, &expected.publication_generation)
        || !matches(ANNOTATION_SOURCE_INPUT_REVISION, &expected.source_input_revision)
        || !matches(ANNOTATION_LEADER_TERM, &expected.leader_term)
    {
        return Ok(false);
    }

    let params = delete_params(&secret)?;
    api.delete(&expected.identity.consumer.name, &params).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
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
        let expected =
            capture_delete_expectation(&managed_secret(), identity(), "binding-revision".to_string()).unwrap();
        assert_eq!(expected.secret_uid, "secret-uid");
        assert_eq!(expected.publication_generation, "publication-old");
        assert_eq!(expected.source_input_revision, "input-revision");
        assert_eq!(expected.leader_term, "leader-old");
    }
}
