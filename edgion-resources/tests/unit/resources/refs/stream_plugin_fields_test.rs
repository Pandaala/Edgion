use super::*;
use crate::resources::edgion_plugins::PluginValidation;
use serde_json::json;

fn owner(distributed: bool) -> EdgionStreamPlugins {
    serde_json::from_value(json!({
        "metadata":{"name":"limiter"},"spec":{"plugins":[{
            "type":"ConnectionRateLimit","config":{
                "redisRef":"café/cache_key",
                "perListener":{"rate":10,"interval":"1s","redisDistributed":distributed}
            }
        }]}
    }))
    .unwrap()
}

#[test]
fn active_stream_redis_field_checks_actual_owner_and_preserves_target_grammar() {
    let mut owner = owner(true);
    let field = StreamConnectionRateLimitField::from_plugins(&owner, 0).unwrap();
    let config = field.configuration().unwrap();
    let EdgionStreamPlugin::ConnectionRateLimit(actual) = &owner.spec.plugins.as_ref().unwrap()[0].plugin else {
        unreachable!()
    };
    assert!(std::ptr::eq(config, actual));
    assert!(config.validation_error().is_none());
    assert_eq!(owner.metadata.namespace, None);
    assert!(StreamConnectionRateLimitField::from_plugins(&owner, 1).is_none());
    for namespace in ["", "bad/ns"] {
        owner.metadata.namespace = Some(namespace.into());
        assert!(StreamConnectionRateLimitField::from_plugins(&owner, 0)
            .unwrap()
            .configuration()
            .is_err());
    }
    owner.metadata.namespace = None;
    for name in [None, Some(""), Some("bad/name")] {
        owner.metadata.name = name.map(str::to_owned);
        assert!(StreamConnectionRateLimitField::from_plugins(&owner, 0)
            .unwrap()
            .configuration()
            .is_err());
    }
    owner.spec.plugins.as_mut().unwrap()[0].enable = false;
    assert!(StreamConnectionRateLimitField::from_plugins(&owner, 0)
        .unwrap()
        .configuration()
        .is_ok());
}

#[test]
fn inactive_stream_redis_fields_and_fresh_dimension_changes_use_actual_config() {
    let mut owner = owner(false);
    owner.metadata.name = None;
    let EdgionStreamPlugin::ConnectionRateLimit(config) = &mut owner.spec.plugins.as_mut().unwrap()[0].plugin else {
        unreachable!()
    };
    config.redis_ref = Some("unused/not/a/key".into());
    let config = StreamConnectionRateLimitField::from_plugins(&owner, 0)
        .unwrap()
        .configuration()
        .unwrap();
    assert!(!config.needs_redis());
    assert!(config.validation_error().is_none());
    assert_eq!(config.redis_ref.as_deref(), Some("unused/not/a/key"));
    let EdgionStreamPlugin::ConnectionRateLimit(config) = &mut owner.spec.plugins.as_mut().unwrap()[0].plugin else {
        unreachable!()
    };
    config.per_listener.as_mut().unwrap().redis_distributed = true;
    assert!(StreamConnectionRateLimitField::from_plugins(&owner, 0)
        .unwrap()
        .configuration()
        .is_err());
    owner.metadata.name = Some("limiter".into());
    let config = StreamConnectionRateLimitField::from_plugins(&owner, 0)
        .unwrap()
        .configuration()
        .unwrap();
    assert!(config.needs_redis());
    assert!(
        config.validation_error().is_some(),
        "target validation remains schema-owned"
    );
    let EdgionStreamPlugin::ConnectionRateLimit(config) = &mut owner.spec.plugins.as_mut().unwrap()[0].plugin else {
        unreachable!()
    };
    config.redis_ref = None;
    assert!(StreamConnectionRateLimitField::from_plugins(&owner, 0)
        .unwrap()
        .configuration()
        .unwrap()
        .validation_error()
        .is_some());
    owner.spec.plugins.as_mut().unwrap()[0].plugin = EdgionStreamPlugin::Unsupported;
    assert!(StreamConnectionRateLimitField::from_plugins(&owner, 0).is_none());
}
