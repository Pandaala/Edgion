use super::*;

fn context(access_log_path: PathBuf) -> TestContext {
    TestContext {
        target_host: "127.0.0.1".to_string(),
        http_port: 0,
        grpc_port: 0,
        websocket_port: 0,
        tcp_port: 0,
        tcp_filtered_port: 0,
        udp_port: 0,
        https_port: 0,
        grpc_https_port: 0,
        admin_port: 0,
        http_client: reqwest::Client::new(),
        http_host: None,
        grpc_host: None,
        gateway: true,
        verbose: false,
        access_log_path,
        admin_token: None,
    }
}

#[tokio::test]
async fn exact_fixture_serial_round_trips_through_openssl_and_stash() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = context(temp.path().join("access.log"));
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("integration/conf/EdgionTls/LastKnownGood/initial/Secret_lkg-secret.yaml");
    let serial = LastKnownGoodInitialTestSuite::stash_fixture_serial(&ctx, &fixture)
        .await
        .unwrap();
    assert!(!serial.is_empty());
    assert!(serial.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(std::fs::read_to_string(stash_path(&ctx)).unwrap(), serial);
    assert_eq!(
        LastKnownGoodInitialTestSuite::stash_fixture_serial(&ctx, &fixture)
            .await
            .unwrap(),
        serial
    );
}

#[tokio::test]
async fn parser_deadline_is_an_operational_failure() {
    let error = get_pem_serial(b"not a certificate", TokioInstant::now())
        .await
        .unwrap_err();
    assert!(error.contains("deadline exceeded"));
}
