use super::*;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};

fn slot(name: &str) -> SecretSlot {
    SecretSlot {
        secret_ref: SecretObjectReference {
            group: None,
            kind: None,
            name: name.into(),
            namespace: None,
        },
        secret: None,
    }
}

#[test]
fn redis_auth_provider_name() {
    let a = RedisAuth { slot: slot("r") };
    assert_eq!(<RedisAuth as AuthSpec>::provider_name(&a), "Redis");
    assert_eq!(<RedisAuth as AuthSpec>::slot(&a).secret_ref.name, "r");
}

#[test]
fn webhook_auth_apikey_slot_view() {
    let a = WebhookAuth::ApiKey {
        slot: slot("w"),
        r#in: "header".into(),
        header_name: Some("X-API-Key".into()),
        query_param: None,
    };
    assert_eq!(<WebhookAuth as AuthSpec>::slot(&a).secret_ref.name, "w");
}

#[test]
fn es_auth_basic_slot_view() {
    let a = ElasticsearchAuth::Basic { slot: slot("es") };
    assert_eq!(<ElasticsearchAuth as AuthSpec>::slot(&a).secret_ref.name, "es");
}

#[test]
fn kafka_sasl_provider_name_and_slot() {
    let a = KafkaSaslConfig {
        username: "user".to_string(),
        password: slot("kafka-secret"),
    };
    assert_eq!(<KafkaSaslConfig as AuthSpec>::provider_name(&a), "Kafka");
    assert_eq!(<KafkaSaslConfig as AuthSpec>::slot(&a).secret_ref.name, "kafka-secret");
}
