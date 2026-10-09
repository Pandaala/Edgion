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
