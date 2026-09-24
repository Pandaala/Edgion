use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static PAYLOAD_DROPS: AtomicUsize = AtomicUsize::new(0);

struct PanickingPayloadDrop;

impl Drop for PanickingPayloadDrop {
    fn drop(&mut self) {
        PAYLOAD_DROPS.fetch_add(1, Ordering::Relaxed);
        panic!("payload destructor detail must stay redacted");
    }
}

#[test]
fn redacted_scope_hides_payload_and_restores_outer_behavior() {
    let secret = String::from("authorization bearer body-secret");
    assert_eq!(panic_payload_message(&secret), secret);

    let observed = catch_redacted_panic(|| panic_payload_message(&secret));
    assert_eq!(observed.unwrap(), "redacted extension panic");
    assert_eq!(panic_payload_message(&secret), secret);
}

#[test]
fn redacted_scope_nests_without_leaking_depth() {
    let secret = String::from("nested-secret");
    let outer = catch_redacted_panic(|| {
        let inner = catch_redacted_panic(|| panic_payload_message(&secret)).unwrap();
        assert_eq!(inner, "redacted extension panic");
        panic_payload_message(&secret)
    })
    .unwrap();
    assert_eq!(outer, "redacted extension panic");
    assert_eq!(panic_payload_message(&secret), secret);
}

#[test]
fn caught_payload_is_consumed_inside_scope_and_never_returned() {
    PAYLOAD_DROPS.store(0, Ordering::Relaxed);
    let result = catch_redacted_panic(|| -> () { std::panic::panic_any(PanickingPayloadDrop) });
    assert_eq!(result, Err(RedactedPanic));
    assert_eq!(PAYLOAD_DROPS.load(Ordering::Relaxed), 1);
    assert_eq!(REDACT_PANIC_DEPTH.with(Cell::get), 0);
}

#[test]
fn installed_hook_redacts_extension_payload_in_subprocess() {
    const CHILD_ENV: &str = "EDGION_REDACTED_PANIC_HOOK_CHILD";
    const SECRET: &str = "authorization bearer request-body-secret";
    if std::env::var_os(CHILD_ENV).is_some() {
        install_panic_hook();
        assert_eq!(catch_redacted_panic(|| panic!("{SECRET}")), Err(RedactedPanic));
        return;
    }

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "startup::tests::installed_hook_redacts_extension_payload_in_subprocess",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("redacted extension panic"), "stderr: {stderr}");
    assert!(!stderr.contains(SECRET), "stderr leaked secret: {stderr}");
}
