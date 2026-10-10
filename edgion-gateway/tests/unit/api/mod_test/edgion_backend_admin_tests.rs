use super::*;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
use edgion_resources::resources::edgion_backend::{
    AiBackendDefaults, AiBackendSpec, AiCredential, AiCredentialPool, AiModel, AiProvider, EdgionBackend,
    EdgionBackendSpec,
};
use edgion_resources::resources::edgion_plugins::OnRedisFailure;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

fn ai_backend(name: &str, endpoint: &str, credential: Option<&[u8]>) -> EdgionBackend {
    let secret = credential.map(|value| Secret {
        metadata: kube::api::ObjectMeta {
            name: Some("provider-key".into()),
            namespace: Some("ai-admin-test".into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([("api-key".to_string(), ByteString(value.to_vec()))])),
        ..Default::default()
    });
    let spec = EdgionBackendSpec {
        current_status: None,
        ai: AiBackendSpec {
            provider: AiProvider::OpenAICompatible,
            endpoint: Some(endpoint.to_string()),
            allow_insecure_http: false,
            credential_pool: AiCredentialPool {
                redis_ref: None,
                on_redis_failure: OnRedisFailure::Deny,
                credentials: vec![AiCredential {
                    name: "primary".to_string(),
                    weight: 1,
                    credential: SecretSlot {
                        secret_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "provider-key".to_string(),
                            namespace: None,
                        },
                        secret,
                    },
                    limits: None,
                }],
            },
            models: vec![AiModel {
                name: "model-a".to_string(),
                aliases: vec![],
                public: true,
                pricing: None,
            }],
            defaults: AiBackendDefaults::default(),
        },
        reconcile_outcome: None,
    };
    let mut service = EdgionBackend::new(name, spec);
    service.metadata.namespace = Some("ai-admin-test".to_string());
    service
}

#[test]
fn configclient_redacts_ai_credential_and_retains_inactive_resources() {
    let _store_guard = crate::ai::lock_global_ai_backend_store_for_test();
    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
    let _enter = rt.enter();
    let client = ConfigClient::new("cid-ai".to_string(), "cname-ai".to_string());
    let mut active = ai_backend("active", "https://ai.example.com/v1", Some(b"admin-private-key"));
    let mut secondary = active.spec.ai.credential_pool.credentials[0].clone();
    secondary.name = "secondary".to_string();
    secondary.credential.secret = Some(Secret {
        metadata: kube::api::ObjectMeta {
            name: Some("provider-key".into()),
            namespace: Some("ai-admin-test".into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            "api-key".to_string(),
            ByteString(b"secondary-private-key".to_vec()),
        )])),
        ..Default::default()
    });
    active.spec.ai.credential_pool.credentials.push(secondary);

    client.edgion_backends().reset(
        vec![
            active,
            ai_backend("unresolved", "https://ai.example.com/v1", None),
            ai_backend("invalid", "http://ai.example.com/v1", Some(b"invalid-private-key")),
        ],
        1,
    );

    let listed = list_client_resources(&client, ResourceKind::EdgionBackend);
    assert_eq!(
        listed.len(),
        3,
        "ClientCache must retain inactive resources for Admin visibility"
    );
    let active = listed
        .iter()
        .find(|value| value["metadata"]["name"] == "active")
        .expect("active resource listed");
    assert_eq!(
        active["spec"]["ai"]["credentialPool"]["credentials"][0]["secret"],
        "[redacted]"
    );
    assert_eq!(
        active["spec"]["ai"]["credentialPool"]["credentials"][1]["secret"],
        "[redacted]"
    );
    let serialized = serde_json::to_string(&listed).unwrap();
    assert!(!serialized.contains("admin-private-key"));
    assert!(!serialized.contains("secondary-private-key"));
    assert!(!serialized.contains("invalid-private-key"));

    let runtime = crate::ai::get_global_ai_backend_store();
    assert_eq!(runtime.count(), 1, "only the valid, resolved resource is activatable");
    assert!(runtime.get_by_key("ai-admin-test/active").is_some());
    assert!(runtime.get_by_key("ai-admin-test/unresolved").is_none());
    assert!(runtime.get_by_key("ai-admin-test/invalid").is_none());

    client.edgion_backends().reset(Vec::new(), 2);
    assert_eq!(runtime.count(), 0, "wipe must clear the derived runtime store");
}
