use super::*;

#[tokio::test]
async fn missing_identity_fails_before_constructing_the_etcd_client() {
    // An invalid endpoint makes precedence observable without any network IO.
    let config: EtcdConfig = serde_yaml::from_str("endpoints: ['not-an-endpoint']").unwrap();
    let Err(error) = EtcdCenter::new(config).await else {
        panic!("missing controller identity must fail startup");
    };
    assert!(error.to_string().contains("conf_center.controller_name is required"));
}
