use super::*;

#[tokio::test]
async fn probe_cluster_health_false_on_unreachable() {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(200))
        .build()
        .unwrap();
    // Port 1 is unreachable -> the probe must report false, never panic.
    assert!(!probe_cluster_health(&client, "http://127.0.0.1:1").await);
}
