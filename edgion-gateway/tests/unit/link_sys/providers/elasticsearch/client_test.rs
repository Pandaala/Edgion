use super::*;

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
    let result = EsLinkClient::from_config("test", &config, &no_auth());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("empty"));
}

#[test]
fn test_try_send_bulk_not_initialized_returns_err() {
    let config = config_with_endpoints(vec!["http://localhost:9200".to_string()]);
    let client = EsLinkClient::from_config("test/client", &config, &no_auth())
        .expect("from_config should succeed with a valid endpoint");

    // bulk_sender is None until init() is called
    let result = client.try_send_bulk("test doc".to_string());
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("not initialized"), "unexpected error: {}", msg);
}
