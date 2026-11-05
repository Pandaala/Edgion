use super::super::EtcdBootstrapConfig;
use super::*;

#[test]
fn source_range_bounds_keep_prefix_and_exclude_previous_key() {
    assert_eq!(
        range_bounds(b"/tenant/conf/", None).unwrap(),
        (b"/tenant/conf/".to_vec(), b"/tenant/conf0".to_vec())
    );
    assert_eq!(range_bounds(b"/a/", Some(b"/a/z")).unwrap().0, b"/a/z\0");
    assert_eq!(
        range_bounds(b"/a/", Some(b"/b/z")).unwrap_err(),
        SourceRangeError::Invalid
    );
    assert_eq!(range_bounds(b"", None).unwrap_err(), SourceRangeError::Invalid);
    assert_eq!(range_bounds(&[1, 255], None).unwrap().1, vec![2]);
    assert_eq!(range_bounds(&[255], None).unwrap_err(), SourceRangeError::Invalid);
}

#[test]
fn source_range_errors_never_retain_upstream_body() {
    for (code, expected) in [
        (tonic::Code::OutOfRange, SourceRangeError::Expired),
        (tonic::Code::PermissionDenied, SourceRangeError::Denied),
        (tonic::Code::ResourceExhausted, SourceRangeError::Budget),
        (tonic::Code::DeadlineExceeded, SourceRangeError::Timeout),
        (tonic::Code::Internal, SourceRangeError::Unavailable),
    ] {
        let error = range_status(code as i32, "sensitive-marker");
        assert_eq!(error, expected);
    }
    assert_eq!(
        range_status(11, "Error, decoded message length too large: found 999 bytes"),
        SourceRangeError::Budget
    );
    let error = range_error(etcd_client::Error::InvalidArgs("sensitive-marker".into()));
    assert_eq!(error, SourceRangeError::Unavailable);
    assert!(!error.to_string().contains("sensitive-marker"));
}

#[tokio::test]
#[ignore = "requires an isolated etcd in EDGION_SOURCE_TEST_ETCD_ENDPOINT"]
async fn source_range_real_snapshot_namespace_and_compaction() {
    let namespace = format!(
        "/source-range-{}/",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let client = EtcdLinkClient::from_bootstrap(
        "source-range-test",
        EtcdBootstrapConfig {
            endpoints: vec![std::env::var("EDGION_SOURCE_TEST_ETCD_ENDPOINT").expect("isolated test endpoint")],
            auth: None,
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: Some(namespace),
        },
    )
    .unwrap();
    client.init().await.unwrap();
    for key in ["/conf/a", "/conf/b", "/conf/c", "/conf-other/d"] {
        client.put_string(key, "original").await.unwrap();
    }
    assert_eq!(
        client.get_source_value("/conf/a").await.unwrap().unwrap().0,
        b"original"
    );
    assert!(client.get_source_value("/conf").await.unwrap().is_none());
    let first = client.get_prefix_page("/conf/", None, None, 1).await.unwrap();
    assert_eq!(first.entries[0].0, "/conf/a");
    assert!(first.more);
    client.put_string("/conf/b", "changed").await.unwrap();
    client.delete("/conf/c").await.unwrap();
    client.put_string("/conf/d", "new").await.unwrap();
    let second = client
        .get_prefix_page("/conf/", Some("/conf/a"), Some(first.revision), 2)
        .await
        .unwrap();
    assert_eq!(second.revision, first.revision);
    assert!(!second.more);
    assert_eq!(
        second
            .entries
            .iter()
            .map(|v| (v.0.as_str(), v.1.as_slice()))
            .collect::<Vec<_>>(),
        vec![("/conf/b", b"original".as_slice()), ("/conf/c", b"original".as_slice())]
    );
    let latest = client.get_prefix_page("/conf/", None, None, 100).await.unwrap();
    assert_eq!(latest.entries.len(), 3);
    assert!(latest.revision > first.revision);
    let mut native = client.get_client().await.unwrap();
    native.compact(latest.revision, None).await.unwrap();
    assert_eq!(
        client
            .get_prefix_page("/conf/", Some("/conf/a"), Some(first.revision), 1)
            .await
            .err()
            .unwrap(),
        SourceRangeError::Expired
    );
    assert_eq!(
        client
            .get_prefix_page("/conf/", Some("/conf-other/d"), None, 1)
            .await
            .err()
            .unwrap(),
        SourceRangeError::Invalid
    );
    client.delete_prefix("/conf").await.unwrap();
}

#[tokio::test]
#[ignore = "requires an isolated etcd in EDGION_SOURCE_TEST_ETCD_ENDPOINT"]
async fn source_range_real_oversize_is_budget_not_expired() {
    let prefix = format!(
        "/source-size-{}/",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let client = EtcdLinkClient::from_bootstrap(
        "source-size-test",
        EtcdBootstrapConfig {
            endpoints: vec![std::env::var("EDGION_SOURCE_TEST_ETCD_ENDPOINT").expect("isolated test endpoint")],
            auth: None,
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: None,
        },
    )
    .unwrap();
    client.init().await.unwrap();
    for i in 0..9 {
        client
            .put(&format!("{prefix}{i}"), vec![b'x'; 1024 * 1024], None)
            .await
            .unwrap();
    }
    assert_eq!(
        client.get_prefix_page(&prefix, None, None, 100).await.err().unwrap(),
        SourceRangeError::Budget
    );
    let small = client.get_prefix_page(&prefix, None, None, 1).await.unwrap();
    assert_eq!(small.entries.len(), 1);
    assert!(small.more);
    client.delete_prefix(&prefix).await.unwrap();
}
