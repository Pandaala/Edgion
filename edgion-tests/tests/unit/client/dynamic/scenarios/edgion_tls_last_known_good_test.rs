use super::*;
use crate::framework::TestContext;
use std::cell::Cell;

fn context(root: &std::path::Path) -> ScenarioCtx {
    ScenarioCtx {
        test_ctx: TestContext {
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
            access_log_path: root.join("access.log"),
            admin_token: None,
        },
        conf_root: root.to_path_buf(),
        conf_path: "missing-fixtures".to_string(),
        controller_url: "invalid-controller-url".to_string(),
        gateway_url: String::new(),
        project_root: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf(),
    }
}

#[tokio::test]
async fn baseline_failure_precedes_every_resource_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let error = run(context(temp.path())).await.unwrap_err();
    // If an apply were attempted first, the invalid Controller URL would fail
    // instead. Missing baseline must stop before any transition or restoration.
    assert_eq!(error.to_string(), "failed to read the valid TLS Secret fixture");
    assert!(!temp.path().join("lkg_stashed_serial.txt").exists());
}

#[tokio::test]
async fn rotation_changes_fixture_serial_and_restoring_reseeds_original() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = context(temp.path());
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("integration/conf/EdgionTls/LastKnownGood/initial/Secret_lkg-secret.yaml");
    let initial = suites::LastKnownGoodInitialTestSuite::stash_fixture_serial(&ctx.test_ctx, &fixture)
        .await
        .unwrap();
    let rotated = generate_rotated_secret(&ctx).await.unwrap();
    let new_serial = suites::LastKnownGoodInitialTestSuite::stash_fixture_serial(&ctx.test_ctx, &rotated)
        .await
        .unwrap();
    assert_ne!(new_serial, initial);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("lkg_stashed_serial.txt")).unwrap(),
        new_serial
    );
    let restored = suites::LastKnownGoodInitialTestSuite::stash_fixture_serial(&ctx.test_ctx, &fixture)
        .await
        .unwrap();
    assert_eq!(restored, initial);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("lkg_stashed_serial.txt")).unwrap(),
        initial
    );
}

#[tokio::test]
async fn operational_deadline_reaches_restoration_and_preserves_failure() {
    // A controlled direct child exercises the actual child owner without a
    // TLS listener or another integration topology.
    let mut command = Command::new("sh");
    command.args(["-c", "while :; do :; done"]);
    let failure = tls_child::output(command, None, Instant::now() + Duration::from_millis(100))
        .await
        .unwrap_err();
    let restored = Cell::new(false);
    let error = restore_after_result(Err(failure.into()), async {
        restored.set(true);
        Ok(())
    })
    .await
    .unwrap_err();
    assert!(restored.get());
    assert!(error.to_string().contains("deadline exceeded"));
}

#[tokio::test]
async fn restoration_runs_for_both_outcomes_and_keeps_both_failures() {
    let restored = Cell::new(false);
    restore_after_result(Ok(()), async {
        restored.set(true);
        Ok(())
    })
    .await
    .unwrap();
    assert!(restored.get());
    let only_restore = restore_after_result(Ok(()), async { anyhow::bail!("restore failure") })
        .await
        .unwrap_err();
    assert!(format!("{only_restore:#}").contains("failed to restore the valid TLS Secret: restore failure"));
    let both = restore_after_result(Err(anyhow::anyhow!("transition failure")), async {
        anyhow::bail!("restore failure")
    })
    .await
    .unwrap_err();
    let message = format!("{both:#}");
    assert!(message.contains("transition failure"));
    assert!(message.contains("also failed to restore the valid TLS Secret: restore failure"));
}
