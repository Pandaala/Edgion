use super::*;
use std::io::{Read, Write};
use std::path::Path;
use tokio::time::timeout;

// An exact subprocess fixture avoids a second binary target or external server.
#[test]
#[ignore = "invoked only as an exact controlled child"]
fn controlled_child() {
    let mode = std::env::var("EDGION_TLS_CHILD_MODE").unwrap();
    let ready = std::env::var_os("EDGION_TLS_CHILD_READY").unwrap();
    if mode == "exchange" {
        // Fill both output pipes before reading input. Sequential parent IO
        // would deadlock once its own input pipe fills.
        std::io::stdout().write_all(&vec![b'o'; 256 * 1024]).unwrap();
        std::io::stderr().write_all(&vec![b'e'; 256 * 1024]).unwrap();
        let mut input = Vec::new();
        std::io::stdin().read_to_end(&mut input).unwrap();
        assert_eq!(input, vec![b'i'; 256 * 1024]);
        println!("input complete");
        return;
    }
    if mode == "reject_input" {
        std::process::exit(7);
    }
    if mode == "nonzero" {
        println!("handshake failure");
        eprintln!("negative evidence");
        std::process::exit(7);
    }
    if mode == "output" {
        std::io::stdout().write_all(&vec![b'o'; 256 * 1024]).unwrap();
        std::io::stderr().write_all(&vec![b'e'; 256 * 1024]).unwrap();
        std::io::stdout().flush().unwrap();
        std::io::stderr().flush().unwrap();
    }
    std::fs::write(ready, std::process::id().to_string()).unwrap();
    loop {
        std::thread::park();
    }
}

fn fixture(mode: &str, ready: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "tls_child::tests::controlled_child",
            "--ignored",
            "--nocapture",
        ])
        .env("EDGION_TLS_CHILD_MODE", mode)
        .env("EDGION_TLS_CHILD_READY", ready);
    command
}

async fn ready_pid(path: &Path) -> String {
    timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(path) {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("controlled child must reach its readiness marker")
}

async fn assert_reaped(reaped: oneshot::Receiver<io::Result<std::process::ExitStatus>>, pid: &str) {
    let status = timeout(Duration::from_secs(5), reaped)
        .await
        .expect("cleanup must finish on the controlled child")
        .expect("supervisor must acknowledge explicit wait")
        .expect("explicit wait must succeed");
    assert!(!status.success());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
        let mut command = Command::new("kill");
        command.args(["-0", pid]);
        let output = output(command, None, Instant::now() + Duration::from_secs(2))
            .await
            .unwrap();
        assert!(!output.status.success(), "the reaped child's PID must no longer exist");
    }
}

#[tokio::test]
async fn concurrent_input_and_output_finish_without_pipe_deadlock() {
    let temp = tempfile::tempdir().unwrap();
    let operation = start(
        fixture("exchange", &temp.path().join("ready")),
        Some(vec![b'i'; 256 * 1024]),
        Instant::now() + Duration::from_secs(5),
    );
    let output = operation.result.await.unwrap().unwrap();
    assert!(output.status.success());
    assert!(output
        .stdout
        .windows(b"input complete".len())
        .any(|part| part == b"input complete"));
    assert!(output.stderr.iter().filter(|byte| **byte == b'e').count() >= 256 * 1024);
    assert!(operation.reaped.await.unwrap().unwrap().success());
}

#[tokio::test]
async fn genuine_nonzero_exit_preserves_negative_handshake_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let operation = start(
        fixture("nonzero", &temp.path().join("ready")),
        None,
        Instant::now() + OPERATION_TIMEOUT,
    );
    let output = operation.result.await.unwrap().unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert!(String::from_utf8_lossy(&output.stdout).contains("handshake failure"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("negative evidence"));
    assert_eq!(operation.reaped.await.unwrap().unwrap().code(), Some(7));
}

#[tokio::test]
async fn deadline_interrupts_completion_and_blocked_input_then_explicitly_reaps() {
    for input in [None, Some(vec![b'i'; 4 * 1024 * 1024])] {
        let temp = tempfile::tempdir().unwrap();
        let ready = temp.path().join("ready");
        let operation = start(
            fixture("blocked", &ready),
            input,
            Instant::now() + Duration::from_millis(500),
        );
        let pid = ready_pid(&ready).await;
        // This executes on the same single-thread runtime while child IO is pending.
        tokio::task::yield_now().await;
        assert!(matches!(operation.result.await.unwrap(), Err(ChildError::Deadline)));
        assert_reaped(operation.reaped, &pid).await;
    }
}

#[tokio::test]
async fn caller_cancellation_during_input_and_output_retains_explicit_reap_owner() {
    for mode in ["blocked", "output"] {
        let temp = tempfile::tempdir().unwrap();
        let ready = temp.path().join("ready");
        let input = (mode == "blocked").then(|| vec![b'i'; 4 * 1024 * 1024]);
        let operation = start(fixture(mode, &ready), input, Instant::now() + OPERATION_TIMEOUT);
        let requester = tokio::spawn(operation.result);
        let pid = ready_pid(&ready).await;
        requester.abort();
        assert!(requester.await.unwrap_err().is_cancelled());
        assert_reaped(operation.reaped, &pid).await;
    }
}

#[tokio::test]
async fn expired_budget_does_not_spawn_a_new_child() {
    let temp = tempfile::tempdir().unwrap();
    let ready = temp.path().join("ready");
    assert!(matches!(
        output(fixture("blocked", &ready), None, Instant::now()).await,
        Err(ChildError::Deadline)
    ));
    assert!(!ready.exists());
}

#[tokio::test]
async fn broken_input_pipe_fails_operationally_and_still_acknowledges_wait() {
    let temp = tempfile::tempdir().unwrap();
    let operation = start(
        fixture("reject_input", &temp.path().join("ready")),
        Some(vec![b'i'; 4 * 1024 * 1024]),
        Instant::now() + OPERATION_TIMEOUT,
    );
    assert!(matches!(
        operation.result.await.unwrap(),
        Err(ChildError::Io(_) | ChildError::Terminate(_))
    ));
    let status = timeout(Duration::from_secs(5), operation.reaped)
        .await
        .unwrap()
        .expect("supervisor must acknowledge explicit wait after IO failure")
        .unwrap();
    assert!(!status.success());
}
