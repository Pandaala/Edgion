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
async fn discovery_preserves_help_criterion_order_and_unsupported_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let unsupported = executable(temp.path(), "unsupported", "printf 'unsupported' >&2; exit 1");
    let supported = executable(temp.path(), "supported", "printf -- '-noservername' >&2; exit 1");
    let missing = temp.path().join("missing");
    let deadline = TokioInstant::now() + OPERATION_TIMEOUT;
    assert_eq!(
        openssl_from_candidates(&[missing.to_str().unwrap(), unsupported.to_str().unwrap()], deadline)
            .await
            .unwrap(),
        "openssl"
    );
    assert_eq!(
        openssl_from_candidates(&[unsupported.to_str().unwrap(), supported.to_str().unwrap()], deadline)
            .await
            .unwrap(),
        supported.to_str().unwrap()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn discovery_deadline_cannot_fall_back_to_an_unsupported_tool() {
    let temp = tempfile::tempdir().unwrap();
    let pending = executable(temp.path(), "pending", "while :; do :; done");
    let deadline = TokioInstant::now() + std::time::Duration::from_millis(100);
    assert!(matches!(
        openssl_from_candidates(&[pending.to_str().unwrap()], deadline).await,
        Err(ChildError::Deadline)
    ));
    let denied = temp.path().join("denied");
    std::fs::write(&denied, "not executable").unwrap();
    assert!(matches!(
        openssl_from_candidates(&[denied.to_str().unwrap()], TokioInstant::now() + OPERATION_TIMEOUT).await,
        Err(ChildError::Spawn(_))
    ));
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
