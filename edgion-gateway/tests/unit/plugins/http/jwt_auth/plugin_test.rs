use std::collections::HashMap;

use edgion_resources::resources::edgion_plugins::plugin_configs::{
    JwtAlgorithm, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
};

use super::*;

fn assert_send_sync<T: Send + Sync>() {}

fn config_with_resolved_credential() -> JwtAuthConfig {
    let credential = ResolvedJwtCredential {
        key: Some("kid-1".to_string()),
        secret: Some("prepared-secret-at-least-32-bytes-long".to_string()),
        public_key: None,
    };
    let group = ResolvedJwtIssuerGroup {
        group: "group-1".to_string(),
        algorithm: JwtAlgorithm::HS256,
        audiences: None,
        credentials: HashMap::from([("kid-1".to_string(), credential)]),
    };
    JwtAuthConfig {
        resolved_groups_by_iss: Some(HashMap::from([("issuer-1".to_string(), group)])),
        ..Default::default()
    }
}

#[test]
fn construction_consumes_only_the_runtime_config_resolved_credentials() {
    let config = config_with_resolved_credential();
    let auth = JwtAuth::new(&config, "default".to_string());

    assert!(config.resolved_groups_by_iss.is_some());
    assert!(auth.config.resolved_groups_by_iss.is_none());
    let prepared = auth.store.lookup_by_iss("issuer-1").unwrap();
    assert_eq!(prepared.group, "group-1");
    assert!(prepared.lookup_decoding_key("kid-1").unwrap().is_ok());
}

#[test]
fn prepared_jwt_auth_is_send_and_sync() {
    assert_send_sync::<JwtAuth>();
}
