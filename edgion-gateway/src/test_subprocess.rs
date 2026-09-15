use std::future::Future;
use std::process::Command;

const COMPLETION_PREFIX: &str = "EDGION_TEST_SUBPROCESS_COMPLETE";

fn child_output(test_module: &str, test_name: &str, child_env: &str, child_case: &str) -> std::process::Output {
    let exact_test_name = format!("{test_module}::{test_name}");
    Command::new(std::env::current_exe().expect("the Rust test harness must have a current executable"))
        .args(["--exact", exact_test_name.as_str(), "--nocapture"])
        .env(child_env, child_case)
        .output()
        .expect("the isolated test process must start")
}

fn completion_marker(test_module: &str, test_name: &str, child_env: &str, child_case: &str) -> String {
    format!("{COMPLETION_PREFIX}:{test_module}::{test_name}:{child_env}:{child_case}")
}

fn assert_child_completed(output: &std::process::Output, marker: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.matches(marker).count() == 1,
        "isolated test failed or did not complete exactly once (status: {}):\nstdout: {stdout}\nstderr: {stderr}",
        output.status
    );
}

pub(crate) fn assert_in_subprocess(
    test_module: &str,
    test_name: &str,
    child_env: &str,
    child_case: &str,
    assertion: impl FnOnce(),
) {
    match std::env::var(child_env) {
        Ok(active_case) => {
            if active_case == child_case {
                assertion();
                println!("{}", completion_marker(test_module, test_name, child_env, child_case));
            }
        }
        Err(_) => {
            let marker = completion_marker(test_module, test_name, child_env, child_case);
            let output = child_output(test_module, test_name, child_env, child_case);
            assert_child_completed(&output, &marker);
        }
    }
}

pub(crate) async fn assert_in_subprocess_async<F, Fut>(
    test_module: &str,
    test_name: &str,
    child_env: &str,
    child_case: &str,
    assertion: F,
) where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
{
    match std::env::var(child_env) {
        Ok(active_case) => {
            if active_case == child_case {
                assertion().await;
                println!("{}", completion_marker(test_module, test_name, child_env, child_case));
            }
        }
        Err(_) => {
            let marker = completion_marker(test_module, test_name, child_env, child_case);
            let output = child_output(test_module, test_name, child_env, child_case);
            assert_child_completed(&output, &marker);
        }
    }
}
