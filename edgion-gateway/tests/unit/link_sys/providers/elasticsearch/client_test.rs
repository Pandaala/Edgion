use super::*;
use crate::link_sys::credential::{CredentialState, ResolvedCredential};
use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchClientConfig;
use edgion_resources::resources::link_sys::LinkSysSpec;

fn from_config(
    label: &str,
    config: &ElasticsearchClientConfig,
    auth: &CredentialState<ResolvedCredential>,
) -> Result<EsLinkClient> {
    assert!(matches!(auth, CredentialState::NotRequired));
    let (namespace, name) = label.split_once('/').unwrap_or(("test", label));
    let mut owner = LinkSys::new(
        name,
        LinkSysSpec {
            config: SystemConfig::Elasticsearch(config.clone()),
            current_status: None,
            unknown_fields: Vec::new(),
        },
    );
    owner.metadata.namespace = Some(namespace.into());
    EsLinkClient::from_link_sys(Arc::new(owner))
}

fn config_with_endpoints(endpoints: Vec<String>) -> ElasticsearchClientConfig {
    ElasticsearchClientConfig {
        endpoints,
        auth: None,
        tls: None,
        timeout: None,
        pool: None,
        bulk: None,
        index: None,
    }
}

fn no_auth() -> CredentialState<ResolvedCredential> {
    CredentialState::NotRequired
}

#[test]
fn test_empty_endpoints_returns_error() {
    let config = config_with_endpoints(vec![]);
    let result = from_config("test", &config, &no_auth());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("empty"));
}

#[test]
fn test_try_send_bulk_not_initialized_returns_err() {
    let config = config_with_endpoints(vec!["http://localhost:9200".to_string()]);
    let client =
        from_config("test/client", &config, &no_auth()).expect("from_config should succeed with a valid endpoint");

    // bulk_sender is None until init() is called
    let result = client.try_send_bulk("test doc".to_string());
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("not initialized"), "unexpected error: {}", msg);
}
