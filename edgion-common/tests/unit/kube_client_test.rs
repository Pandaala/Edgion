use super::*;

#[test]
fn request_policy_preserves_inferred_cluster_settings() {
    let mut config = Config::new("https://cluster.example:6443".parse().unwrap());
    config.default_namespace = "tenant".into();
    config.root_cert = Some(vec![vec![1, 2, 3]]);
    config.root_cert_file = Some("/service-account/ca.crt".into());
    config.auth_info.token = Some("test-token".to_string().into());
    config.proxy_url = Some("http://proxy.example:8080".parse().unwrap());
    config.tls_server_name = Some("api.example".into());
    config
        .headers
        .push((http::header::ACCEPT, "application/json".parse().unwrap()));
    let before = config.clone();
    let config = client_config(config);
    assert!(!config.default_retry);
    assert_eq!(config.read_timeout, Some(Duration::from_secs(295)));
    assert!(config.root_cert_file.is_none());
    assert_eq!(config.cluster_url, before.cluster_url);
    assert_eq!(config.default_namespace, before.default_namespace);
    assert_eq!(config.root_cert, before.root_cert);
    assert_eq!(
        serde_json::to_value(&config.auth_info).unwrap(),
        serde_json::to_value(&before.auth_info).unwrap()
    );
    assert_eq!(config.proxy_url, before.proxy_url);
    assert_eq!(config.tls_server_name, before.tls_server_name);
    assert_eq!(config.headers, before.headers);
    assert_eq!(config.connect_timeout, before.connect_timeout);
    assert_eq!(config.write_timeout, before.write_timeout);
    assert_eq!(config.accept_invalid_certs, before.accept_invalid_certs);
    assert_eq!(config.disable_compression, before.disable_compression);
}

#[tokio::test]
async fn multi_document_kubeconfig_preserves_context_credentials_and_ca() {
    use base64::Engine;
    use kube::config::{KubeConfigOptions, Kubeconfig};
    let cert = rcgen::generate_simple_self_signed(vec!["api.example".into()]).unwrap();
    let pem = cert.cert.pem();
    let ca = base64::engine::general_purpose::STANDARD.encode(pem.as_bytes());
    let source = format!(
        r#"
apiVersion: v1
kind: Config
current-context: selected
clusters:
- name: cluster
  cluster:
    server: https://api.example:6443
    certificate-authority-data: {ca}
    proxy-url: http://proxy.example:8080
    tls-server-name: api.example
---
apiVersion: v1
kind: Config
contexts:
- name: selected
  context:
    cluster: cluster
    user: identity
    namespace: tenant
users:
- name: identity
  user:
    token: test-token
"#
    );
    let parsed = Kubeconfig::from_yaml(&source).unwrap();
    let inferred = Config::from_custom_kubeconfig(parsed, &KubeConfigOptions::default())
        .await
        .unwrap();
    let config = client_config(inferred);
    assert_eq!(config.cluster_url.to_string(), "https://api.example:6443/");
    assert_eq!(config.default_namespace, "tenant");
    assert_eq!(config.root_cert, Some(vec![cert.cert.der().to_vec()]));
    assert_eq!(serde_json::to_value(&config.auth_info).unwrap()["token"], "test-token");
    assert_eq!(config.proxy_url.unwrap().to_string(), "http://proxy.example:8080/");
    assert_eq!(config.tls_server_name.as_deref(), Some("api.example"));
    assert!(!config.accept_invalid_certs);
}
