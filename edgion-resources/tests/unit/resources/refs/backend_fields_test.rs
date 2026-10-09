use super::*;
use serde_json::json;

fn owner() -> EdgionBackend {
    serde_json::from_value(json!({
        "metadata":{"name":"provider","uid":"present-uid"},
        "spec":{"ai":{"provider":"OpenAI","credentialPool":{
            "redisRef":"NS_name/Cache_key","credentials":[{
                "name":"primary","secretRef":{"name":"credential"},"limits":{"requestsPerMinute":100}
            }]
        },"models":[{"name":"model-a"}]}}
    }))
    .unwrap()
}

#[test]
fn actual_backend_redis_field_borrows_exact_target_and_checks_source_not_uid() {
    let mut owner = owner();
    assert!(owner.spec.validate().is_ok());
    let reference = BackendCredentialPoolField::from_backend(&owner)
        .redis_reference()
        .unwrap()
        .unwrap();
    assert_eq!(reference, "NS_name/Cache_key");
    assert!(std::ptr::eq(
        reference,
        owner.spec.ai.credential_pool.redis_ref.as_deref().unwrap()
    ));
    assert_eq!(owner.metadata.namespace, None);
    for namespace in ["", "bad/ns"] {
        owner.metadata.namespace = Some(namespace.into());
        assert!(BackendCredentialPoolField::from_backend(&owner)
            .redis_reference()
            .is_err());
    }
    owner.metadata.namespace = None;
    for name in [None, Some(""), Some("bad/name")] {
        owner.metadata.name = name.map(str::to_owned);
        assert!(BackendCredentialPoolField::from_backend(&owner)
            .redis_reference()
            .is_err());
    }
    owner.spec.ai.credential_pool.redis_ref = None;
    assert_eq!(
        BackendCredentialPoolField::from_backend(&owner)
            .redis_reference()
            .unwrap(),
        None
    );
}

#[test]
fn backend_schema_retains_ascii_pairing_and_fresh_field_loans_observe_changes() {
    let mut owner = owner();
    for value in ["Upper_case/Target_1", "apps/cache"] {
        owner.spec.ai.credential_pool.redis_ref = Some(value.into());
        assert!(owner.spec.validate().is_ok());
        assert_eq!(
            BackendCredentialPoolField::from_backend(&owner)
                .redis_reference()
                .unwrap(),
            Some(value)
        );
    }
    for value in ["", "apps/", "/cache", "apps/cache/other", "café/cache"] {
        owner.spec.ai.credential_pool.redis_ref = Some(value.into());
        assert!(owner.spec.validate().is_err());
    }
    owner.spec.ai.credential_pool.redis_ref = None;
    assert!(owner.spec.validate().is_err(), "limited credentials require Redis");
    owner.spec.ai.credential_pool.credentials[0].limits = None;
    assert!(owner.spec.validate().is_ok());
    owner.spec.ai.credential_pool.redis_ref = Some("apps/cache".into());
    assert!(
        owner.spec.validate().is_err(),
        "unlimited pool forbids an unused Redis field"
    );
}

#[test]
fn actual_backend_secret_field_validates_source_and_custom_secret_profile() {
    let mut backend = owner();
    for group in [None, Some(""), Some("core")] {
        for kind in [None, Some("Secret")] {
            let wire = &mut backend.spec.ai.credential_pool.credentials[0].credential.secret_ref;
            wire.group = group.map(str::to_owned);
            wire.kind = kind.map(str::to_owned);
            let field = BackendCredentialPoolField::from_backend(&backend).secret_reference(0);
            let reference = field.validated().unwrap();
            assert_eq!(reference.source().scope().namespace(), Some("default"));
            assert_eq!(reference.target().group_kind().group(), "");
            assert_eq!(reference.target().group_kind().kind(), "Secret");
            assert_eq!(reference.target().name(), "credential");
            assert!(std::ptr::eq(
                reference.target().name().as_ptr(),
                backend.spec.ai.credential_pool.credentials[0]
                    .credential
                    .secret_ref
                    .name
                    .as_ptr()
            ));
        }
    }
    for (namespace, name) in [
        (Some(""), Some("provider")),
        (Some("bad/ns"), Some("provider")),
        (None, None),
        (None, Some("")),
        (None, Some("bad/name")),
    ] {
        let mut invalid = backend.clone();
        invalid.metadata.namespace = namespace.map(str::to_owned);
        invalid.metadata.name = name.map(str::to_owned);
        assert!(invalid.metadata.uid.is_some());
        assert!(BackendCredentialPoolField::from_backend(&invalid)
            .secret_reference(0)
            .validated()
            .is_err());
    }
    for (group, kind, namespace, name) in [
        (Some("other.io"), None, None, "credential"),
        (None, Some("ConfigMap"), None, "credential"),
        (None, Some(""), None, "credential"),
        (None, None, Some(""), "credential"),
        (None, None, Some("bad/ns"), "credential"),
        (None, None, None, ""),
        (None, None, None, "bad/name"),
    ] {
        let wire = &mut backend.spec.ai.credential_pool.credentials[0].credential.secret_ref;
        wire.group = group.map(str::to_owned);
        wire.kind = kind.map(str::to_owned);
        wire.namespace = namespace.map(str::to_owned);
        wire.name = name.into();
        assert!(BackendCredentialPoolField::from_backend(&backend)
            .secret_reference(0)
            .validated()
            .is_err());
    }
}

#[test]
fn backend_secret_ordinals_follow_current_fields_without_retained_descriptors() {
    let mut backend = owner();
    let mut second = backend.spec.ai.credential_pool.credentials[0].clone();
    second.name = "second".into();
    second.credential.secret_ref.name = "second-key".into();
    backend.spec.ai.credential_pool.credentials.push(second);
    for expected in [["credential", "second-key"], ["second-key", "credential"]] {
        for (index, expected) in expected.into_iter().enumerate() {
            let field = BackendCredentialPoolField::from_backend(&backend).secret_reference(index);
            assert_eq!(field.validated().unwrap().target().name(), expected);
        }
        backend.spec.ai.credential_pool.credentials.swap(0, 1);
    }
    backend.spec.ai.credential_pool.credentials.pop();
    assert_eq!(
        BackendCredentialPoolField::from_backend(&backend)
            .secret_reference(1)
            .validated()
            .unwrap_err(),
        ReferenceError::StaleBinding
    );
    backend.spec.ai.credential_pool.credentials[0]
        .credential
        .secret_ref
        .name = "updated".into();
    let field = BackendCredentialPoolField::from_backend(&backend).secret_reference(0);
    assert_eq!(field.validated().unwrap().target().name(), "updated");
}
