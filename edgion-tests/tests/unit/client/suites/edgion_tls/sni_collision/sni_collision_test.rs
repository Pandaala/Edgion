use super::*;

#[cfg(unix)]
fn executable(directory: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = directory.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[cfg(unix)]
#[tokio::test]
async fn discovery_preserves_candidate_order_and_libressl_only_skip() {
    let temp = tempfile::tempdir().unwrap();
    let libre = executable(temp.path(), "libre", "printf 'LibreSSL test'");
    let real = executable(temp.path(), "real", "printf 'OpenSSL test'");
    let missing = temp.path().join("missing");
    let deadline = TokioInstant::now() + OPERATION_TIMEOUT;
    assert_eq!(
        openssl_from_candidates(&[missing.to_str().unwrap(), libre.to_str().unwrap()], deadline)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        openssl_from_candidates(&[libre.to_str().unwrap(), real.to_str().unwrap()], deadline)
            .await
            .unwrap()
            .as_deref(),
        real.to_str()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn discovery_deadline_and_spawn_errors_cannot_become_skips() {
    let temp = tempfile::tempdir().unwrap();
    let pending = executable(temp.path(), "pending", "while :; do :; done");
    let deadline = TokioInstant::now() + std::time::Duration::from_millis(100);
    assert!(matches!(
        openssl_from_candidates(&[pending.to_str().unwrap()], deadline).await,
        Err(ChildError::Deadline)
    ));
    let denied = temp.path().join("denied");
    std::fs::write(&denied, "not executable").unwrap();
    let error = openssl_from_candidates(&[denied.to_str().unwrap()], TokioInstant::now() + OPERATION_TIMEOUT)
        .await
        .unwrap_err();
    assert!(matches!(error, ChildError::Spawn(_)));
    assert!(!error.is_not_found());
}

#[cfg(unix)]
#[tokio::test]
async fn candidate_discovery_shares_one_deadline_across_processes() {
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let first = executable(
        temp.path(),
        "first",
        r#"printf ready > "${0}.ready"; sleep 2; printf done > "${0}.done"; printf unsupported"#,
    );
    let second = executable(temp.path(), "second", r#"printf ready > "${0}.ready"; exec sleep 30"#);
    let started = TokioInstant::now();
    let deadline = started + Duration::from_secs(5);
    let result = tokio::time::timeout(
        Duration::from_secs(6),
        openssl_from_candidates(&[first.to_str().unwrap(), second.to_str().unwrap()], deadline),
    )
    .await
    .expect("discovery must not restart its deadline for the second candidate");
    assert!(matches!(result, Err(ChildError::Deadline)));
    // Markers distinguish slow process startup/first-candidate completion from
    // reaching the pending second candidate. A reset budget takes at least 7s.
    assert!(
        temp.path().join("first.done").exists() && temp.path().join("second.ready").exists(),
        "candidate progress: first_started={}, first_finished={}, second_started={}, elapsed={:?}",
        temp.path().join("first.ready").exists(),
        temp.path().join("first.done").exists(),
        temp.path().join("second.ready").exists(),
        TokioInstant::now().saturating_duration_since(started),
    );
}
