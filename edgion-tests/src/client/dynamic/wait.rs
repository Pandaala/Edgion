// Wait helpers — reuse the existing resource_diff binary to avoid converting it into a library.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

/// Invoke target/debug/resource_diff to verify that the controller and gateway resource states align.
/// Returns Ok(()) when the diff passes; returns Err when there is a mismatch or the call fails.
pub async fn resource_diff(project_root: &Path, controller_url: &str, gateway_url: &str) -> Result<()> {
    let binary = project_root.join("target/debug/resource_diff");
    if !binary.exists() {
        anyhow::bail!(
            "resource_diff binary not found at {}. Run `cargo build -p edgion-tests --bin resource_diff`.",
            binary.display()
        );
    }
    let output = Command::new(&binary)
        .arg("--controller-url")
        .arg(controller_url)
        .arg("--gateway-url")
        .arg(gateway_url)
        .output()
        .await
        .with_context(|| format!("Failed to spawn {}", binary.display()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "resource_diff failed (exit={:?}):\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        ))
    }
}

/// Repeatedly invoke resource_diff within the given timeout until it passes or the deadline is reached.
/// A poll_interval of 500ms is recommended.
pub async fn wait_for_resource_sync(
    project_root: &Path,
    controller_url: &str,
    gateway_url: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last_err: Option<anyhow::Error> = None;
    while tokio::time::Instant::now() < deadline {
        match resource_diff(project_root, controller_url, gateway_url).await {
            Ok(()) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
        tokio::time::sleep(poll_interval).await;
    }
    Err(last_err.unwrap_or_else(|| anyhow!("wait_for_resource_sync timed out")))
}
