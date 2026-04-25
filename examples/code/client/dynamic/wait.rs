// Wait helpers — 复用既有 resource_diff binary，避免库化改造。

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

/// 调用 target/debug/examples/resource_diff 校验 controller 与 gateway 的资源状态对齐。
/// 返回 Ok(()) 表示 diff 通过；返回 Err 表示不对齐或调用失败。
pub async fn resource_diff(project_root: &Path, controller_url: &str, gateway_url: &str) -> Result<()> {
    let binary = project_root.join("target/debug/examples/resource_diff");
    if !binary.exists() {
        anyhow::bail!(
            "resource_diff binary not found at {}. Run `cargo build --example resource_diff`.",
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

/// 在给定 timeout 内重复调用 resource_diff 直至通过或超时。
/// poll_interval 建议 500ms。
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
